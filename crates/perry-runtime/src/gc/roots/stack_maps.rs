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
    locations: Vec<StackMapLocation>,
}

static STACK_MAPS: OnceLock<Vec<StackMapRecord>> = OnceLock::new();

pub(in crate::gc) fn initialize() {
    let _ = stack_maps();
}

fn stack_maps() -> &'static [StackMapRecord] {
    STACK_MAPS
        .get_or_init(|| {
            let Some(section) = loaded_stack_map_section() else {
                return Vec::new();
            };
            let mut records = parse_concatenated_stack_maps(section).unwrap_or_default();
            records.sort_unstable_by_key(|record| record.pc);
            records
        })
        .as_slice()
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

pub(super) fn visit_stack_map_root_slots(visit: &mut impl FnMut(MutableRootSlot)) {
    let maps = stack_maps();
    if maps.is_empty() {
        return;
    }
    unwind::visit(maps, visit);
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
        let records = read_u64(bytes, offset + 16)? as usize;
        functions.push((address, records));
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
    for (function_address, function_record_count) in functions {
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

#[cfg(not(target_os = "macos"))]
fn loaded_stack_map_section() -> Option<&'static [u8]> {
    None
}

#[cfg(target_os = "macos")]
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
        maps: &'a [StackMapRecord],
        visit: &'a mut F,
    }

    pub(super) fn visit<F: FnMut(MutableRootSlot)>(maps: &[StackMapRecord], visit: &mut F) {
        let mut state = WalkState { maps, visit };
        unsafe {
            _Unwind_Backtrace(
                walk_frame::<F>,
                (&mut state as *mut WalkState<'_, _>).cast::<c_void>(),
            );
        }
    }

    unsafe extern "C" fn walk_frame<F: FnMut(MutableRootSlot)>(
        context: *mut UnwindContext,
        argument: *mut c_void,
    ) -> i32 {
        let state = &mut *argument.cast::<WalkState<'_, F>>();
        let ip = _Unwind_GetIP(context);
        let Some(candidate_pc) = closest_record_pc(state.maps, ip) else {
            return 0;
        };
        let delta = ip.abs_diff(candidate_pc);
        if delta > MAX_SAFEPOINT_RETURN_DELTA {
            return 0;
        }

        let first = state
            .maps
            .partition_point(|record| record.pc < candidate_pc);
        let last = state
            .maps
            .partition_point(|record| record.pc <= candidate_pc);
        for record in &state.maps[first..last] {
            for location in &record.locations {
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
                    // Reuse the compiled-frame telemetry bucket so the
                    // experiment compares root source counts directly.
                    kind: MutableRootSlotKind::ShadowStack,
                    ptr: address as *mut u64,
                });
            }
        }
        0
    }
}

#[cfg(not(target_os = "macos"))]
mod unwind {
    use super::*;

    pub(super) fn visit(_maps: &[StackMapRecord], _visit: &mut impl FnMut(MutableRootSlot)) {}
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
    fn matches_plain_maps_before_and_statepoints_after_unwinder_ips() {
        let maps = vec![
            StackMapRecord {
                pc: 0x1000,
                locations: Vec::new(),
            },
            StackMapRecord {
                pc: 0x1020,
                locations: Vec::new(),
            },
        ];
        assert_eq!(closest_record_pc(&maps, 0x1004), Some(0x1000));
        assert_eq!(closest_record_pc(&maps, 0x101c), Some(0x1020));
        assert_eq!(closest_record_pc(&maps, 0x1020), Some(0x1020));
    }
}
