//! Research precise-root backend for LLVM stack maps.
//!
//! The plain-map prototype places `llvm.experimental.stackmap` immediately
//! before mapped calls and records the address of each native root alloca.
//! The statepoint prototype instead records LLVM-owned spill slots for
//! `gc.relocate` values. Both are writable frame-register-relative locations
//! in the emitted stack-map section.
//!
//! This first implementation deliberately targets macOS, where the experiment
//! is being measured. It discovers the concatenated `__LLVM_STACKMAPS` section
//! in the main Mach-O image and uses the platform unwinder to recover the
//! frame-register value for each active generated frame. Unsupported targets
//! return no roots; neither native-stack experiment may be used for correctness
//! there.

use super::{MutableRootSlot, MutableRootSlotKind};
use crate::gc::telemetry::RootSourcesTraceStats;
use std::ffi::c_void;
use std::sync::OnceLock;

const STACK_MAP_VERSION: u8 = 3;
const LOCATION_DIRECT: u8 = 2;
const LOCATION_INDIRECT: u8 = 3;
const MAX_SAFEPOINT_RETURN_DELTA: usize = 16;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StackMapLocation {
    dwarf_reg: u16,
    offset: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StackMapRecord {
    pc: usize,
    /// Start address of the containing function, from the stack-map header.
    /// Used to decode that function's prologue when an SP-relative location
    /// needs the FP-to-SP offset (see `fp_to_sp_offset`).
    function_address: usize,
    /// The containing function's total frame size from the stack-map header.
    stack_size: u64,
    locations: Vec<StackMapLocation>,
}

/// Parsed section plus the facts the fast walker's preconditions need.
///
/// `chain_walkable` is decided once at parse time: the raw x29-chain walk can
/// recover only the frame pointer (register 29) directly, plus the body SP
/// (register 31) derived from the header's per-function stack size. Any other
/// register anywhere in the maps disables the fast path for the whole image
/// rather than risking a wrong base mid-walk.
#[derive(Debug, Default)]
struct StackMapIndex {
    records: Vec<StackMapRecord>,
    chain_walkable: bool,
    min_pc: usize,
    max_pc: usize,
}

static STACK_MAPS: OnceLock<StackMapIndex> = OnceLock::new();

const DWARF_REG_FP_AARCH64: u16 = 29;
const DWARF_REG_SP_AARCH64: u16 = 31;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WalkerMode {
    /// x29-chain walk when `chain_walkable`, transparent unwinder fallback otherwise.
    Fast,
    /// Force the platform unwinder (bisection control).
    Unwind,
    /// Run both walks and panic unless they visit the identical slot set.
    /// This is the only check that can catch a fast walk that silently skips
    /// frames: forced-evacuation verification enumerates roots through the
    /// same walker, so it cannot see a slot the walker never reached.
    Verify,
}

fn walker_mode() -> WalkerMode {
    static MODE: OnceLock<WalkerMode> = OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("PERRY_STACKMAP_WALKER").as_deref() {
        Ok("unwind") => WalkerMode::Unwind,
        Ok("verify") => WalkerMode::Verify,
        _ => WalkerMode::Fast,
    })
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::gc) struct NativeStackWalkStats {
    pub(in crate::gc) walks: usize,
    pub(in crate::gc) frames_visited: usize,
    pub(in crate::gc) records_matched: usize,
    pub(in crate::gc) locations_visited: usize,
    pub(in crate::gc) fp_walks: usize,
    pub(in crate::gc) fallback_walks: usize,
}

#[inline]
pub(in crate::gc) fn record_native_stack_walk_source(
    stats: NativeStackWalkStats,
    root_sources: &mut Option<&mut RootSourcesTraceStats>,
) {
    if let Some(sources) = root_sources {
        sources.native_stack_maps.record_walk(
            stats.walks,
            stats.frames_visited,
            stats.records_matched,
            stats.locations_visited,
            stats.fp_walks,
            stats.fallback_walks,
        );
    }
}

pub(in crate::gc) fn initialize() {
    let _ = stack_maps();
}

/// Whether this image carries any native stack-map records — i.e. whether
/// precise frame roots depend on mapped PCs at all. Consumed by the
/// `PERRY_GC_SAFEPOINT_ONLY` contract assert.
pub(in crate::gc) fn native_maps_active() -> bool {
    !stack_maps().records.is_empty()
}

fn stack_maps() -> &'static StackMapIndex {
    STACK_MAPS.get_or_init(|| {
        let Some(section) = loaded_stack_map_section() else {
            return StackMapIndex::default();
        };
        let mut records = parse_concatenated_stack_maps(section).unwrap_or_default();
        records.sort_unstable_by_key(|record| record.pc);
        index_records(records)
    })
}

fn index_records(records: Vec<StackMapRecord>) -> StackMapIndex {
    // SP-relative locations are admitted here and resolved per FRAME in the
    // walker, which decodes the owning function's `add x29, sp, #imm`
    // prologue to get the body SP (#7173). Deciding it here would mean
    // dereferencing every function address at startup — unsafe for records
    // whose addresses are not live code, and unnecessary because the walker
    // already fails closed to the platform unwinder on any anomaly.
    let chain_walkable = records.iter().all(|record| {
        record.locations.iter().all(|location| {
            matches!(
                location.dwarf_reg,
                DWARF_REG_FP_AARCH64 | DWARF_REG_SP_AARCH64
            )
        })
    });
    let min_pc = records.first().map_or(usize::MAX, |record| record.pc);
    let max_pc = records.last().map_or(0, |record| record.pc);
    StackMapIndex {
        records,
        chain_walkable,
        min_pc,
        max_pc,
    }
}

/// Recover a function's frame-pointer-to-stack-pointer offset by decoding its
/// prologue (#7173).
///
/// AArch64 prologues set the frame pointer with a single
/// `add x29, sp, #imm` after saving the `[x29, x30]` pair, so the body SP is
/// `fp - imm`. On Darwin that offset is a constant (the pair sits at the top
/// of the frame) but on Linux it varies per function with the callee-save
/// area laid out below the pair — measured 0x30 and 0x60 in adjacent
/// generated functions, which is why no `(fp, stack_size)` formula works
/// there and the fast chain previously fell back to the DWARF unwinder for
/// every collection (~22% of samples on a Pi 5).
///
/// Instruction encoding: ADD (immediate, 64-bit, shift 0) with Rn = 31 (sp)
/// and Rd = 29 (fp) — `word & 0xFFC0_03FF == 0x9100_03FD`, immediate in bits
/// [21:10]. Scans a bounded prologue window and fails closed (`None`) if the
/// pattern is absent, in which case the caller uses the platform unwinder.
#[cfg(target_arch = "aarch64")]
fn fp_to_sp_offset(function_address: usize) -> Option<usize> {
    const ADD_FP_SP_MASK: u32 = 0xFFC0_03FF;
    const ADD_FP_SP_PATTERN: u32 = 0x9100_03FD;
    const PROLOGUE_WINDOW_INSNS: usize = 24;
    if function_address == 0 || function_address & 0x3 != 0 {
        return None;
    }
    for i in 0..PROLOGUE_WINDOW_INSNS {
        let word = unsafe { std::ptr::read((function_address + i * 4) as *const u32) };
        if word & ADD_FP_SP_MASK == ADD_FP_SP_PATTERN {
            return Some(((word >> 10) & 0xFFF) as usize);
        }
        // `ret` ends the prologue window for a leaf that never sets up fp.
        if word == 0xD65F_03C0 {
            break;
        }
    }
    None
}

#[cfg(not(target_arch = "aarch64"))]
fn fp_to_sp_offset(_function_address: usize) -> Option<usize> {
    None
}

fn closest_record_pc(maps: &[StackMapRecord], ip: usize) -> Option<usize> {
    let insertion = maps.partition_point(|record| record.pc < ip);
    let before = insertion
        .checked_sub(1)
        .and_then(|idx| maps.get(idx))
        .map(|record| record.pc);
    let at_or_after = maps.get(insertion).map(|record| record.pc);
    match (before, at_or_after) {
        (Some(before), Some(after)) => Some(if ip.abs_diff(before) <= ip.abs_diff(after) {
            before
        } else {
            after
        }),
        (Some(before), None) => Some(before),
        (None, Some(after)) => Some(after),
        (None, None) => None,
    }
}

impl StackMapIndex {
    /// The records describing the frame whose return address is `ip`: the
    /// ±16-byte nearest-PC match, which can select several records at one PC
    /// (plain maps sit just before the call, statepoints exactly at the
    /// return address).
    fn match_records(&self, ip: usize) -> &[StackMapRecord] {
        let Some(candidate_pc) = closest_record_pc(&self.records, ip) else {
            return &[];
        };
        if ip.abs_diff(candidate_pc) > MAX_SAFEPOINT_RETURN_DELTA {
            return &[];
        }
        let first = self
            .records
            .partition_point(|record| record.pc < candidate_pc);
        let last = self
            .records
            .partition_point(|record| record.pc <= candidate_pc);
        &self.records[first..last]
    }
}

pub(super) fn visit_stack_map_root_slots(
    visit: &mut impl FnMut(MutableRootSlot),
) -> NativeStackWalkStats {
    let index = stack_maps();
    if index.records.is_empty() {
        return NativeStackWalkStats::default();
    }
    match walker_mode() {
        WalkerMode::Unwind => unwind::visit(index, visit),
        WalkerMode::Fast => {
            if index.chain_walkable {
                if let Some(stats) = fp_chain::visit(index, visit) {
                    return stats;
                }
            }
            let mut stats = unwind::visit(index, visit);
            stats.fallback_walks = 1;
            stats
        }
        WalkerMode::Verify => verify_visit(index, visit),
    }
}

/// Debug-only cross-check: the fast walk reads slot addresses without
/// mutating, then the unwinder performs the real visitation while recording
/// what it reached. Any set difference is a missed or invented frame and
/// panics immediately — this is the liveness gate for the fast walker itself.
fn verify_visit(
    index: &StackMapIndex,
    visit: &mut impl FnMut(MutableRootSlot),
) -> NativeStackWalkStats {
    let mut fast_addresses: Vec<usize> = Vec::new();
    let fast_stats = fp_chain::visit(index, &mut |slot: MutableRootSlot| {
        fast_addresses.push(slot.ptr as usize);
    });
    let Some(fast_stats) = fast_stats else {
        panic!(
            "PERRY_STACKMAP_WALKER=verify: fast walk unavailable \
             (chain_walkable={}, anomaly or unsupported target)",
            index.chain_walkable
        );
    };
    let mut unwind_addresses: Vec<usize> = Vec::new();
    let mut stats = unwind::visit(index, &mut |slot: MutableRootSlot| {
        unwind_addresses.push(slot.ptr as usize);
        visit(slot);
    });
    fast_addresses.sort_unstable();
    fast_addresses.dedup();
    unwind_addresses.sort_unstable();
    unwind_addresses.dedup();
    assert_eq!(
        fast_addresses,
        unwind_addresses,
        "PERRY_STACKMAP_WALKER=verify: fast walk visited {} unique slots, \
         unwinder visited {}",
        fast_addresses.len(),
        unwind_addresses.len()
    );
    stats.fp_walks = fast_stats.fp_walks;
    stats
}

fn parse_concatenated_stack_maps(bytes: &[u8]) -> Option<Vec<StackMapRecord>> {
    let mut all = Vec::new();
    let mut base = 0usize;
    while base < bytes.len() {
        // Linkers preserve the input section's 8-byte alignment. Ignore a
        // zero-filled tail, but do not search through malformed non-zero data.
        if bytes[base..].iter().all(|byte| *byte == 0) {
            break;
        }
        let (mut records, consumed) = parse_one_stack_map(&bytes[base..])?;
        if consumed == 0 {
            return None;
        }
        all.append(&mut records);
        base = base.checked_add(consumed)?;
    }
    Some(all)
}

fn parse_one_stack_map(bytes: &[u8]) -> Option<(Vec<StackMapRecord>, usize)> {
    if read_u8(bytes, 0)? != STACK_MAP_VERSION {
        return None;
    }
    let function_count = read_u32(bytes, 4)? as usize;
    let constant_count = read_u32(bytes, 8)? as usize;
    let record_count = read_u32(bytes, 12)? as usize;
    let mut offset = 16usize;

    let mut functions = Vec::with_capacity(function_count);
    let mut expected_records = 0usize;
    for _ in 0..function_count {
        let address = read_u64(bytes, offset)? as usize;
        let stack_size = read_u64(bytes, offset + 8)?;
        let records = read_u64(bytes, offset + 16)? as usize;
        functions.push((address, stack_size, records));
        expected_records = expected_records.checked_add(records)?;
        offset = offset.checked_add(24)?;
    }
    if expected_records != record_count {
        return None;
    }
    offset = offset.checked_add(constant_count.checked_mul(8)?)?;
    if offset > bytes.len() {
        return None;
    }

    let mut out = Vec::with_capacity(record_count);
    for (function_address, function_stack_size, function_record_count) in functions {
        for _ in 0..function_record_count {
            let instruction_offset = read_u32(bytes, offset + 8)? as usize;
            let location_count = read_u16(bytes, offset + 14)? as usize;
            offset = offset.checked_add(16)?;

            let mut locations = Vec::new();
            for _ in 0..location_count {
                let kind = read_u8(bytes, offset)?;
                let size = read_u16(bytes, offset + 2)?;
                let dwarf_reg = read_u16(bytes, offset + 4)?;
                let location_offset = read_i32(bytes, offset + 8)?;
                if matches!(kind, LOCATION_DIRECT | LOCATION_INDIRECT) && size == 8 {
                    let location = StackMapLocation {
                        dwarf_reg,
                        offset: location_offset,
                    };
                    // A statepoint records a base/derived pair for every
                    // relocation. Perry currently uses the same value for
                    // both, so LLVM commonly emits the exact same spill slot
                    // twice. Visit that physical word once.
                    if !locations.contains(&location) {
                        locations.push(location);
                    }
                }
                offset = offset.checked_add(12)?;
            }

            // LLVM aligns the live-out header independently from the whole
            // record. This first padding is observable whenever the location
            // count is odd (one Direct root is a common case).
            offset = align_up(offset, 8)?;
            // Two reserved bytes followed by the live-out count.
            let live_out_count = read_u16(bytes, offset + 2)? as usize;
            offset = offset
                .checked_add(4)?
                .checked_add(live_out_count.checked_mul(4)?)?;
            offset = align_up(offset, 8)?;
            if offset > bytes.len() {
                return None;
            }

            out.push(StackMapRecord {
                pc: function_address.checked_add(instruction_offset)?,
                function_address,
                stack_size: function_stack_size,
                locations,
            });
        }
    }
    Some((out, offset))
}

fn align_up(value: usize, alignment: usize) -> Option<usize> {
    value
        .checked_add(alignment.checked_sub(1)?)
        .map(|value| value & !(alignment - 1))
}

fn read_u8(bytes: &[u8], offset: usize) -> Option<u8> {
    bytes.get(offset).copied()
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_i32(bytes: &[u8], offset: usize) -> Option<i32> {
    Some(i32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

#[cfg(target_os = "macos")]
fn loaded_stack_map_section() -> Option<&'static [u8]> {
    use mach2::dyld::{_dyld_get_image_header, _dyld_get_image_vmaddr_slide};

    const LC_SEGMENT_64: u32 = 0x19;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct MachHeader64 {
        magic: u32,
        cpu_type: i32,
        cpu_subtype: i32,
        file_type: u32,
        command_count: u32,
        commands_size: u32,
        flags: u32,
        reserved: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct LoadCommand {
        command: u32,
        size: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SegmentCommand64 {
        command: u32,
        size: u32,
        segment_name: [u8; 16],
        vm_address: u64,
        vm_size: u64,
        file_offset: u64,
        file_size: u64,
        max_protection: i32,
        initial_protection: i32,
        section_count: u32,
        flags: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Section64 {
        section_name: [u8; 16],
        segment_name: [u8; 16],
        address: u64,
        size: u64,
        offset: u32,
        alignment: u32,
        relocation_offset: u32,
        relocation_count: u32,
        flags: u32,
        reserved1: u32,
        reserved2: u32,
        reserved3: u32,
    }

    fn fixed_name_matches(actual: &[u8; 16], expected: &[u8]) -> bool {
        actual.get(..expected.len()) == Some(expected)
            && actual.get(expected.len()).copied().unwrap_or(0) == 0
    }

    unsafe {
        let raw_header = _dyld_get_image_header(0);
        if raw_header.is_null() {
            return None;
        }
        let header = &*(raw_header.cast::<MachHeader64>());
        let slide = _dyld_get_image_vmaddr_slide(0);
        let mut command_ptr = raw_header
            .cast::<u8>()
            .add(std::mem::size_of::<MachHeader64>());
        for _ in 0..header.command_count {
            let load = std::ptr::read_unaligned(command_ptr.cast::<LoadCommand>());
            if load.size < std::mem::size_of::<LoadCommand>() as u32 {
                return None;
            }
            if load.command == LC_SEGMENT_64 {
                let segment = std::ptr::read_unaligned(command_ptr.cast::<SegmentCommand64>());
                let mut section_ptr = command_ptr.add(std::mem::size_of::<SegmentCommand64>());
                for _ in 0..segment.section_count {
                    let section = std::ptr::read_unaligned(section_ptr.cast::<Section64>());
                    if fixed_name_matches(&section.segment_name, b"__LLVM_STACKMAPS")
                        && fixed_name_matches(&section.section_name, b"__llvm_stackmaps")
                    {
                        let address = (section.address as isize).checked_add(slide)? as usize;
                        let size = usize::try_from(section.size).ok()?;
                        if address == 0 || size == 0 {
                            return None;
                        }
                        return Some(std::slice::from_raw_parts(address as *const u8, size));
                    }
                    section_ptr = section_ptr.add(std::mem::size_of::<Section64>());
                }
            }
            command_ptr = command_ptr.add(load.size as usize);
        }
    }
    None
}

/// ELF (#7173): the `.llvm_stackmaps` section of the main executable.
///
/// Linker-provided `__start_`/`__stop_` symbols would need weak linkage
/// (unstable in Rust) or `-rdynamic` (not guaranteed), so instead: read
/// `/proc/self/exe`'s section headers for `.llvm_stackmaps` (sh_addr,
/// sh_size) and add the main object's load bias from the first
/// `dl_iterate_phdr` callback. Runtime-verified gates for this path are
/// pending a Linux host — tracked in #7173; the parser, index, matching,
/// and verify machinery above are platform-independent already.
#[cfg(target_os = "linux")]
fn loaded_stack_map_section() -> Option<&'static [u8]> {
    let bytes = std::fs::read("/proc/self/exe").ok()?;
    let (addr, size) = elf_section_vaddr(&bytes, b".llvm_stackmaps")?;
    let bias = main_object_load_bias()?;
    let start = bias.checked_add(addr)?;
    if start == 0 || size == 0 {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(start as *const u8, size) })
}

/// Minimal ELF64 section-header walk: returns (sh_addr, sh_size) for the
/// named section. Same defensive read style as the stack-map parser.
#[cfg(target_os = "linux")]
fn elf_section_vaddr(bytes: &[u8], name: &[u8]) -> Option<(usize, usize)> {
    if bytes.get(..4)? != b"\x7fELF" || *bytes.get(4)? != 2 {
        return None; // not ELF64
    }
    let shoff = read_u64(bytes, 0x28)? as usize;
    let shentsize = read_u16(bytes, 0x3A)? as usize;
    let shnum = read_u16(bytes, 0x3C)? as usize;
    let shstrndx = read_u16(bytes, 0x3E)? as usize;
    let strtab_hdr = shoff.checked_add(shstrndx.checked_mul(shentsize)?)?;
    let strtab_off = read_u64(bytes, strtab_hdr.checked_add(0x18)?)? as usize;
    for i in 0..shnum {
        let hdr = shoff.checked_add(i.checked_mul(shentsize)?)?;
        let name_off = read_u32(bytes, hdr)? as usize;
        let name_pos = strtab_off.checked_add(name_off)?;
        let candidate = bytes.get(name_pos..name_pos.checked_add(name.len())?)?;
        let terminator = bytes.get(name_pos + name.len()).copied().unwrap_or(1);
        if candidate == name && terminator == 0 {
            let addr = read_u64(bytes, hdr.checked_add(0x10)?)? as usize;
            let size = read_u64(bytes, hdr.checked_add(0x20)?)? as usize;
            return Some((addr, size));
        }
    }
    None
}

/// Load bias of the main object: `dlpi_addr` of the first `dl_iterate_phdr`
/// callback (the executable itself on glibc and musl).
#[cfg(target_os = "linux")]
fn main_object_load_bias() -> Option<usize> {
    #[repr(C)]
    struct DlPhdrInfo {
        dlpi_addr: usize,
        dlpi_name: *const std::os::raw::c_char,
        // remaining fields unused
    }
    unsafe extern "C" {
        fn dl_iterate_phdr(
            callback: unsafe extern "C" fn(*mut DlPhdrInfo, usize, *mut c_void) -> i32,
            data: *mut c_void,
        ) -> i32;
    }
    unsafe extern "C" fn first(info: *mut DlPhdrInfo, _size: usize, data: *mut c_void) -> i32 {
        unsafe {
            *data.cast::<usize>() = (*info).dlpi_addr;
        }
        1 // stop after the first (main) object
    }
    let mut bias = usize::MAX;
    unsafe {
        dl_iterate_phdr(first, (&mut bias as *mut usize).cast::<c_void>());
    }
    (bias != usize::MAX).then_some(bias)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn loaded_stack_map_section() -> Option<&'static [u8]> {
    None
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod unwind {
    use super::*;

    #[repr(C)]
    struct UnwindContext {
        _private: [u8; 0],
    }

    unsafe extern "C" {
        fn _Unwind_Backtrace(
            trace: unsafe extern "C" fn(*mut UnwindContext, *mut c_void) -> i32,
            argument: *mut c_void,
        ) -> i32;
        fn _Unwind_GetIP(context: *mut UnwindContext) -> usize;
        fn _Unwind_GetGR(context: *mut UnwindContext, register: i32) -> usize;
    }

    struct WalkState<'a, F> {
        index: &'a StackMapIndex,
        visit: &'a mut F,
        stats: NativeStackWalkStats,
    }

    pub(super) fn visit<F: FnMut(MutableRootSlot)>(
        index: &StackMapIndex,
        visit: &mut F,
    ) -> NativeStackWalkStats {
        let mut state = WalkState {
            index,
            visit,
            stats: NativeStackWalkStats {
                walks: 1,
                ..NativeStackWalkStats::default()
            },
        };
        unsafe {
            _Unwind_Backtrace(
                walk_frame::<F>,
                (&mut state as *mut WalkState<'_, _>).cast::<c_void>(),
            );
        }
        state.stats
    }

    unsafe extern "C" fn walk_frame<F: FnMut(MutableRootSlot)>(
        context: *mut UnwindContext,
        argument: *mut c_void,
    ) -> i32 {
        let state = &mut *argument.cast::<WalkState<'_, F>>();
        state.stats.frames_visited = state.stats.frames_visited.saturating_add(1);
        let ip = _Unwind_GetIP(context);
        let matched = state.index.match_records(ip);
        if matched.is_empty() {
            return 0;
        }
        state.stats.records_matched = state.stats.records_matched.saturating_add(matched.len());
        for record in matched {
            for location in &record.locations {
                state.stats.locations_visited = state.stats.locations_visited.saturating_add(1);
                let base = _Unwind_GetGR(context, i32::from(location.dwarf_reg));
                let address = if location.offset < 0 {
                    base.checked_sub(location.offset.unsigned_abs() as usize)
                } else {
                    base.checked_add(location.offset as usize)
                };
                let Some(address) = address else {
                    continue;
                };
                if address == 0 || address & (std::mem::align_of::<u64>() - 1) != 0 {
                    continue;
                }
                (state.visit)(MutableRootSlot {
                    kind: MutableRootSlotKind::NativeStack,
                    ptr: address as *mut u64,
                });
            }
        }
        0
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod unwind {
    use super::*;

    pub(super) fn visit(
        _index: &StackMapIndex,
        _visit: &mut impl FnMut(MutableRootSlot),
    ) -> NativeStackWalkStats {
        NativeStackWalkStats::default()
    }
}

/// Raw x29-chain walker.
///
/// AArch64 prologues under `"frame-pointer"="non-leaf"` are
/// `stp x29, x30, [sp, #-16]!; mov x29, sp`, so every frame's x29 points at
/// a `[caller x29, return address]` pair. One hop is therefore two loads,
/// against a full unwind step (compact-unwind lookup plus register
/// recovery) — this is what turns the measured 350:1 frames-to-roots ratio
/// from a tax into noise.
///
/// Fail-closed everywhere: a misaligned, non-increasing, or out-of-bounds
/// frame pointer abandons the walk with `None` and the caller re-runs the
/// whole scan through the platform unwinder. Slot visitation is idempotent
/// (a rewritten slot no longer points at a forwarded object), so a partial
/// fast walk followed by a full unwinder walk is safe.
#[cfg(all(any(target_os = "macos", target_os = "linux"), target_arch = "aarch64"))]
mod fp_chain {
    use super::*;

    fn current_frame_pointer() -> usize {
        let fp: usize;
        unsafe {
            core::arch::asm!("mov {fp}, x29", fp = out(reg) fp, options(nomem, nostack));
        }
        fp
    }

    #[cfg(target_os = "macos")]
    fn stack_top() -> usize {
        unsafe extern "C" {
            fn pthread_self() -> usize;
            fn pthread_get_stackaddr_np(thread: usize) -> *mut c_void;
        }
        unsafe { pthread_get_stackaddr_np(pthread_self()) as usize }
    }

    /// Linux (#7173): stack bounds via pthread attrs — the returned address
    /// is the LOW end, so the exclusive top is addr + size. Runtime gates
    /// pending a Linux host; a failure here returns 0 and the caller falls
    /// back to the platform unwinder (fail-closed like every other anomaly).
    #[cfg(target_os = "linux")]
    fn stack_top() -> usize {
        unsafe extern "C" {
            fn pthread_self() -> usize;
            fn pthread_getattr_np(thread: usize, attr: *mut u8) -> i32;
            fn pthread_attr_getstack(
                attr: *const u8,
                stackaddr: *mut *mut c_void,
                stacksize: *mut usize,
            ) -> i32;
            fn pthread_attr_destroy(attr: *mut u8) -> i32;
        }
        // pthread_attr_t is at most 64 bytes on glibc/musl for the supported
        // targets; over-allocate defensively.
        let mut attr = [0u8; 128];
        let mut addr: *mut c_void = std::ptr::null_mut();
        let mut size: usize = 0;
        unsafe {
            if pthread_getattr_np(pthread_self(), attr.as_mut_ptr()) != 0 {
                return 0;
            }
            let ok = pthread_attr_getstack(attr.as_ptr(), &mut addr, &mut size) == 0;
            pthread_attr_destroy(attr.as_mut_ptr());
            if !ok {
                return 0;
            }
        }
        (addr as usize).saturating_add(size)
    }

    pub(super) fn visit<F: FnMut(MutableRootSlot)>(
        index: &StackMapIndex,
        visit: &mut F,
    ) -> Option<NativeStackWalkStats> {
        if !index.chain_walkable {
            return None;
        }
        let top = stack_top();
        if top == 0 {
            return None;
        }
        let mut stats = NativeStackWalkStats {
            walks: 1,
            fp_walks: 1,
            ..NativeStackWalkStats::default()
        };
        let low_pc = index.min_pc.saturating_sub(MAX_SAFEPOINT_RETURN_DELTA);
        let high_pc = index.max_pc.saturating_add(MAX_SAFEPOINT_RETURN_DELTA);
        let mut fp = current_frame_pointer();
        while fp != 0 {
            if fp & 0xF != 0 || fp.checked_add(16)? > top {
                return None;
            }
            let return_address = unsafe { *((fp + 8) as *const usize) };
            let caller_fp = unsafe { *(fp as *const usize) };
            stats.frames_visited = stats.frames_visited.saturating_add(1);
            if return_address == 0 {
                break;
            }
            if return_address >= low_pc && return_address <= high_pc {
                let matched = index.match_records(return_address);
                {
                    if !matched.is_empty() {
                        // The record describes the caller's frame; its
                        // locations are relative to the caller's own x29,
                        // which is exactly the saved word we just read.
                        if caller_fp == 0 {
                            return None;
                        }
                        stats.records_matched = stats.records_matched.saturating_add(matched.len());
                        for record in matched {
                            // Body SP = fp - (prologue's `add x29, sp, #imm`).
                            // `chain_walkable` proved this decodes for every
                            // SP-relative record in the image (#7173).
                            let sp = fp_to_sp_offset(record.function_address)
                                .and_then(|off| caller_fp.checked_sub(off));
                            for location in &record.locations {
                                stats.locations_visited = stats.locations_visited.saturating_add(1);
                                let base = if location.dwarf_reg == DWARF_REG_FP_AARCH64 {
                                    Some(caller_fp)
                                } else {
                                    sp
                                };
                                let Some(base) = base else {
                                    return None;
                                };
                                let address = if location.offset < 0 {
                                    base.checked_sub(location.offset.unsigned_abs() as usize)
                                } else {
                                    base.checked_add(location.offset as usize)
                                };
                                let Some(address) = address else {
                                    continue;
                                };
                                if address == 0 || address & (std::mem::align_of::<u64>() - 1) != 0
                                {
                                    continue;
                                }
                                visit(MutableRootSlot {
                                    kind: MutableRootSlotKind::NativeStack,
                                    ptr: address as *mut u64,
                                });
                            }
                        }
                    }
                }
            }
            if caller_fp != 0 && caller_fp <= fp {
                return None;
            }
            fp = caller_fp;
        }
        Some(stats)
    }
}

#[cfg(not(all(any(target_os = "macos", target_os = "linux"), target_arch = "aarch64")))]
mod fp_chain {
    use super::*;

    pub(super) fn visit(
        _index: &StackMapIndex,
        _visit: &mut impl FnMut(MutableRootSlot),
    ) -> Option<NativeStackWalkStats> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_map_with_locations(
        function: u64,
        id: u64,
        offset: u32,
        locations: &[(u8, i32)],
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[STACK_MAP_VERSION, 0, 0, 0]);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&function.to_le_bytes());
        bytes.extend_from_slice(&32u64.to_le_bytes());
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&id.to_le_bytes());
        bytes.extend_from_slice(&offset.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&(locations.len() as u16).to_le_bytes());
        for (kind, frame_offset) in locations {
            bytes.push(*kind);
            bytes.push(0);
            bytes.extend_from_slice(&8u16.to_le_bytes());
            bytes.extend_from_slice(&29u16.to_le_bytes());
            bytes.extend_from_slice(&0u16.to_le_bytes());
            bytes.extend_from_slice(&frame_offset.to_le_bytes());
        }
        while bytes.len() % 8 != 0 {
            bytes.push(0);
        }
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        while bytes.len() % 8 != 0 {
            bytes.push(0);
        }
        bytes
    }

    fn one_map(function: u64, id: u64, offset: u32, frame_offset: i32) -> Vec<u8> {
        one_map_with_locations(function, id, offset, &[(LOCATION_DIRECT, frame_offset)])
    }

    #[test]
    fn parses_direct_mutable_frame_location() {
        let bytes = one_map(0x1000, 42, 0x10, -8);
        let (records, consumed) = parse_one_stack_map(&bytes).expect("valid stack map");
        assert_eq!(consumed, bytes.len());
        assert_eq!(
            records,
            vec![StackMapRecord {
                pc: 0x1010,
                function_address: 0x1000,
                stack_size: 32,
                locations: vec![StackMapLocation {
                    dwarf_reg: 29,
                    offset: -8,
                }],
            }]
        );
    }

    #[test]
    fn parses_linker_concatenated_input_sections() {
        let mut bytes = one_map(0x1000, 42, 0x10, -8);
        bytes.extend_from_slice(&one_map(0x2000, 43, 0x20, -16));
        let records = parse_concatenated_stack_maps(&bytes).expect("concatenated maps");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].pc, 0x1010);
        assert_eq!(records[1].pc, 0x2020);
    }

    #[test]
    fn parses_and_deduplicates_statepoint_spill_locations() {
        let bytes = one_map_with_locations(
            0x1000,
            7,
            0x20,
            &[(LOCATION_INDIRECT, -16), (LOCATION_INDIRECT, -16)],
        );
        let (records, consumed) = parse_one_stack_map(&bytes).expect("valid statepoint map");
        assert_eq!(consumed, bytes.len());
        assert_eq!(
            records,
            vec![StackMapRecord {
                pc: 0x1020,
                function_address: 0x1000,
                stack_size: 32,
                locations: vec![StackMapLocation {
                    dwarf_reg: 29,
                    offset: -16,
                }],
            }]
        );
    }

    #[test]
    fn rejects_truncated_or_wrong_version_sections() {
        assert!(parse_one_stack_map(&[]).is_none());
        let mut bytes = one_map(0x1000, 42, 0x10, -8);
        bytes[0] = 2;
        assert!(parse_one_stack_map(&bytes).is_none());
        bytes[0] = STACK_MAP_VERSION;
        bytes.truncate(bytes.len() - 1);
        assert!(parse_one_stack_map(&bytes).is_none());
    }

    #[test]
    fn chain_walkable_index_accepts_fp_and_sp_locations_only() {
        let rec = |pc: usize, reg: u16| StackMapRecord {
            pc,
            function_address: pc,
            stack_size: 160,
            locations: vec![StackMapLocation {
                dwarf_reg: reg,
                offset: -8,
            }],
        };
        // FP and SP are both walkable: SP resolves per frame by decoding the
        // owning function's prologue (#7173).
        let walkable = index_records(vec![
            rec(0x1000, DWARF_REG_FP_AARCH64),
            rec(0x2000, DWARF_REG_SP_AARCH64),
        ]);
        assert!(walkable.chain_walkable);
        assert_eq!(walkable.min_pc, 0x1000);
        assert_eq!(walkable.max_pc, 0x2000);
        // Any other register disqualifies the whole image.
        assert!(
            !index_records(vec![rec(0x1000, DWARF_REG_FP_AARCH64), rec(0x3000, 1)]).chain_walkable,
            "a non-FP/SP register must disable the fast walk"
        );
    }

    #[test]
    fn matches_plain_maps_before_and_statepoints_after_unwinder_ips() {
        let maps = vec![
            StackMapRecord {
                pc: 0x1000,
                function_address: 0x1000,
                stack_size: 32,
                locations: Vec::new(),
            },
            StackMapRecord {
                pc: 0x1020,
                function_address: 0x1020,
                stack_size: 32,
                locations: Vec::new(),
            },
        ];
        assert_eq!(closest_record_pc(&maps, 0x1004), Some(0x1000));
        assert_eq!(closest_record_pc(&maps, 0x101c), Some(0x1020));
        assert_eq!(closest_record_pc(&maps, 0x1020), Some(0x1020));
    }
}
