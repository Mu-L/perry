#!/usr/bin/env python3
"""Break an LLVM stackmap v3 section down into where its bytes actually go.

Answers the only question that matters for file size: which structural
component dominates, and how much of it is redundant for a collector that
has no interior pointers.

Usage: stackmap_anatomy.py <binary>            # finds the section itself
       stackmap_anatomy.py --raw <section.bin>
"""
import struct
import subprocess
import sys

LOC_KIND = {1: "Register", 2: "Direct", 3: "Indirect", 4: "Constant", 5: "ConstIndex"}


def extract_section(path):
    """Pull __LLVM_STACKMAPS (Mach-O) or .llvm_stackmaps (ELF) out of a binary."""
    with open(path, "rb") as fh:
        head = fh.read(4)
    if head[:4] == b"\x7fELF":
        out = subprocess.run(
            ["readelf", "-x", ".llvm_stackmaps", path],
            capture_output=True, text=True).stdout
        data = bytearray()
        for line in out.splitlines():
            parts = line.split()
            if len(parts) >= 2 and parts[0].startswith("0x"):
                for word in parts[1:5]:
                    if all(c in "0123456789abcdefABCDEF" for c in word) and len(word) % 2 == 0:
                        data += bytes.fromhex(word)
        return bytes(data)
    # Mach-O: locate section by (segment, section) and slice the file.
    out = subprocess.run(["otool", "-l", path], capture_output=True, text=True).stdout
    lines = out.splitlines()
    for i, line in enumerate(lines):
        if "__llvm_stackmaps" in line:
            off = size = None
            for probe in lines[i:i + 14]:
                p = probe.split()
                if len(p) == 2 and p[0] == "offset":
                    off = int(p[1], 0)
                if len(p) == 2 and p[0] == "size":
                    size = int(p[1], 0)
            if off is not None and size is not None:
                with open(path, "rb") as fh:
                    fh.seek(off)
                    return fh.read(size)
    raise SystemExit(f"no stackmap section found in {path}")


def analyze(buf):
    ver, _, _ = struct.unpack_from("<BBH", buf, 0)
    nfunc, nconst, nrec = struct.unpack_from("<III", buf, 4)
    pos = 16
    stats = {
        "header": 16,
        "func_records": nfunc * 24,
        "constants": nconst * 8,
        "record_headers": 0,
        "locations": 0,
        "liveouts": 0,
        "padding": 0,
    }
    pos += nfunc * 24 + nconst * 8

    kinds = {}
    n_locs = 0
    n_liveouts = 0
    locs_per_rec = []
    # A collector without interior pointers records base==derived; count how
    # many location slots are exact duplicates of an earlier slot in the
    # same record (that is the redundancy an AOT format would not pay for).
    dup_locs = 0
    const_locs = 0

    for _ in range(nrec):
        rec_start = pos
        _pid, _off, _res, nloc = struct.unpack_from("<QIHH", buf, pos)
        pos += 16
        stats["record_headers"] += 16
        seen = set()
        for _ in range(nloc):
            kind, _r, _sz, reg, _r2, offv = struct.unpack_from("<BBHHHi", buf, pos)
            pos += 12
            n_locs += 1
            kinds[LOC_KIND.get(kind, kind)] = kinds.get(LOC_KIND.get(kind, kind), 0) + 1
            if kind in (4, 5):
                const_locs += 1
            key = (kind, reg, offv)
            if key in seen:
                dup_locs += 1
            seen.add(key)
        stats["locations"] += nloc * 12
        locs_per_rec.append(nloc)
        if (pos - rec_start) % 8:
            pad = 8 - ((pos - rec_start) % 8)
            pos += pad
            stats["padding"] += pad
        (nlive,) = struct.unpack_from("<H", buf, pos)
        pos += 2
        pos += nlive * 4
        n_liveouts += nlive
        stats["liveouts"] += 2 + nlive * 4
        if (pos - rec_start) % 8:
            pad = 8 - ((pos - rec_start) % 8)
            pos += pad
            stats["padding"] += pad

    total = len(buf)
    print(f"stackmap v{ver}  total {total:,} B")
    print(f"  functions {nfunc:,}   constants {nconst:,}   records {nrec:,}")
    print(f"  locations {n_locs:,}  ({n_locs / max(nrec,1):.1f} per record)"
          f"   liveouts {n_liveouts:,}")
    print()
    print("  byte breakdown:")
    for key, val in sorted(stats.items(), key=lambda kv: -kv[1]):
        print(f"    {key:<16} {val:>12,}  {100.0 * val / total:5.1f}%")
    print()
    print("  location kinds:")
    for key, val in sorted(kinds.items(), key=lambda kv: -kv[1]):
        print(f"    {str(key):<16} {val:>12,}  {100.0 * val / max(n_locs,1):5.1f}%")
    print()
    print(f"  duplicate location slots within a record: {dup_locs:,} "
          f"({100.0 * dup_locs / max(n_locs,1):.1f}% of locations, "
          f"{dup_locs * 12:,} B = {100.0 * dup_locs * 12 / total:.1f}% of section)")
    print(f"  constant-kind locations: {const_locs:,} "
          f"({const_locs * 12:,} B = {100.0 * const_locs * 12 / total:.1f}% of section)")

    return kinds


def varint_len(value):
    """Bytes a LEB128 encoding of `value` occupies.

    Rejects negatives rather than returning 1 for them: Python ints are
    unbounded, so a negative slips straight past `>= 0x80` and every negative
    frame offset would be counted as a single byte, understating the very
    encoding this script claims to measure exactly.
    """
    if value < 0:
        raise ValueError(f"varint_len expects a non-negative value, got {value}")
    n = 1
    while value >= 0x80:
        value >>= 7
        n += 1
    return n


def compact_size(buf):
    """Exact byte count of the encoding the runtime would actually consume.

    The runtime already discards Constant locations and dedups the base/derived
    pair at parse time (`stack_maps.rs`), so it keeps only {dwarf_reg, offset}
    per distinct root. This measures shipping precisely that:

      header      16 B
      per function  16 B  (u64 relocated address, u32 stack size, u32 records)
      per record   varint(delta instruction offset) + varint(root count)
      per root     varint(zigzag(offset) << 1 | reg_is_sp)
    """
    _ver, _, _ = struct.unpack_from("<BBH", buf, 0)
    nfunc, nconst, nrec = struct.unpack_from("<III", buf, 4)
    pos = 16
    funcs = []
    for _ in range(nfunc):
        _addr, _ss, rc = struct.unpack_from("<QQQ", buf, pos)
        funcs.append(rc)
        pos += 24
    pos += nconst * 8

    size = 16 + nfunc * 16
    roots_kept = 0
    for rc in funcs:
        prev_off = 0
        for _ in range(rc):
            rec_start = pos
            _pid, instr_off, _res, nloc = struct.unpack_from("<QIHH", buf, pos)
            pos += 16
            size += varint_len((instr_off - prev_off) & 0xFFFFFFFF)
            prev_off = instr_off
            seen = []
            for _ in range(nloc):
                kind, _r, lsize, reg, _r2, offv = struct.unpack_from("<BBHHHi", buf, pos)
                pos += 12
                if kind in (2, 3) and lsize == 8:
                    key = (reg, offv)
                    if key not in seen:
                        seen.append(key)
            size += varint_len(len(seen))
            for reg, offv in seen:
                zig = ((offv << 1) ^ (offv >> 31)) & 0xFFFFFFFF
                size += varint_len((zig << 1) | (1 if reg == 31 else 0))
            roots_kept += len(seen)
            if (pos - rec_start) % 8:
                pos += 8 - ((pos - rec_start) % 8)
            (nlive,) = struct.unpack_from("<H", buf, pos)
            pos += 2 + nlive * 4
            if (pos - rec_start) % 8:
                pos += 8 - ((pos - rec_start) % 8)
    return size, roots_kept


def map_extent(buf, start):
    """Byte length of the single v3 map beginning at `start`.

    The linker concatenates one map per object file, so the section is a
    sequence of these, not a single map (`parse_concatenated_stack_maps`).
    """
    nfunc, nconst, nrec = struct.unpack_from("<III", buf, start + 4)
    pos = start + 16
    counts = []
    for _ in range(nfunc):
        counts.append(struct.unpack_from("<QQQ", buf, pos)[2])
        pos += 24
    pos += nconst * 8
    for _ in range(nrec):
        rec_start = pos
        nloc = struct.unpack_from("<QIHH", buf, pos)[3]
        pos += 16 + nloc * 12
        if (pos - rec_start) % 8:
            pos += 8 - ((pos - rec_start) % 8)
        (nlive,) = struct.unpack_from("<H", buf, pos)
        pos += 2 + nlive * 4
        if (pos - rec_start) % 8:
            pos += 8 - ((pos - rec_start) % 8)
    return pos - start


def split_maps(buf):
    """Every concatenated v3 map in the section, as byte slices."""
    maps = []
    pos = 0
    while pos + 16 <= len(buf):
        if buf[pos] != 3:
            # 8-byte alignment padding between maps.
            pos += 1
            continue
        extent = map_extent(buf, pos)
        if extent <= 0 or pos + extent > len(buf):
            break
        maps.append(buf[pos:pos + extent])
        pos += extent
        while pos % 8:
            pos += 1
    return maps


if __name__ == "__main__":
    args = sys.argv[1:]
    raw = open(args[1], "rb").read() if args and args[0] == "--raw" \
        else extract_section(args[0])
    maps = split_maps(raw)
    print(f"section {len(raw):,} B  =  {len(maps)} concatenated map(s)\n")
    merged = {"total": 0, "compact": 0, "roots": 0}
    for chunk in maps:
        merged["total"] += len(chunk)
        csize, croots = compact_size(chunk)
        # Only the first map pays a format header in the merged encoding.
        merged["compact"] += csize
        merged["roots"] += croots
    if len(maps) == 1:
        analyze(maps[0])
    else:
        # Aggregate composition across every map.
        agg = {}
        for chunk in maps:
            nfunc, nconst, nrec = struct.unpack_from("<III", chunk, 4)
            agg["functions"] = agg.get("functions", 0) + nfunc
            agg["records"] = agg.get("records", 0) + nrec
        print(f"  functions {agg['functions']:,}   records {agg['records']:,}")
    merged["compact"] -= 16 * (len(maps) - 1)
    print()
    print(f"  parsed coverage: {merged['total']:,} of {len(raw):,} B "
          f"({100.0 * merged['total'] / len(raw):.1f}%)")
    print(f"  compact AOT encoding: {merged['compact']:,} B for "
          f"{merged['roots']:,} distinct roots")
    print(f"  -> {len(raw) / max(merged['compact'],1):.1f}x smaller, "
          f"saves {len(raw) - merged['compact']:,} B")
