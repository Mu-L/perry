#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PERRY="${PERRY_BIN:-${PERRY:-$REPO_ROOT/target/release/perry}}"

if [[ ! -x "$PERRY" ]]; then
    PERRY="$REPO_ROOT/target/debug/perry"
fi
if [[ ! -x "$PERRY" ]]; then
    echo "SKIP: perry binary not found (build with cargo build -p perry)"
    exit 0
fi

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

cat >"$TMPDIR/json_lazy_roundtrip_shape_gc.ts" <<'TS'
const items: any[] = [];
for (let i = 0; i < 10000; i++) {
  items.push({
    id: i,
    name: "item_" + i,
    value: i * 3.14159,
    tags: ["tag_" + (i % 10), "tag_" + (i % 5)],
    nested: { x: i, y: i * 2 }
  });
}

const blob = JSON.stringify(items);
if (blob.length !== 1064711) {
  throw new Error("unexpected source JSON length: " + blob.length);
}

for (let i = 0; i < 3; i++) {
  JSON.stringify(JSON.parse(blob));
}

let checksum = 0;
for (let iter = 0; iter < 4; iter++) {
  const parsed = JSON.parse(blob);
  if (parsed.length !== 10000) {
    throw new Error("parsed length changed at iter " + iter + ": " + parsed.length);
  }
  const out = JSON.stringify(parsed);
  if (out !== blob) {
    let first = -1;
    const minLen = out.length < blob.length ? out.length : blob.length;
    for (let j = 0; j < minLen; j++) {
      if (out.charCodeAt(j) !== blob.charCodeAt(j)) {
        first = j;
        break;
      }
    }
    throw new Error(
      "roundtrip mismatch at iter " + iter +
      " first=" + first +
      " expected=" + blob.slice(first - 40, first + 80) +
      " actual=" + out.slice(first - 40, first + 80)
    );
  }
  checksum += parsed.length + out.length;
}

if (checksum !== 4298844) {
  throw new Error("checksum mismatch: " + checksum);
}

console.log("json lazy roundtrip shape gc ok:" + checksum);
TS

"$PERRY" compile --no-cache "$TMPDIR/json_lazy_roundtrip_shape_gc.ts" \
    -o "$TMPDIR/json_lazy_roundtrip_shape_gc" >"$TMPDIR/compile.log" 2>&1 || {
    echo "FAIL: compile failed"
    sed 's/^/    /' "$TMPDIR/compile.log" | tail -80
    exit 1
}

"$TMPDIR/json_lazy_roundtrip_shape_gc" >"$TMPDIR/run.log" 2>&1 || {
    echo "FAIL: program failed"
    sed 's/^/    /' "$TMPDIR/run.log" | tail -80
    exit 1
}

if ! grep -q "json lazy roundtrip shape gc ok:4298844" "$TMPDIR/run.log"; then
    echo "FAIL: expected success marker"
    sed 's/^/    /' "$TMPDIR/run.log" | tail -80
    exit 1
fi

echo "PASS: JSON lazy roundtrip preserves homogeneous object shape under GC"
