#!/usr/bin/env bash
# Build an exact-head #1090 GC evidence packet from clean detached worktrees.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BASE_REF="origin/main"
HEAD_REF="HEAD"
RUNS=5
OUT=""
SKIP_PERF_COMPREHENSIVE=0
KEEP_WORKTREES=0
GATE=0
HEAD_WORKTREE_SNAPSHOT=0
PERF_MATH_SLICE_ROWS=()
REQUIRED_BENCHMARK_TRACE_ROWS=(
  "bench_json_roundtrip"
  "bench_gc_pressure"
  "07_object_create"
  "12_binary_trees"
)

usage() {
  cat <<'EOF'
Usage: scripts/gc_1090_evidence_packet.sh [options]

Options:
  --base-ref REF                 Comparison base (default: origin/main)
  --head-ref REF                 Head/PR ref (default: HEAD)
  --runs N                       Benchmark samples per benchmark (default: 5)
  --out PATH                     Output root (default: tmp/gc-1090-evidence-<utc>)
  --gate                         Fail on missing strict evidence
  --head-worktree-snapshot       Test a synthetic commit made from the dirty current worktree
  --perf-math-slice-row NAME     Limit nested perf-frontier math slices
  --skip-perf-comprehensive      Skip optional perf-comprehensive probe
  --keep-worktrees               Keep detached worktrees after the run
  -h, --help                     Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --base-ref) BASE_REF="$2"; shift 2 ;;
    --head-ref) HEAD_REF="$2"; shift 2 ;;
    --runs) RUNS="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --gate) GATE=1; shift ;;
    --head-worktree-snapshot) HEAD_WORKTREE_SNAPSHOT=1; shift ;;
    --perf-math-slice-row) PERF_MATH_SLICE_ROWS+=("$2"); shift 2 ;;
    --skip-perf-comprehensive) SKIP_PERF_COMPREHENSIVE=1; shift ;;
    --keep-worktrees) KEEP_WORKTREES=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if ! [[ "$RUNS" =~ ^[0-9]+$ ]] || [[ "$RUNS" -lt 1 ]]; then
  echo "--runs must be a positive integer" >&2
  exit 2
fi

if [[ -z "$OUT" ]]; then
  OUT="tmp/gc-1090-evidence-$(date -u +%Y%m%dT%H%M%SZ)"
fi

cd "$ROOT"

BASE_SHA="$(git rev-parse --verify "$BASE_REF^{commit}")"
ORIGINAL_HEAD_SHA="$(git rev-parse --verify "$HEAD_REF^{commit}")"
HEAD_SHA="$ORIGINAL_HEAD_SHA"
if ! [[ "$BASE_SHA" =~ ^[0-9a-f]{40}$ ]]; then
  echo "base ref did not resolve to an exact 40-char SHA: $BASE_REF -> $BASE_SHA" >&2
  exit 2
fi
if ! [[ "$ORIGINAL_HEAD_SHA" =~ ^[0-9a-f]{40}$ ]]; then
  echo "head ref did not resolve to an exact 40-char SHA: $HEAD_REF -> $ORIGINAL_HEAD_SHA" >&2
  exit 2
fi
OUT_ABS="$(python3 - "$ROOT" "$OUT" <<'PY'
import os
import sys
root, out = sys.argv[1], sys.argv[2]
if not os.path.isabs(out):
    out = os.path.join(root, out)
print(os.path.abspath(out))
PY
)"
OUT_REL="$(python3 - "$ROOT" "$OUT_ABS" <<'PY'
import os
import sys
root, out = map(os.path.abspath, sys.argv[1:3])
try:
    rel = os.path.relpath(out, root)
except ValueError:
    raise SystemExit(1)
if rel.startswith(".."):
    raise SystemExit(1)
print(rel)
PY
)" || {
  echo "output path must be inside the repository: $OUT_ABS" >&2
  exit 2
}

if ! git check-ignore -q -- "$OUT_REL"; then
  echo "output path must be ignored by git: $OUT_REL" >&2
  exit 2
fi

if [[ -n "$(git ls-files -- "$OUT_REL" "$OUT_REL/**")" ]]; then
  echo "output path contains tracked files; choose a fresh ignored path: $OUT_REL" >&2
  exit 2
fi

if [[ "$GATE" -eq 1 && -d "$OUT_ABS" && -n "$(find "$OUT_ABS" -mindepth 1 -print -quit 2>/dev/null)" ]]; then
  echo "gated evidence output path must be empty; choose a fresh ignored path: $OUT_REL" >&2
  exit 2
fi

mkdir -p "$OUT_ABS"

BASE_WT="$OUT_ABS/worktrees/base"
HEAD_WT="$OUT_ABS/worktrees/head"
METADATA="$OUT_ABS/metadata.json"
SOURCE_STATUS="$OUT_ABS/source-status.txt"
SOURCE_DIFF="$OUT_ABS/source-diff.patch"
SOURCE_TESTED_HEAD_DIFF="$OUT_ABS/source-tested-head.patch"
SOURCE_STATE_MODE="exact-ref"
CURRENT_HEAD_SHA="$(git rev-parse --verify HEAD^{commit})"

sha256_file() {
  python3 - "$1" <<'PY'
import hashlib
import sys
from pathlib import Path

print(hashlib.sha256(Path(sys.argv[1]).read_bytes()).hexdigest())
PY
}

NODE_BIN_ENV="${NODE_BIN:-}"
NODE_RESOLVED_BIN=""
NODE_RESOLUTION_ERROR=""
NODE_CANDIDATES=()
NODE_RESOLUTION_TRIED=()

append_node_candidate() {
  local candidate="${1:-}"
  [[ -z "$candidate" ]] && return
  local existing
  for existing in "${NODE_CANDIDATES[@]}"; do
    [[ "$existing" == "$candidate" ]] && return
  done
  NODE_CANDIDATES+=("$candidate")
}

collect_node_candidates() {
  append_node_candidate "${PERRY_BENCH_NODE:-}"
  append_node_candidate "${PERRY_NODE_BIN:-}"
  append_node_candidate "$NODE_BIN_ENV"
  append_node_candidate "$ROOT/externals/node24/bin/node"
  append_node_candidate "$ROOT/externals/node20/bin/node"
  append_node_candidate "$ROOT/externals/node/bin/node"
  append_node_candidate "/home/github-runner/actions-runner/externals/node24/bin/node"
  append_node_candidate "/home/github-runner/actions-runner/externals/node20/bin/node"
  append_node_candidate "/tmp/perry-node25-bin/node"

  local path_node
  path_node="$(command -v node 2>/dev/null || true)"
  append_node_candidate "$path_node"
}

resolve_node_binary() {
  collect_node_candidates
  local node
  for node in "${NODE_CANDIDATES[@]}"; do
    NODE_RESOLUTION_TRIED+=("$node")
    [[ -x "$node" ]] || continue
    if "$node" --version >/dev/null 2>&1; then
      NODE_RESOLVED_BIN="$node"
      return 0
    fi
  done
  NODE_RESOLUTION_ERROR="no usable Node.js runtime found for benchmark references"
  return 1
}

git status --porcelain=v1 --untracked-files=all >"$SOURCE_STATUS"
if ! git diff --binary HEAD -- >"$SOURCE_DIFF"; then
  : >"$SOURCE_DIFF"
fi
SOURCE_DIFF_SHA256="$(sha256_file "$SOURCE_DIFF")"

create_head_worktree_snapshot() {
  if [[ "$ORIGINAL_HEAD_SHA" != "$CURRENT_HEAD_SHA" ]]; then
    echo "--head-worktree-snapshot requires --head-ref to resolve to the current HEAD" >&2
    echo "head ref: $HEAD_REF -> $ORIGINAL_HEAD_SHA" >&2
    echo "current HEAD: $CURRENT_HEAD_SHA" >&2
    exit 2
  fi

  local snapshot_index
  local snapshot_tree
  snapshot_index="$(mktemp "$OUT_ABS/head-worktree-index.XXXXXX")"
  GIT_INDEX_FILE="$snapshot_index" git read-tree "$CURRENT_HEAD_SHA"
  GIT_INDEX_FILE="$snapshot_index" git add -A -- .
  snapshot_tree="$(GIT_INDEX_FILE="$snapshot_index" git write-tree)"
  HEAD_SHA="$(
    printf 'GC evidence worktree snapshot for %s\n' "$CURRENT_HEAD_SHA" \
      | git commit-tree "$snapshot_tree" -p "$CURRENT_HEAD_SHA"
  )"
  rm -f "$snapshot_index"
  SOURCE_STATE_MODE="worktree-snapshot"
}

if [[ "$HEAD_WORKTREE_SNAPSHOT" -eq 1 ]]; then
  create_head_worktree_snapshot
elif [[ "$GATE" -eq 1 && "$ORIGINAL_HEAD_SHA" == "$CURRENT_HEAD_SHA" && -s "$SOURCE_STATUS" ]]; then
  echo "gated evidence would ignore dirty current worktree changes; use --head-worktree-snapshot or commit the work first" >&2
  exit 2
fi

resolve_node_binary || true

HEAD_TREE_SHA="$(git show -s --format=%T "$HEAD_SHA")"
if [[ "$SOURCE_STATE_MODE" == "worktree-snapshot" ]]; then
  if ! git diff --binary "$CURRENT_HEAD_SHA" "$HEAD_SHA" -- >"$SOURCE_TESTED_HEAD_DIFF"; then
    : >"$SOURCE_TESTED_HEAD_DIFF"
  fi
else
  if ! git diff --binary "$ORIGINAL_HEAD_SHA" "$HEAD_SHA" -- >"$SOURCE_TESTED_HEAD_DIFF"; then
    : >"$SOURCE_TESTED_HEAD_DIFF"
  fi
fi
SOURCE_TESTED_HEAD_DIFF_SHA256="$(sha256_file "$SOURCE_TESTED_HEAD_DIFF")"

cleanup() {
  if [[ "$KEEP_WORKTREES" -eq 0 ]]; then
    git worktree remove --force "$BASE_WT" >/dev/null 2>&1 || true
    git worktree remove --force "$HEAD_WT" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

write_metadata() {
  python3 - "$METADATA" "$BASE_REF" "$HEAD_REF" "$BASE_SHA" "$HEAD_SHA" "$RUNS" "$SKIP_PERF_COMPREHENSIVE" "$GATE" "$SOURCE_STATE_MODE" "$ORIGINAL_HEAD_SHA" "$CURRENT_HEAD_SHA" "$HEAD_TREE_SHA" "$SOURCE_STATUS" "$SOURCE_DIFF_SHA256" "$SOURCE_TESTED_HEAD_DIFF_SHA256" <<'PY'
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

path = Path(sys.argv[1])
existing = {}
if path.exists():
    existing = json.loads(path.read_text(encoding="utf-8"))
status_path = Path(sys.argv[13])
try:
    status_lines = status_path.read_text(encoding="utf-8", errors="replace").splitlines()
except Exception:
    status_lines = []
existing.update({
    "schema_version": 1,
    "generated_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "base_ref": sys.argv[2],
    "head_ref": sys.argv[3],
    "base_sha": sys.argv[4],
    "head_sha": sys.argv[5],
    "runs": int(sys.argv[6]),
    "skip_perf_comprehensive": sys.argv[7] == "1",
    "gate": sys.argv[8] == "1",
    "source_state": {
        "mode": sys.argv[9],
        "original_head_sha": sys.argv[10],
        "current_head_sha": sys.argv[11],
        "tested_head_sha": sys.argv[5],
        "tested_head_tree_sha": sys.argv[12],
        "dirty": bool(status_lines),
        "status_path": "source-status.txt",
        "status_porcelain_v1": status_lines,
        "tracked_diff_path": "source-diff.patch",
        "tracked_diff_sha256": sys.argv[14],
        "tested_head_diff_path": "source-tested-head.patch",
        "tested_head_diff_sha256": sys.argv[15],
    },
    "commands": {},
    "tool_versions": {},
})
path.parent.mkdir(parents=True, exist_ok=True)
path.write_text(json.dumps(existing, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY
}

capture_tool_versions() {
  local tried
  tried="$(printf '%s\n' "${NODE_RESOLUTION_TRIED[@]}")"
  NODE_RESOLUTION_TRIED_TEXT="$tried" python3 - "$METADATA" "$ROOT" "$NODE_RESOLVED_BIN" "$NODE_RESOLUTION_ERROR" <<'PY'
import json
import os
import platform
import subprocess
import sys
from pathlib import Path

metadata = Path(sys.argv[1])
root = Path(sys.argv[2])
node_bin = sys.argv[3]
node_error = sys.argv[4]
node_tried = [
    line
    for line in os.environ.get("NODE_RESOLUTION_TRIED_TEXT", "").splitlines()
    if line
]

def run(cmd):
    try:
        completed = subprocess.run(cmd, cwd=root, text=True, capture_output=True, timeout=15)
    except Exception as exc:
        return {"available": False, "error": str(exc)}
    return {
        "available": completed.returncode == 0,
        "exit_code": completed.returncode,
        "stdout": completed.stdout.strip().splitlines()[:3],
        "stderr": completed.stderr.strip().splitlines()[:3],
    }

data = json.loads(metadata.read_text(encoding="utf-8"))
data["tool_versions"] = {
    "platform": platform.platform(),
    "python": sys.version.split()[0],
    "git": run(["git", "--version"]),
    "cargo": run(["cargo", "--version"]),
    "rustc": run(["rustc", "--version"]),
    "node": run([node_bin, "--version"]) if node_bin else {
        "available": False,
        "error": node_error or "no Node candidate selected",
    },
    "node_resolver": {
        "path": node_bin or None,
        "tried": node_tried,
        "error": node_error or None,
    },
    "sample": run(["/usr/bin/sample", "-h"]) if Path("/usr/bin/sample").exists() else {"available": False},
    "perf": run(["perf", "--version"]),
}
metadata.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY
}

record_command() {
  local label="$1"
  local name="$2"
  local status="$3"
  local exit_code="$4"
  local log_path="${5:-}"
  local reason="${6:-}"
  python3 - "$METADATA" "$label" "$name" "$status" "$exit_code" "$log_path" "$reason" <<'PY'
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

path = Path(sys.argv[1])
label, name, status, exit_code, log_path, reason = sys.argv[2:8]
data = json.loads(path.read_text(encoding="utf-8"))
commands = data.setdefault("commands", {})
label_commands = commands.setdefault(label, {})
entry = {
    "status": status,
    "exit_code": int(exit_code),
    "finished_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
}
if log_path:
    entry["log"] = log_path
if reason:
    entry["reason"] = reason
label_commands[name] = entry
path.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY
}

write_metadata
capture_tool_versions

echo "=== #1090 GC evidence packet ==="
echo "base: $BASE_REF -> $BASE_SHA"
echo "head: $HEAD_REF -> $HEAD_SHA"
if [[ "$SOURCE_STATE_MODE" == "worktree-snapshot" ]]; then
  echo "head source: worktree snapshot of $ORIGINAL_HEAD_SHA"
fi
echo "out:  $OUT_ABS"

mkdir -p "$OUT_ABS/worktrees"
git worktree add --detach "$BASE_WT" "$BASE_SHA"
git worktree add --detach "$HEAD_WT" "$HEAD_SHA"

run_logged() {
  local label="$1"
  local name="$2"
  local worktree="$3"
  local log="$4"
  shift 4
  mkdir -p "$(dirname "$log")"
  echo "=== $label: $name ==="
  set +e
  (
    cd "$worktree"
    "$@"
  ) >"$log" 2>&1
  local code=$?
  set -e
  local status="pass"
  if [[ "$code" -ne 0 ]]; then
    status="fail"
  fi
  record_command "$label" "$name" "$status" "$code" "$log" ""
  echo "  $status (exit=$code, log=$log)"
  return 0
}

trace_has_gc_cycle() {
  local trace_file="$1"
  python3 - "$trace_file" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
try:
    handle = path.open("r", encoding="utf-8", errors="replace")
except OSError:
    raise SystemExit(1)

with handle:
    for line in handle:
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(event, dict) and event.get("event") == "gc_cycle":
            raise SystemExit(0)
raise SystemExit(1)
PY
}

strip_suite_types_to_mjs() {
  local src="$1"
  local dst="$2"
  sed -E \
    -e 's/: (number|string|boolean|any|void)(\[\])*//g' \
    -e 's/\): (number|string|boolean|any|void)(\[\])* \{/) {/g' \
    "$src" >"$dst"
}

precompile_node_reference() {
  local src="$1"
  local dst="$2"
  case "$NODE_TS_STRIP" in
    esbuild)
      esbuild "$src" --format=esm --platform=neutral --target=esnext \
        --outfile="$dst" --log-level=warning >/dev/null 2>&1
      ;;
    npx-esbuild)
      npx --no-install esbuild "$src" --format=esm --platform=neutral \
        --target=esnext --outfile="$dst" --log-level=warning >/dev/null 2>&1
      ;;
    tsc)
      local d b
      d=$(dirname "$dst")
      b=$(basename "$src" .ts)
      tsc --target esnext --module esnext --moduleResolution bundler \
        --outDir "$d" "$src" >/dev/null 2>&1 || return 1
      [[ -f "$d/$b.js" ]] || return 1
      mv "$d/$b.js" "$dst"
      ;;
    suite-sed)
      strip_suite_types_to_mjs "$src" "$dst"
      ;;
    *)
      return 1
      ;;
  esac
}

write_bench_gc_pressure_trace_source() {
  local dst="$1"
  cat >"$dst" <<'EOF'
// Generated by gc_1090_evidence_packet.sh for required benchmark GC tracing.
//
// The regular bench_gc_pressure benchmark permits scalar replacement so it
// keeps measuring the fast allocation path. GC evidence needs real escaping
// nursery pressure, so this generated variant retains a bounded ring while
// preserving the same semantic checksum line.

const ITERATIONS = 500000;
const PRESSURE_RETAIN = 8192;
const pressureSink: any[] = new Array(PRESSURE_RETAIN);

// Warmup
for (let i = 0; i < 1000; i++) {
  const obj = { x: i, y: i * 2, name: "item_" + i };
  pressureSink[i % PRESSURE_RETAIN] = obj;
}

const start = Date.now();

let checksum = 0;
for (let i = 0; i < ITERATIONS; i++) {
  const obj: any = { x: i, y: i * 2, name: "item_" + i };
  const arr = [i, i + 1, i + 2];
  if ((i & 3) === 0) {
    pressureSink[(i >> 2) % PRESSURE_RETAIN] = { obj, arr };
  }
  checksum += obj.x + arr[0];
}

const elapsed = Date.now() - start;
console.log("gc_pressure:" + elapsed);
console.log("checksum:" + checksum);
EOF
}

correctness_status() {
  local report="$1"
  python3 - "$report" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
try:
    data = json.loads(path.read_text(encoding="utf-8"))
except Exception:
    print("missing")
    raise SystemExit(0)
status = data.get("status")
print(status if isinstance(status, str) else "missing")
PY
}

run_required_benchmark_gc_traces() {
  local label="$1"
  local worktree="$2"
  local label_out="$3"
  local perry_bin="$worktree/target/release/perry"
  local verifier="$worktree/benchmarks/verify_benchmark_output.py"
  local trace_root="$label_out/memory/traces/required-benchmark"
  local evidence_root="$label_out/benchmark-gc-traces"
  local log="$label_out/logs/benchmark-gc-traces.log"
  local code=0
  local index=0
  local node_available=0
  local node_strip="suite-sed"

  if [[ -n "$NODE_RESOLVED_BIN" && -x "$NODE_RESOLVED_BIN" ]]; then
    node_available=1
  fi
  if command -v esbuild >/dev/null 2>&1; then
    node_strip="esbuild"
  elif command -v npx >/dev/null 2>&1 && npx --no-install esbuild --version >/dev/null 2>&1; then
    node_strip="npx-esbuild"
  elif command -v tsc >/dev/null 2>&1; then
    node_strip="tsc"
  fi

  mkdir -p \
    "$trace_root" \
    "$evidence_root/bin" \
    "$evidence_root/reference" \
    "$evidence_root/source" \
    "$evidence_root/stdout" \
    "$evidence_root/stderr" \
    "$evidence_root/reference-stdout" \
    "$evidence_root/reference-stderr" \
    "$evidence_root/correctness" \
    "$evidence_root/compile" \
    "$label_out/logs"
  : >"$log"

  local row
  for row in "${REQUIRED_BENCHMARK_TRACE_ROWS[@]}"; do
    index=$((index + 1))
    local src="$worktree/benchmarks/suite/$row.ts"
    if [[ "$row" == "bench_gc_pressure" ]]; then
      src="$evidence_root/source/bench_gc_pressure_trace.ts"
      write_bench_gc_pressure_trace_source "$src"
    fi
    local bin="$evidence_root/bin/$row"
    local compile_log="$evidence_root/compile/$row.log"
    local stdout="$evidence_root/stdout/$row.out"
    local stderr="$evidence_root/stderr/$row.trace.log"
    local reference_input="$evidence_root/reference/$row.mjs"
    local reference_stdout="$evidence_root/reference-stdout/$row.out"
    local reference_stderr="$evidence_root/reference-stderr/$row.log"
    local correctness_json="$evidence_root/correctness/$row.json"
    local copied_trace
    copied_trace=$(printf '%s/%03d_%s.log' "$trace_root" "$index" "$row")

    {
      echo "=== required benchmark GC trace: $row ==="
      if [[ ! -f "$src" ]]; then
        echo "missing source: $src"
        code=1
        continue
      fi
      if ! "$perry_bin" compile --no-cache "$src" -o "$bin" >"$compile_log" 2>&1; then
        echo "compile failed: $compile_log"
        code=1
        continue
      fi
      if [[ "$node_available" -ne 1 ]]; then
        echo "Node is required for traced benchmark reference output: ${NODE_RESOLUTION_ERROR:-no Node candidate selected}"
        code=1
        continue
      fi
      if [[ ! -f "$verifier" ]]; then
        echo "missing benchmark output verifier: $verifier"
        code=1
        continue
      fi
      NODE_TS_STRIP="$node_strip"
      if ! precompile_node_reference "$src" "$reference_input"; then
        echo "failed to prepare Node reference input: $src"
        code=1
        continue
      fi
      if ! env PERRY_GC_TRACE=1 "$NODE_RESOLVED_BIN" "$reference_input" >"$reference_stdout" 2>"$reference_stderr"; then
        echo "Node reference run failed: $reference_stderr"
        code=1
        continue
      fi
      if ! env PERRY_GC_TRACE=1 PERRY_GC_MODE=latency "$bin" >"$stdout" 2>"$stderr"; then
        echo "trace run failed: $stderr"
        code=1
        if [[ -f "$stderr" ]]; then
          cp "$stderr" "$copied_trace" || true
        fi
        continue
      fi
      if ! trace_has_gc_cycle "$stderr"; then
        echo "trace emitted no gc_cycle JSON events: $stderr"
        code=1
      fi
      if ! cp "$stderr" "$copied_trace"; then
        echo "failed to copy trace: $stderr -> $copied_trace"
        code=1
        continue
      fi
      if ! python3 "$verifier" \
        --expected "$reference_stdout" \
        --actual "$stdout" \
        --reference node \
        --json-out "$correctness_json"; then
        echo "traced stdout does not match Node reference: $correctness_json"
        code=1
      fi
      local status
      status="$(correctness_status "$correctness_json")"
      if [[ "$status" != "pass" ]]; then
        echo "traced stdout correctness did not pass (status=$status): $correctness_json"
        code=1
      fi
      echo "trace: $copied_trace"
      echo "reference_stdout: $reference_stdout"
      echo "correctness: $correctness_json"
    } >>"$log" 2>&1
  done

  local status="pass"
  if [[ "$code" -ne 0 ]]; then
    status="fail"
  fi
  record_command "$label" "benchmark_gc_traces" "$status" "$code" "$log" ""
  echo "=== $label: benchmark_gc_traces ==="
  echo "  $status (exit=$code, log=$log)"
}

run_for_label() {
  local label="$1"
  local worktree="$2"
  local label_out="$OUT_ABS/$label"
  mkdir -p "$label_out/logs" "$label_out/benchmarks"

  run_build_for_label "$label" "$worktree"

  if [[ "$(command_status "$label" build)" == "fail" ]]; then
    skip_label_runtime_after_build_failure "$label" "build failed"
    return
  fi

  local perry_bin="$worktree/target/release/perry"
  run_logged "$label" "memory_stability" "$worktree" "$label_out/logs/memory-stability.command.log" \
    env "CARGO_TARGET_DIR=$worktree/target" "PERRY_BIN=$perry_bin" "PERRY_GC_EVIDENCE_DIR=$label_out/memory" scripts/run_memory_stability_tests.sh

  run_logged "$label" "benchmarks" "$worktree" "$label_out/logs/benchmarks-full-runs${RUNS}.log" \
    env "PERRY_BIN=$perry_bin" "PERRY_BENCH_NODE=$NODE_RESOLVED_BIN" benchmarks/compare.sh --full --runs "$RUNS" --json-out "$label_out/benchmarks/full.json"

  run_required_benchmark_gc_traces "$label" "$worktree" "$label_out"

  run_perf_comprehensive "$label" "$worktree" "$label_out"
}

run_build_for_label() {
  local label="$1"
  local worktree="$2"
  local label_out="$OUT_ABS/$label"
  mkdir -p "$label_out/logs" "$label_out/benchmarks"
  run_logged "$label" "build" "$worktree" "$label_out/logs/build.log" \
    env "CARGO_TARGET_DIR=$worktree/target" cargo build --release -p perry
}

skip_label_runtime_after_build_failure() {
  local label="$1"
  local reason="$2"
  record_command "$label" "memory_stability" "skipped" 0 "" "$reason"
  record_command "$label" "benchmarks" "skipped" 0 "" "$reason"
  record_command "$label" "benchmark_gc_traces" "skipped" 0 "" "$reason"
  record_command "$label" "perf_comprehensive" "skipped" 0 "" "$reason"
}

finalize_packet_report() {
  set +e
  REPORT_ARGS=(
    --root "$OUT_ABS"
    --json-out "$OUT_ABS/gc-1090-packet.json"
    --md-out "$OUT_ABS/gc-1090-packet.md"
  )
  if [[ "$GATE" -eq 1 ]]; then
    REPORT_ARGS+=(--gate)
  fi
  python3 "$ROOT/scripts/gc_1090_evidence_report.py" "${REPORT_ARGS[@]}"
  REPORT_EXIT=$?
  set -e

  echo ""
  echo "packet markdown: $OUT_ABS/gc-1090-packet.md"
  echo "packet json:     $OUT_ABS/gc-1090-packet.json"
  exit "$REPORT_EXIT"
}

clean_label_target() {
  local label="$1"
  local worktree="$2"
  if [[ "$KEEP_WORKTREES" -ne 0 || ! -d "$worktree/target" ]]; then
    return
  fi
  local log="$OUT_ABS/$label/logs/target-cleanup.log"
  mkdir -p "$(dirname "$log")"
  {
    du -sh "$worktree/target" 2>/dev/null || true
    rm -rf "$worktree/target"
  } >"$log" 2>&1 || true
  echo "=== $label: target_cleanup ==="
  echo "  pass (log=$log)"
}

command_status() {
  local label="$1"
  local name="$2"
  python3 - "$METADATA" "$label" "$name" <<'PY'
import json
import sys
data = json.load(open(sys.argv[1]))
print(data.get("commands", {}).get(sys.argv[2], {}).get(sys.argv[3], {}).get("status", "missing"))
PY
}

discover_perf_command() {
  local worktree="$1"
  if [[ -x "$worktree/scripts/run_perf_comprehensive.sh" ]]; then
    printf '%s\n' "scripts/run_perf_comprehensive.sh"
    return 0
  fi
  if [[ -x "$worktree/scripts/perf-comprehensive.sh" ]]; then
    printf '%s\n' "scripts/perf-comprehensive.sh"
    return 0
  fi
  return 1
}

run_perf_comprehensive() {
  local label="$1"
  local worktree="$2"
  local label_out="$3"
  local perry_bin="$worktree/target/release/perry"
  local log="$label_out/logs/perf-comprehensive.log"

  if [[ "$SKIP_PERF_COMPREHENSIVE" -eq 1 ]]; then
    record_command "$label" "perf_comprehensive" "skipped" 0 "" "skipped by --skip-perf-comprehensive"
    return
  fi

  local cmd
  if ! cmd="$(discover_perf_command "$worktree")"; then
    record_command "$label" "perf_comprehensive" "skipped" 0 "" "command not found"
    return
  fi

  run_logged "$label" "perf_comprehensive" "$worktree" "$log" \
    env "CARGO_TARGET_DIR=$worktree/target" "PERRY_BIN=$perry_bin" "$cmd"
}

run_perf_frontier() {
  local perf_out="$OUT_ABS/perf-frontier"
  local log="$OUT_ABS/logs/perf-frontier.command.log"
  local baseline="$ROOT/tmp/perf-frontier-baseline.json"
  local code=0
  local args=(
    --base-ref "$BASE_SHA"
    --head-ref "$HEAD_SHA"
    --runs "$RUNS"
    --out "$perf_out"
  )
  if [[ -f "$baseline" ]]; then
    args+=(--baseline-in "$baseline")
  else
    args+=(--update-baseline "$baseline")
  fi
  if [[ "$GATE" -eq 1 ]]; then
    args+=(--gate)
  fi
  local row
  for row in "${PERF_MATH_SLICE_ROWS[@]}"; do
    args+=(--math-slice-row "$row")
  done
  mkdir -p "$(dirname "$log")"
  echo "=== packet: perf_frontier ==="
  set +e
  "$ROOT/scripts/perf_frontier_gate.sh" "${args[@]}" >"$log" 2>&1
  code=$?
  set -e
  local status="pass"
  if [[ "$code" -ne 0 ]]; then
    status="fail"
  fi
  record_command "packet" "perf_frontier" "$status" "$code" "$log" ""
  echo "  $status (exit=$code, log=$log)"
}

write_old_page_policy_workloads() {
  local out_dir="$1"
  mkdir -p "$out_dir"

  cat >"$out_dir/bench_json_roundtrip_retained.ts" <<'EOF'
declare function gc(): void;
declare const process: any;

function forceGc(): void {
  gc();
  gc();
  gc();
}

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
items.length = 0;

for (let i = 0; i < 3; i++) {
  const parsed = JSON.parse(blob);
  JSON.stringify(parsed);
}

const ITERATIONS = 50;
const start = Date.now();
let checksum = 0;
for (let iter = 0; iter < ITERATIONS; iter++) {
  const parsed = JSON.parse(blob);
  checksum += parsed.length;
  const reStringified = JSON.stringify(parsed);
  checksum += reStringified.length;
}

forceGc();
const retainedRss = process.memoryUsage().rss;
const elapsed = Date.now() - start;
console.log("json_roundtrip:" + elapsed);
console.log("checksum:" + checksum);
console.log("retained_rss_bytes:" + retainedRss);
EOF

  cat >"$out_dir/old_gen_churn_retained.ts" <<'EOF'
declare function gc(): void;
declare const process: any;

function forceGc(): void {
  gc();
  gc();
}

function makeRecord(i: number): { id: number; name: string; tags: string[] } {
  return {
    id: i,
    name: "record_" + i,
    tags: ["tag_a_" + i, "tag_b_" + i, "tag_c_" + i, "tag_d_" + i],
  };
}

const survivors: any[] = [];
let checksum = 0;

for (let cycle = 0; cycle < 14; cycle++) {
  for (let i = 0; i < 24000; i++) {
    const id = cycle * 24000 + i;
    const record = makeRecord(id);
    checksum += record.id + record.tags.length + record.name.length;
    if (i % 3000 === 0) {
      survivors[(cycle + i / 3000) % 32] = record;
    }
  }

  forceGc();
  checksum += survivors[cycle % 32].id;
  console.log("rss_sample_bytes:" + process.memoryUsage().rss);
}

forceGc();
console.log("old_gen_churn_retained:" + checksum);
EOF
}

run_policy_binary() {
  local stdout="$1"
  local stderr="$2"
  local bin="$3"
  shift 3
  mkdir -p "$(dirname "$stdout")" "$(dirname "$stderr")"
  set +e
  if [[ "$(uname)" == "Darwin" ]]; then
    env "$@" /usr/bin/time -l "$bin" >"$stdout" 2>"$stderr"
  else
    env "$@" /usr/bin/time -v "$bin" >"$stdout" 2>"$stderr"
  fi
  local code=$?
  set -e
  return "$code"
}

run_old_page_policy_evidence() {
  local policy_root="$OUT_ABS/old-page-policy"
  local workloads_dir="$policy_root/workloads"
  local log="$OUT_ABS/logs/old-page-policy.log"
  local json_out="$OUT_ABS/old-page-policy.json"
  local code=0
  mkdir -p "$policy_root" "$(dirname "$log")"
  : >"$log"

  write_old_page_policy_workloads "$workloads_dir"

  local label worktree perry_bin label_out bin compile_log stdout stderr compile_code run_code
  for label in base head; do
    if [[ "$label" == "base" ]]; then
      worktree="$BASE_WT"
    else
      worktree="$HEAD_WT"
    fi
    perry_bin="$worktree/target/release/perry"
    label_out="$policy_root/$label/bench_json_roundtrip_retained"
    bin="$label_out/bench_json_roundtrip_retained"
    compile_log="$label_out/compile.log"
    stdout="$label_out/stdout.log"
    stderr="$label_out/stderr-trace.log"
    mkdir -p "$label_out"

    echo "=== $label: old-page bench_json_roundtrip_retained compile ===" >>"$log"
    set +e
    (
      cd "$worktree"
      "$perry_bin" compile --no-cache "$workloads_dir/bench_json_roundtrip_retained.ts" -o "$bin"
    ) >"$compile_log" 2>&1
    compile_code=$?
    set -e
    echo "$compile_code" >"$label_out/compile.exit"
    cat "$compile_log" >>"$log" || true
    if [[ "$compile_code" -ne 0 ]]; then
      code=1
      continue
    fi

    echo "=== $label: old-page bench_json_roundtrip_retained run ===" >>"$log"
    if run_policy_binary "$stdout" "$stderr" "$bin" PERRY_GC_TRACE=1; then
      run_code=0
    else
      run_code=$?
      code=1
    fi
    echo "$run_code" >"$label_out/run.exit"
    cat "$stdout" >>"$log" || true
    cat "$stderr" >>"$log" || true
  done

  local churn_out="$policy_root/head/old_gen_churn_retained"
  mkdir -p "$churn_out"
  set +e
  (
    cd "$HEAD_WT"
    "$HEAD_WT/target/release/perry" compile --no-cache "$workloads_dir/old_gen_churn_retained.ts" -o "$churn_out/old_gen_churn_retained"
  ) >"$churn_out/compile.log" 2>&1
  compile_code=$?
  set -e
  echo "$compile_code" >"$churn_out/compile.exit"
  cat "$churn_out/compile.log" >>"$log" || true
  if [[ "$compile_code" -ne 0 ]]; then
    code=1
  else
    if run_policy_binary \
      "$churn_out/stdout.log" \
      "$churn_out/stderr-trace.log" \
      "$churn_out/old_gen_churn_retained" \
      PERRY_GC_TRACE=1 PERRY_GEN_GC=1; then
      run_code=0
    else
      run_code=$?
      code=1
    fi
    echo "$run_code" >"$churn_out/run.exit"
    cat "$churn_out/stdout.log" >>"$log" || true
    cat "$churn_out/stderr-trace.log" >>"$log" || true
  fi

  python3 - "$OUT_ABS" "$json_out" <<'PY'
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path

root = Path(sys.argv[1])
out = Path(sys.argv[2])
policy_root = root / "old-page-policy"


def read_json(path, default):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return default


def read_exit(path):
    try:
        return int(path.read_text(encoding="utf-8").strip())
    except Exception:
        return None


def parse_stdout(path):
    result = {
        "checksum": None,
        "retained_rss_bytes": None,
        "retained_rss_kb": None,
        "samples_rss_kb": [],
        "stdout_path": str(path),
    }
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except Exception:
        return result
    for line in lines:
        if line.startswith("checksum:"):
            try:
                result["checksum"] = int(line.split(":", 1)[1])
            except ValueError:
                pass
        elif line.startswith("retained_rss_bytes:"):
            try:
                value = int(line.split(":", 1)[1])
            except ValueError:
                continue
            result["retained_rss_bytes"] = value
            result["retained_rss_kb"] = (value + 1023) // 1024
        elif line.startswith("rss_sample_bytes:"):
            try:
                value = int(line.split(":", 1)[1])
            except ValueError:
                continue
            result["samples_rss_kb"].append((value + 1023) // 1024)
        elif line.startswith("old_gen_churn_retained:"):
            try:
                result["checksum"] = int(line.split(":", 1)[1])
            except ValueError:
                pass
    return result


def parse_time_bytes_or_kb(value):
    if value > 10_000_000:
        return value // 1024
    return value


def parse_peak_rss_kb(path):
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except Exception:
        return None
    match = re.search(r"(\d+)\s+maximum resident set size", text)
    if match:
        return parse_time_bytes_or_kb(int(match.group(1)))
    match = re.search(r"Maximum resident set size \(kbytes\):\s+(\d+)", text)
    if match:
        return int(match.group(1))
    return None


def parse_peak_footprint_kb(path):
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except Exception:
        return None
    match = re.search(r"(\d+)\s+peak memory footprint", text)
    if match:
        return parse_time_bytes_or_kb(int(match.group(1)))
    return None


def trace_totals(path):
    totals = {
        "candidate_pages": 0,
        "selected_pages": 0,
        "selected_live_bytes": 0,
        "reclaimable_bytes": 0,
        "old_page_scanned_objects": 0,
        "old_page_scanned_bytes": 0,
        "old_page_moved_objects": 0,
        "old_page_moved_bytes": 0,
        "released_original_objects": 0,
        "released_original_bytes": 0,
        "released_original_reusable_bytes": 0,
        "released_original_returned_bytes": 0,
        "reusable_bytes": 0,
        "returned_bytes": 0,
    }
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except Exception:
        return totals
    for line in lines:
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(event, dict) or event.get("event") != "gc_cycle":
            continue
        policy = event.get("evacuation_policy", {})
        evacuation = event.get("evacuation", {})
        old_pages = event.get("old_pages", {})
        sweep = event.get("sweep", {})
        if isinstance(policy, dict):
            totals["candidate_pages"] += max(0, int(policy.get("old_page_candidate_pages", 0) or 0))
            totals["selected_pages"] += max(0, int(policy.get("old_page_selected_pages", 0) or 0))
            totals["selected_live_bytes"] += max(0, int(policy.get("old_page_selected_live_bytes", 0) or 0))
            totals["reclaimable_bytes"] += max(0, int(policy.get("old_page_reclaimable_bytes", 0) or 0))
        if isinstance(evacuation, dict):
            for key in (
                "old_page_scanned_objects",
                "old_page_scanned_bytes",
                "old_page_moved_objects",
                "old_page_moved_bytes",
                "released_original_objects",
                "released_original_bytes",
                "released_original_reusable_bytes",
                "released_original_returned_bytes",
            ):
                totals[key] += max(0, int(evacuation.get(key, 0) or 0))
        if isinstance(old_pages, dict):
            totals["reusable_bytes"] += max(0, int(old_pages.get("reusable_bytes", 0) or 0))
            totals["returned_bytes"] += max(0, int(old_pages.get("returned_bytes", 0) or 0))
        if isinstance(sweep, dict):
            totals["reusable_bytes"] += max(0, int(sweep.get("reusable_bytes", 0) or 0))
            totals["returned_bytes"] += max(0, int(sweep.get("returned_bytes", 0) or 0))
    return totals


def benchmark_peak(label):
    report = read_json(root / label / "benchmarks" / "full.json", {})
    try:
        return report["benchmarks"]["bench_json_roundtrip"]["perry_rss_kb"]
    except Exception:
        return None


def bench_entry(label):
    run_dir = policy_root / label / "bench_json_roundtrip_retained"
    stdout = run_dir / "stdout.log"
    stderr = run_dir / "stderr-trace.log"
    parsed = parse_stdout(stdout)
    parsed.update({
        "compile_exit": read_exit(run_dir / "compile.exit"),
        "run_exit": read_exit(run_dir / "run.exit"),
        "compile_log": str(run_dir / "compile.log"),
        "trace_path": str(stderr),
        "peak_rss_kb": parse_peak_rss_kb(stderr),
        "benchmark_peak_reported_kb": benchmark_peak(label),
        "probe_peak_rss_kb": parse_peak_rss_kb(stderr),
        "probe_peak_footprint_kb": parse_peak_footprint_kb(stderr),
        "old_page": trace_totals(stderr),
    })
    return parsed


def churn_entry():
    run_dir = policy_root / "head" / "old_gen_churn_retained"
    stdout = run_dir / "stdout.log"
    stderr = run_dir / "stderr-trace.log"
    parsed = parse_stdout(stdout)
    samples = parsed.get("samples_rss_kb", [])
    warmup = 8
    plateau = samples[warmup:]
    parsed.update({
        "compile_exit": read_exit(run_dir / "compile.exit"),
        "run_exit": read_exit(run_dir / "run.exit"),
        "compile_log": str(run_dir / "compile.log"),
        "trace_path": str(stderr),
        "probe_peak_rss_kb": parse_peak_rss_kb(stderr),
        "probe_peak_footprint_kb": parse_peak_footprint_kb(stderr),
        "old_page": trace_totals(stderr),
        "warmup_samples": warmup,
        "plateau_allowance_kb": 64 * 1024,
        "plateau_delta_kb": (max(plateau) - min(plateau)) if plateau else None,
    })
    return parsed


packet = {
    "schema_version": 1,
    "generated_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "workloads_dir": str(policy_root / "workloads"),
    "bench_json_roundtrip_retained": {
        "base": bench_entry("base"),
        "head": bench_entry("head"),
    },
    "old_gen_churn_retained": churn_entry(),
}

out.write_text(json.dumps(packet, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY
  if [[ ! -f "$json_out" ]]; then
    code=1
  fi

  local status="pass"
  if [[ "$code" -ne 0 ]]; then
    status="fail"
  fi
  record_command "packet" "old_page_policy" "$status" "$code" "$log" ""
  echo "=== packet: old_page_policy ==="
  echo "  $status (exit=$code, log=$log)"
}

run_gc_store_inventory() {
  local log="$OUT_ABS/logs/gc-store-site-inventory.log"
  local out="$OUT_ABS/gc-store-site-inventory.json"
  local code=0
  local args=(--json-out "$out")
  if [[ "$GATE" -eq 1 ]]; then
    args+=(--gate)
  fi
  mkdir -p "$(dirname "$log")"
  echo "=== packet: gc_store_inventory ==="
  set +e
  (
    cd "$HEAD_WT"
    python3 scripts/gc_store_site_inventory.py "${args[@]}"
  ) >"$log" 2>&1
  code=$?
  set -e
  local status="pass"
  if [[ "$code" -ne 0 ]]; then
    status="fail"
  fi
  record_command "packet" "gc_store_inventory" "$status" "$code" "$log" ""
  echo "  $status (exit=$code, log=$log)"
}

run_type_lowering_evidence() {
  local type_out="$OUT_ABS/type-lowering"
  local log="$OUT_ABS/logs/type-lowering-evidence.log"
  local perry_bin="$HEAD_WT/target/release/perry"
  local code=0
  local args=(
    --out "$type_out"
    --runs "$RUNS"
    --compiler-runs 1
    --benchmark-mode smoke
    --perry "$perry_bin"
  )
  if [[ "$GATE" -eq 1 ]]; then
    args+=(--gate)
  fi
  mkdir -p "$(dirname "$log")"
  echo "=== packet: type_lowering ==="
  set +e
  (
    cd "$HEAD_WT"
    env "PERRY_BIN=$perry_bin" python3 scripts/type_lowering_evidence_packet.py "${args[@]}"
  ) >"$log" 2>&1
  code=$?
  set -e
  local status="pass"
  if [[ "$code" -ne 0 ]]; then
    status="fail"
  fi
  record_command "packet" "type_lowering" "$status" "$code" "$log" ""
  echo "  $status (exit=$code, log=$log)"
}

run_for_label "base" "$BASE_WT"
if [[ "$(command_status base build)" == "fail" ]]; then
  echo "base build failed; running head build, then skipping comparison evidence that requires a runnable base"
  run_build_for_label "head" "$HEAD_WT"
  if [[ "$(command_status head build)" == "fail" ]]; then
    skip_label_runtime_after_build_failure "head" "head build failed after base build failed"
    record_command "packet" "type_lowering" "skipped" 0 "" "head build failed after base build failed"
  else
    skip_label_runtime_after_build_failure "head" "base build failed"
    run_type_lowering_evidence
  fi
  record_command "packet" "old_page_policy" "skipped" 0 "" "base build failed"
  record_command "packet" "gc_store_inventory" "skipped" 0 "" "base build failed"
  record_command "packet" "perf_frontier" "skipped" 0 "" "base build failed"
  record_command "packet" "early_abort" "fail" 1 "" "base build failed"
  clean_label_target "base" "$BASE_WT"
  clean_label_target "head" "$HEAD_WT"
  finalize_packet_report
fi

run_for_label "head" "$HEAD_WT"
run_old_page_policy_evidence
run_type_lowering_evidence
clean_label_target "base" "$BASE_WT"
clean_label_target "head" "$HEAD_WT"
run_gc_store_inventory
run_perf_frontier

finalize_packet_report
