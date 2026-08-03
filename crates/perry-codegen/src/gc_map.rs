//! Re-encode LLVM's stack-map section into Perry's compact GC map.
//!
//! # Why this exists
//!
//! `gc.statepoint` metadata is the statepoint backend's *only* losing axis
//! against the shadow stack. Measured on `test-drizzle-pg`: generated `__text`
//! is 248 KB **smaller** under statepoints, but `__llvm_stackmaps` adds 3.9 MB,
//! so the binary loses by 3.5 MB overall.
//!
//! Almost none of those bytes carry information an AOT collector can use.
//! Measured composition of that section:
//!
//! * **60% of all location slots are `Constant`** — exactly three per record,
//!   `gc.statepoint`'s calling-convention / flags / num-deopt preamble.
//! * every root is recorded as a **(base, derived) pair**, and Perry has no
//!   interior pointers, so half of the remainder is the same slot twice;
//! * each record carries a 16-byte header whose 8-byte **patchpoint ID** only
//!   matters to a JIT that patches call sites, plus inter-record padding.
//!
//! The runtime already threw all of that away at startup (see
//! `perry-runtime/src/gc/roots/stack_maps.rs`): it kept `{dwarf_reg, offset}`
//! per distinct root and nothing else. This module simply stops shipping what
//! was always discarded.
//!
//! # Where the remaining win comes from
//!
//! Dropping the dead weight alone is ~11x. Two further facts about real
//! programs take it to ~32x:
//!
//! * roots within a record cluster in the frame, so **sorting by frame offset
//!   and delta-encoding** them makes most roots a single byte;
//! * **77% of records have exactly the live set of the record before them** —
//!   consecutive safepoints in a function usually share their roots — so a
//!   repeat flag replaces the whole payload.
//!
//! On drizzle: 4,214,384 B -> 131,402 B, which turns the 3.5 MB file-size loss
//! into a ~271 KB win and lets the statepoint backend lead on size, speed and
//! RSS simultaneously.
//!
//! # Why the rewrite happens on assembly rather than the object
//!
//! `clang -S` prints the stack map as ordinary directives with the function
//! addresses as **symbol names in plain text** (`.quad _main`). Rewriting there
//! needs one text parser. Rewriting the object instead would need Mach-O *and*
//! ELF relocation parsing (the addresses are external relocations), plus
//! `llvm-objcopy` to drop the old section, plus a second link pass.
//!
//! It costs almost nothing: `-S` takes the same time as `-c` (the codegen is
//! the cost; printing text is free), and assembling the result is ~0.02s.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{anyhow, Context, Result};

/// Magic at the start of every emitted blob.
pub const GC_MAP_MAGIC: &[u8; 4] = b"PGCM";
/// Format version. Bump on any layout change — the runtime rejects others.
pub const GC_MAP_VERSION: u8 = 2;
/// Section the compact map is emitted into, and the label it is given.
const GC_MAP_LABEL: &str = "_perry_gc_map";
const MACHO_SECTION: &str = "__PERRY_GCMAP,__perry_gcmap";
const ELF_SECTION: &str = ".perry_gcmap,\"a\",@progbits";

/// LLVM stack-map v3 location kinds. Only these two describe a frame slot;
/// `Constant`/`ConstIndex` carry the statepoint preamble and `Register` cannot
/// be recovered at collection time (which is what made plain stack maps
/// unsound — see the experiment write-up).
const LOCATION_DIRECT: u8 = 2;
const LOCATION_INDIRECT: u8 = 3;

/// One safepoint: where it is in its function, and which frame slots are live.
///
/// `instruction_offset` is the **assembly expression**, not a number: at `-O3`
/// LLVM emits it as a label difference (`Ltmp9-_main`) that only the assembler
/// can evaluate. That is why the emitted map stores offsets in a fixed-width
/// `u32` array rather than folding them into the varint stream.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Record {
    instruction_offset: String,
    /// `(dwarf_reg, frame_offset)`, deduplicated and sorted by frame offset.
    roots: Vec<(u16, i32)>,
}

/// One function's safepoints, keyed by the symbol the linker will relocate.
#[derive(Debug, Clone)]
struct FunctionMap {
    symbol: String,
    stack_size: u64,
    records: Vec<Record>,
}

/// Byte width contributed by each data directive LLVM emits in the block.
fn directive_width(directive: &str) -> Option<usize> {
    match directive {
        ".byte" => Some(1),
        ".short" | ".value" | ".hword" => Some(2),
        ".long" | ".word" => Some(4),
        ".quad" | ".xword" => Some(8),
        _ => None,
    }
}

/// The assembled bytes of the stack-map block, plus the byte offsets at which
/// a `.quad` referenced a symbol instead of a literal.
struct RawBlock {
    start_line: usize,
    end_line: usize,
    bytes: Vec<u8>,
    symbols: HashMap<usize, String>,
}

fn parse_block(lines: &[&str]) -> Option<RawBlock> {
    let start_line = lines.iter().position(|line| {
        let t = line.trim_start();
        t.starts_with(".section")
            && (t.contains("__LLVM_STACKMAPS") || t.contains(".llvm_stackmaps"))
    })?;

    let mut bytes: Vec<u8> = Vec::new();
    let mut symbols: HashMap<usize, String> = HashMap::new();
    let mut end_line = lines.len();

    for (index, raw) in lines.iter().enumerate().skip(start_line + 1) {
        let line = raw.trim();
        // The block runs to the next section or to the Mach-O epilogue.
        if line.starts_with(".section") || line.starts_with(".subsections_via_symbols") {
            end_line = index;
            break;
        }
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") || line.ends_with(':')
        {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let directive = parts.next().unwrap_or_default();
        let operand = parts.next().unwrap_or_default();
        let operand = operand
            .split('#')
            .next()
            .unwrap_or_default()
            .split("//")
            .next()
            .unwrap_or_default()
            .trim();

        // Alignment is real content: LLVM aligns every record, and skipping
        // the padding desynchronises every offset that follows it.
        if directive == ".p2align" || directive == ".align" {
            let first = operand.split(',').next().unwrap_or_default().trim();
            let value: u32 = first.parse().ok()?;
            let align = if directive == ".p2align" {
                1usize << value
            } else {
                value as usize
            };
            while align > 1 && bytes.len() % align != 0 {
                bytes.push(0);
            }
            continue;
        }

        let Some(width) = directive_width(directive) else {
            continue;
        };
        match parse_int(operand) {
            Some(value) => bytes.extend_from_slice(&value.to_le_bytes()[..width]),
            None => {
                // A symbolic operand. Two kinds appear: the `.quad` function
                // address, and — at `-O3` — the `.long` instruction offset as
                // a label difference. Remember the expression and reserve the
                // slot so every later structural offset stays correct.
                symbols.insert(bytes.len(), operand.to_string());
                bytes.extend_from_slice(&0u64.to_le_bytes()[..width]);
            }
        }
    }

    Some(RawBlock {
        start_line,
        end_line,
        bytes,
        symbols,
    })
}

fn parse_int(text: &str) -> Option<u64> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16).ok();
    }
    if let Some(negative) = text.strip_prefix('-') {
        return negative
            .parse::<u64>()
            .ok()
            .map(|v| (v as i64).wrapping_neg() as u64);
    }
    text.parse::<u64>().ok()
}

fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn read_u64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

fn align_up(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
}

/// Decode every concatenated v3 map in the block.
///
/// The section is a *sequence* of maps, one per object the linker saw — a
/// decoder that reads only the first header silently drops the rest, so this
/// walks until the bytes are consumed.
fn decode_v3(block: &RawBlock) -> Option<Vec<FunctionMap>> {
    let bytes = &block.bytes;
    let mut out: Vec<FunctionMap> = Vec::new();
    let mut pos = 0usize;

    while pos + 16 <= bytes.len() {
        if bytes[pos] != 3 {
            // Inter-map alignment padding.
            pos += 1;
            continue;
        }
        let function_count = read_u32(bytes, pos + 4)? as usize;
        let constant_count = read_u32(bytes, pos + 8)? as usize;
        let record_count = read_u32(bytes, pos + 12)? as usize;
        pos += 16;

        let mut heads = Vec::with_capacity(function_count);
        let mut expected = 0usize;
        for _ in 0..function_count {
            let symbol = block.symbols.get(&pos)?.clone();
            let stack_size = read_u64(bytes, pos + 8)?;
            let records = read_u64(bytes, pos + 16)? as usize;
            expected = expected.checked_add(records)?;
            heads.push((symbol, stack_size, records));
            pos += 24;
        }
        if expected != record_count {
            return None;
        }
        pos = pos.checked_add(constant_count.checked_mul(8)?)?;

        for (symbol, stack_size, count) in heads {
            let mut records = Vec::with_capacity(count);
            for _ in 0..count {
                let record_start = pos;
                let instruction_offset = block
                    .symbols
                    .get(&(pos + 8))
                    .cloned()
                    .unwrap_or_else(|| read_u32(bytes, pos + 8).unwrap_or(0).to_string());
                let location_count = read_u16(bytes, pos + 14)? as usize;
                pos += 16;

                let mut roots: Vec<(u16, i32)> = Vec::new();
                for _ in 0..location_count {
                    let kind = *bytes.get(pos)?;
                    let size = read_u16(bytes, pos + 2)?;
                    let dwarf_reg = read_u16(bytes, pos + 4)?;
                    let offset = read_u32(bytes, pos + 8)? as i32;
                    // Keep exactly what the collector keeps: 8-byte frame
                    // slots, with the base/derived pair collapsed to one.
                    if matches!(kind, LOCATION_DIRECT | LOCATION_INDIRECT) && size == 8 {
                        // The encoding stores the base as a single bit,
                        // FP-or-SP, using this architecture's DWARF numbers.
                        // Refuse anything else rather than silently rewriting
                        // it to FP: on x86-64 LLVM emits RBP=6/RSP=7, which
                        // would encode as "not SP" and decode back as
                        // aarch64's FP=29 — a wrong base, and a wrong base is
                        // a wrong root address. Bailing keeps LLVM's section,
                        // which costs bytes rather than correctness. (The
                        // native-frame-root backend is aarch64-only today; the
                        // runtime's prologue decoder and fast walker are both
                        // `cfg(target_arch = "aarch64")`.)
                        if dwarf_reg != DWARF_REG_FP_AARCH64 && dwarf_reg != DWARF_REG_SP_AARCH64 {
                            return None;
                        }
                        if !roots.contains(&(dwarf_reg, offset)) {
                            roots.push((dwarf_reg, offset));
                        }
                    }
                    pos += 12;
                }

                pos = align_up(pos - record_start, 8) + record_start;
                let live_out_count = read_u16(bytes, pos + 2)? as usize;
                pos = pos
                    .checked_add(4)?
                    .checked_add(live_out_count.checked_mul(4)?)?;
                pos = align_up(pos - record_start, 8) + record_start;
                if pos > bytes.len() {
                    return None;
                }

                roots.sort_unstable_by_key(|(_, offset)| *offset);
                records.push(Record {
                    instruction_offset,
                    roots,
                });
            }
            out.push(FunctionMap {
                symbol,
                stack_size,
                records,
            });
        }
    }

    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn push_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7F) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn zigzag(value: i32) -> u64 {
    ((value << 1) ^ (value >> 31)) as u32 as u64
}

/// DWARF register number for the stack pointer on aarch64; every other base
/// this backend emits is the frame pointer.
const DWARF_REG_SP_AARCH64: u16 = 31;
/// Frame pointer, the other base the single-bit encoding can express.
const DWARF_REG_FP_AARCH64: u16 = 29;

fn encode_stream(functions: &[FunctionMap]) -> Vec<u8> {
    let mut stream = Vec::new();
    for function in functions {
        let mut previous_roots: Option<&Vec<(u16, i32)>> = None;
        for record in &function.records {
            if previous_roots == Some(&record.roots) {
                // Repeat flag: the live set is the previous record's.
                push_varint(&mut stream, 1);
                continue;
            }
            push_varint(&mut stream, (record.roots.len() as u64) << 1);

            // Deltas are zigzagged rather than emitted raw. `decode_v3` sorts
            // roots so they are non-negative in practice, but a raw negative
            // delta sign-extends into a 10-byte varint and silently bloats the
            // map — the format must not depend on an ordering invariant held
            // somewhere else.
            let mut previous: Option<i32> = None;
            for (reg, offset) in &record.roots {
                let base_bit = u64::from(*reg == DWARF_REG_SP_AARCH64);
                let delta = match previous {
                    None => *offset,
                    Some(prev) => offset.wrapping_sub(prev),
                };
                push_varint(&mut stream, (zigzag(delta) << 1) | base_bit);
                previous = Some(*offset);
            }
            previous_roots = Some(&record.roots);
        }
    }
    stream
}

/// Assemble the emitted directives for one compact blob.
///
/// Layout (little-endian), mirrored by the runtime decoder:
///
/// ```text
///   0  "PGCM"
///   4  u8 version, u8 reserved, u16 reserved
///   8  u32 function_count
///  12  u32 total_len          -- lets the runtime walk concatenated blobs
///  16  function_count x { u64 address, u32 stack_size, u32 record_count }
///      record_count_total x u32 instruction_offset
///      varint root stream (see `encode_stream`)
/// ```
///
/// The function table starts at 16 so every relocated address is 8-byte
/// aligned, and the offset array that follows it is 4-byte aligned.
///
/// Instruction offsets are a fixed-width array rather than part of the varint
/// stream because at `-O3` they are **label differences the assembler
/// evaluates** (`Ltmp9-_main`), so their values do not exist at rewrite time.
/// That costs ~4 bytes per record — 18.7x compaction instead of 31.8x — and
/// buys not having to assemble twice just to learn numbers the assembler is
/// about to compute anyway.
fn emit_asm(functions: &[FunctionMap], stream: &[u8], elf: bool) -> String {
    let record_total: usize = functions.iter().map(|f| f.records.len()).sum();
    let total_len = 16 + functions.len() * 16 + record_total * 4 + stream.len();
    let mut out = String::new();
    if elf {
        out.push_str(&format!("\t.section\t{ELF_SECTION}\n"));
    } else {
        out.push_str(&format!("\t.section\t{MACHO_SECTION}\n"));
    }
    out.push_str("\t.p2align\t3\n");
    out.push_str(&format!("{GC_MAP_LABEL}:\n"));
    out.push_str("\t.ascii\t\"PGCM\"\n");
    out.push_str(&format!("\t.byte\t{GC_MAP_VERSION}\n"));
    out.push_str("\t.byte\t0\n");
    out.push_str("\t.short\t0\n");
    out.push_str(&format!("\t.long\t{}\n", functions.len()));
    out.push_str(&format!("\t.long\t{total_len}\n"));
    for function in functions {
        out.push_str(&format!("\t.quad\t{}\n", function.symbol));
        out.push_str(&format!("\t.long\t{}\n", function.stack_size as u32));
        out.push_str(&format!("\t.long\t{}\n", function.records.len()));
    }
    for function in functions {
        for record in &function.records {
            out.push_str(&format!("\t.long\t{}\n", record.instruction_offset));
        }
    }
    for chunk in stream.chunks(32) {
        let bytes: Vec<String> = chunk.iter().map(|b| b.to_string()).collect();
        out.push_str(&format!("\t.byte\t{}\n", bytes.join(",")));
    }
    out
}

/// Statistics for the caller to log — a compaction that silently did nothing
/// must be distinguishable from one that ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GcMapStats {
    pub original_bytes: usize,
    pub compact_bytes: usize,
    pub functions: usize,
    pub records: usize,
    pub roots: usize,
}

/// Rewrite the LLVM stack-map block in `asm` into the compact map.
///
/// Returns `None` when there is no stack-map block to rewrite (the common case
/// for a module without safepoints) or when the block does not parse — a
/// module whose metadata we do not fully understand keeps LLVM's section
/// rather than shipping a map that might be missing roots.
pub fn compact_stack_map_asm(asm: &str, elf: bool) -> Option<(String, GcMapStats)> {
    let lines: Vec<&str> = asm.lines().collect();
    let block = parse_block(&lines)?;
    let functions = decode_v3(&block)?;
    let stream = encode_stream(&functions);

    let stats = GcMapStats {
        original_bytes: block.bytes.len(),
        compact_bytes: 16
            + functions.len() * 16
            + functions.iter().map(|f| f.records.len()).sum::<usize>() * 4
            + stream.len(),
        functions: functions.len(),
        records: functions.iter().map(|f| f.records.len()).sum(),
        roots: functions
            .iter()
            .flat_map(|f| f.records.iter())
            .map(|r| r.roots.len())
            .sum(),
    };

    let replacement = emit_asm(&functions, &stream, elf);
    let mut out = String::with_capacity(asm.len());
    for line in &lines[..block.start_line] {
        // `.no_dead_strip` names the block's label from outside it. It is also
        // the only thing keeping a section nothing references from being
        // discarded, so retarget it instead of dropping it — without it the
        // map is stripped and the collector finds no roots at all.
        if line.contains(".no_dead_strip") && line.contains("__LLVM_StackMaps") {
            out.push_str(&format!("\t.no_dead_strip\t{GC_MAP_LABEL}\n"));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str(&replacement);
    for line in &lines[block.end_line..] {
        if line.contains("__LLVM_StackMaps") {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    Some((out, stats))
}

/// Rewrite the stack map in `asm_path` into Perry's compact form, then
/// assemble it to `obj_path`.
///
/// A module with no stack-map block is assembled unchanged — there is nothing
/// to compact.
///
/// A module that HAS a block which does not parse is a hard error, not a
/// fallback. Keeping LLVM's section there looks conservative and is not: the
/// runtime reads only `__perry_gcmap`, so that module's records would be
/// present in the binary, unread, and its roots invisible to the collector —
/// while other modules still emit a valid section, so even the "section
/// present but undecodable" guard in the runtime stays quiet. Silent lost
/// roots are precisely what this backend exists to make impossible.
pub fn compact_and_assemble(
    clang: &Path,
    target: &str,
    asm_path: &Path,
    obj_path: &Path,
) -> Result<()> {
    let asm = fs::read_to_string(asm_path)
        .with_context(|| format!("Failed to read assembly at {}", asm_path.display()))?;

    // Only the two object formats whose section syntax this module emits, and
    // whose section the runtime knows how to find, may be rewritten. Anything
    // else (COFF today) keeps LLVM's section: emitting a Mach-O `.section`
    // directive into COFF assembly would fail to assemble, turning an
    // unsupported-platform case into a broken build.
    let macho = target.contains("apple") || target.contains("darwin");
    let elf = !macho && !target.contains("windows") && !target.contains("msvc");
    if !macho && !elf {
        return assemble(clang, target, asm_path, obj_path);
    }

    let has_block = asm.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with(".section")
            && (t.contains("__LLVM_STACKMAPS") || t.contains(".llvm_stackmaps"))
    });
    let compacted = compact_stack_map_asm(&asm, elf);
    if has_block && compacted.is_none() {
        return Err(anyhow!(
            "perry: this module emits an LLVM stack map that the compact-map \
             rewriter could not parse, so its GC roots would be invisible to \
             the collector (the runtime reads only the compact section). \
             Refusing to emit a binary that would lose roots silently. \
             Assembly left at: {}",
            asm_path.display()
        ));
    }
    if let Some((rewritten, stats)) = compacted {
        fs::write(asm_path, rewritten).with_context(|| {
            format!(
                "Failed to write compacted assembly at {}",
                asm_path.display()
            )
        })?;
        GC_MAP_ORIGINAL_BYTES.fetch_add(stats.original_bytes as u64, Ordering::Relaxed);
        GC_MAP_COMPACT_BYTES.fetch_add(stats.compact_bytes as u64, Ordering::Relaxed);
        log::debug!(
            "perry-codegen: gc map {} -> {} bytes ({} functions, {} records, {} roots)",
            stats.original_bytes,
            stats.compact_bytes,
            stats.functions,
            stats.records,
            stats.roots,
        );
    }

    assemble(clang, target, asm_path, obj_path)
}

fn assemble(clang: &Path, target: &str, asm_path: &Path, obj_path: &Path) -> Result<()> {
    let output = Command::new(clang)
        .arg("-c")
        .arg(asm_path)
        .arg("-o")
        .arg(obj_path)
        .arg("-target")
        .arg(target)
        .output()
        .with_context(|| format!("Failed to invoke {}", clang.display()))?;
    if !output.status.success() {
        return Err(anyhow!(
            "assembling the compacted stack map failed (status={}).\n\
             assembly left at: {}\n\
             \n\
             stderr:\n{}",
            output.status,
            asm_path.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let _ = fs::remove_file(asm_path);
    Ok(())
}

/// Totals for the whole process, so a build can report what compaction did.
/// A run where these stay zero did not compact anything — the distinction a
/// gate needs in order to be able to fail.
static GC_MAP_ORIGINAL_BYTES: AtomicU64 = AtomicU64::new(0);
static GC_MAP_COMPACT_BYTES: AtomicU64 = AtomicU64::new(0);

/// `(llvm_bytes, compact_bytes)` summed across every module compiled so far.
pub fn gc_map_compaction_totals() -> (u64, u64) {
    (
        GC_MAP_ORIGINAL_BYTES.load(Ordering::Relaxed),
        GC_MAP_COMPACT_BYTES.load(Ordering::Relaxed),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal but structurally real v3 block: one function, one
    /// record, two roots of which the second is the base/derived duplicate.
    fn sample_asm() -> String {
        let mut asm = String::new();
        asm.push_str("\t.no_dead_strip\t__LLVM_StackMaps\n");
        asm.push_str("\t.section\t__LLVM_STACKMAPS,__llvm_stackmaps\n");
        asm.push_str("__LLVM_StackMaps:\n");
        asm.push_str("\t.byte\t3\n\t.byte\t0\n\t.short\t0\n");
        asm.push_str("\t.long\t1\n"); // functions
        asm.push_str("\t.long\t0\n"); // constants
        asm.push_str("\t.long\t1\n"); // records
        asm.push_str("\t.quad\t_probe_fn\n");
        asm.push_str("\t.quad\t144\n"); // stack size
        asm.push_str("\t.quad\t1\n"); // record count
                                      // record: id, instruction offset, reserved, location count
        asm.push_str("\t.quad\t0\n");
        asm.push_str("\t.long\t64\n");
        asm.push_str("\t.short\t0\n");
        asm.push_str("\t.short\t4\n");
        // three statepoint preamble constants, then base/derived pair
        for _ in 0..3 {
            asm.push_str(
                "\t.byte\t4\n\t.byte\t0\n\t.short\t8\n\t.short\t0\n\t.short\t0\n\t.long\t0\n",
            );
        }
        asm.push_str(
            "\t.byte\t3\n\t.byte\t0\n\t.short\t8\n\t.short\t29\n\t.short\t0\n\t.long\t4294967272\n",
        );
        asm.push_str("\t.p2align\t3\n");
        asm.push_str("\t.short\t0\n\t.short\t0\n"); // live-out header
        asm.push_str("\t.p2align\t3\n");
        asm.push_str("\t.subsections_via_symbols\n");
        asm
    }

    #[test]
    fn compacts_and_keeps_only_real_roots() {
        let (out, stats) = compact_stack_map_asm(&sample_asm(), false).expect("block rewritten");
        assert_eq!(stats.functions, 1);
        assert_eq!(stats.records, 1);
        // Four locations in, one root out: three constants dropped.
        assert_eq!(stats.roots, 1);
        assert!(
            stats.compact_bytes < stats.original_bytes,
            "compact {} should beat original {}",
            stats.compact_bytes,
            stats.original_bytes
        );
        assert!(out.contains("_perry_gc_map:"));
        assert!(out.contains(".quad\t_probe_fn"));
        // The old section must be gone, and nothing may still name its label.
        assert!(!out.contains("__llvm_stackmaps"));
        assert!(!out.contains("__LLVM_StackMaps"));
        // The dead-strip guard must survive, retargeted.
        assert!(out.contains(".no_dead_strip\t_perry_gc_map"));
    }

    #[test]
    fn repeated_live_sets_cost_one_byte() {
        let shared = vec![(29u16, -24i32), (29, -32)];
        let functions = vec![FunctionMap {
            symbol: "_f".to_string(),
            stack_size: 64,
            records: vec![
                Record {
                    instruction_offset: "0".to_string(),
                    roots: shared.clone(),
                },
                Record {
                    instruction_offset: "8".to_string(),
                    roots: shared.clone(),
                },
                Record {
                    instruction_offset: "16".to_string(),
                    roots: shared,
                },
            ],
        }];
        let one_record = vec![FunctionMap {
            symbol: functions[0].symbol.clone(),
            stack_size: functions[0].stack_size,
            records: functions[0].records[..1].to_vec(),
        }];
        // Offsets live in their own fixed-width array now, so in the varint
        // stream the two extra records cost exactly one repeat byte each,
        // regardless of how many roots the shared live set holds.
        assert_eq!(
            encode_stream(&functions).len(),
            encode_stream(&one_record).len() + 2
        );
    }

    #[test]
    fn foreign_register_bases_keep_llvm_section() {
        // x86-64 records RBP=6 / RSP=7. The single-bit base encoding cannot
        // express those, and guessing would decode them back as aarch64's
        // FP=29 — a wrong base, therefore a wrong root address. Keeping
        // LLVM's section costs bytes; guessing costs correctness.
        let asm = sample_asm().replace(
            "\t.byte\t3\n\t.byte\t0\n\t.short\t8\n\t.short\t29\n",
            "\t.byte\t3\n\t.byte\t0\n\t.short\t8\n\t.short\t6\n",
        );
        assert!(
            compact_stack_map_asm(&asm, true).is_none(),
            "a non-FP/SP base must fall back rather than be re-encoded"
        );
    }

    #[test]
    fn no_stack_map_block_is_left_alone() {
        assert!(compact_stack_map_asm("\t.section\t__TEXT,__text\n\tret\n", false).is_none());
    }

    #[test]
    fn unparsable_block_keeps_llvm_section() {
        // Truncated header: better to ship LLVM's section than a map that may
        // be missing roots.
        let asm = "\t.section\t__LLVM_STACKMAPS,__llvm_stackmaps\n\t.byte\t3\n";
        assert!(compact_stack_map_asm(asm, false).is_none());
    }
}
