#!/usr/bin/env python3
"""A/B runner for the typed-array parameter lowering benchmark.

This intentionally compares the current lowering against Perry's
PERRY_DISABLE_BUFFER_FAST_PATH baseline for the same source file. It also runs
a stripped JavaScript reference under Node and fails closed on checksum drift.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from scripts.compiler_output_harness.analyzers import count_calls_by_name, parse_kept_paths
from scripts.compiler_output_harness.common import BUFFER_SLOW_PATH_HELPERS

SCRIPT_DIR = Path(__file__).resolve().parent
SOURCE = SCRIPT_DIR / "bench_typedarray_param_sum.ts"
OFFSET_SOURCE = SCRIPT_DIR / "bench_typedarray_param_offset.ts"
EXPECTED_CHECKSUM = "6323324000"
EXPECTED_OFFSET_CHECKSUM = "98"
LOADS_PER_RUN = 131_072 * 1000
DEFAULT_MIN_SPEEDUP = 8.0
TYPED_ARRAY_ELEMENT_HELPERS = (
    "js_typed_array_get",
    "js_typed_array_set",
    "js_typed_array_index_get_dynamic",
    "js_typed_array_index_set_dynamic",
)
TS_TYPES = (
    "number|string|boolean|any|void|Float64Array|Float32Array|Int32Array|"
    "Uint32Array|Int16Array|Uint16Array|Int8Array|Uint8Array|Uint8ClampedArray"
)


def run(cmd: list[str], *, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    merged_env = os.environ.copy()
    if env:
        merged_env.update(env)
    return subprocess.run(
        cmd,
        cwd=REPO_ROOT,
        env=merged_env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=True,
    )


def ensure_perry() -> Path:
    override = os.environ.get("PERRY_BIN")
    if override:
        perry = Path(override)
        if not perry.exists():
            raise RuntimeError(f"PERRY_BIN does not exist: {perry}")
        return perry
    perry = REPO_ROOT / "target/release/perry"
    subprocess.run(
        ["cargo", "build", "--release", "-p", "perry"],
        cwd=REPO_ROOT,
        check=True,
    )
    return perry


def node_binary() -> str:
    override = os.environ.get("PERRY_BENCH_NODE")
    if override:
        node = Path(override)
        if not node.exists():
            raise RuntimeError(f"PERRY_BENCH_NODE does not exist: {node}")
        return str(node)
    found = shutil.which("node")
    if not found:
        raise RuntimeError("node not found; set PERRY_BENCH_NODE")
    return found


def strip_types(source: Path, out: Path) -> None:
    text = source.read_text(encoding="utf-8")
    text = re.sub(rf": ({TS_TYPES})(\[\])?", "", text)
    text = re.sub(rf"\): ({TS_TYPES})(\[\])? \{{", ") {", text)
    out.write_text(text, encoding="utf-8")


def static_pressure_from_compile_log(log_text: str) -> dict[str, object]:
    ir_paths, _, _, _ = parse_kept_paths(log_text)
    if not ir_paths:
        raise RuntimeError("PERRY_LLVM_KEEP_IR did not report a retained LLVM IR path")
    ir_path = ir_paths[-1]
    ir = ir_path.read_text(encoding="utf-8")
    calls = count_calls_by_name(ir)
    runtime_calls = {
        name: count
        for name, count in calls.items()
        if name.startswith(("js_", "perry_runtime_"))
    }
    typed_array_element_helpers = {
        name: count for name, count in calls.items() if name in TYPED_ARRAY_ELEMENT_HELPERS
    }
    typed_array_helpers = {
        name: count for name, count in calls.items() if name.startswith("js_typed_array_")
    }
    buffer_slow_path_accesses = sum(
        count
        for name, count in calls.items()
        if any(helper in name for helper in BUFFER_SLOW_PATH_HELPERS)
    )
    return {
        "ir_path": str(ir_path),
        "runtime_calls_static": sum(runtime_calls.values()),
        "runtime_call_names_static": runtime_calls,
        "typed_array_helper_calls_static": sum(typed_array_helpers.values()),
        "typed_array_helper_names_static": typed_array_helpers,
        "typed_array_element_helper_calls_static": sum(
            typed_array_element_helpers.values()
        ),
        "typed_array_element_helper_names_static": typed_array_element_helpers,
        "buffer_slow_path_accesses_static": buffer_slow_path_accesses,
        "boxed_number_allocations_static": calls.get("js_boxed_number_new", 0),
    }


def compile_source(
    perry: Path,
    source: Path,
    out: Path,
    *,
    disabled: bool = False,
    capture_static_pressure: bool = False,
) -> dict[str, object] | None:
    env = {"PERRY_NO_CACHE": "1"}
    if disabled:
        env["PERRY_DISABLE_BUFFER_FAST_PATH"] = "1"
    if capture_static_pressure:
        env["PERRY_LLVM_KEEP_IR"] = "1"
    proc = run([str(perry), str(source), "-o", str(out)], env=env)
    if not capture_static_pressure:
        return None
    return static_pressure_from_compile_log(f"{proc.stdout}\n{proc.stderr}")


def compile_benchmark(
    perry: Path, out: Path, *, disabled: bool = False
) -> dict[str, object]:
    pressure = compile_source(
        perry,
        SOURCE,
        out,
        disabled=disabled,
        capture_static_pressure=True,
    )
    assert pressure is not None
    return pressure


def parse_output(stdout: str, label: str) -> tuple[int, str]:
    timing = re.search(r"typedarray_param_sum:(\d+)", stdout)
    checksum = re.search(r"checksum:(\d+)", stdout)
    if not timing or not checksum:
        raise RuntimeError(f"{label} missing benchmark output:\n{stdout}")
    return int(timing.group(1)), checksum.group(1)


def run_timed(cmd: list[str], label: str, runs: int) -> dict[str, object]:
    times_ms: list[int] = []
    rss_kib: list[int] = []
    checksums: list[str] = []
    for _ in range(runs):
        proc = subprocess.run(
            ["/usr/bin/time", "-v", *cmd],
            cwd=REPO_ROOT,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=True,
        )
        elapsed_ms, checksum = parse_output(proc.stdout, label)
        rss_match = re.search(r"Maximum resident set size \(kbytes\): (\d+)", proc.stderr)
        if not rss_match:
            raise RuntimeError(f"{label} missing /usr/bin/time RSS output:\n{proc.stderr}")
        times_ms.append(elapsed_ms)
        rss_kib.append(int(rss_match.group(1)))
        checksums.append(checksum)
    if set(checksums) != {EXPECTED_CHECKSUM}:
        raise RuntimeError(f"{label} checksum mismatch: {checksums}")
    median_ms = statistics.median(times_ms)
    return {
        "times_ms": times_ms,
        "median_ms": median_ms,
        "mean_ms": statistics.mean(times_ms),
        "min_ms": min(times_ms),
        "max_ms": max(times_ms),
        "throughput_million_loads_per_s": (LOADS_PER_RUN / (median_ms / 1000.0)) / 1_000_000.0,
        "max_rss_kib": max(rss_kib),
        "median_rss_kib": statistics.median(rss_kib),
        "checksum": EXPECTED_CHECKSUM,
    }


def parse_offset_output(stdout: str, label: str) -> str:
    checksum = re.search(r"typedarray_param_offset:([-+]?\d+(?:\.\d+)?)", stdout)
    if not checksum:
        raise RuntimeError(f"{label} missing offset correctness output:\n{stdout}")
    value = float(checksum.group(1))
    if value != float(EXPECTED_OFFSET_CHECKSUM):
        raise RuntimeError(f"{label} offset checksum mismatch: {checksum.group(1)}")
    return checksum.group(1)


def run_offset_correctness(perry: Path, node: str, tmp: Path) -> dict[str, object]:
    offset_bin = tmp / "perry_typedarray_param_offset"
    offset_js = tmp / "bench_typedarray_param_offset.js"
    strip_types(OFFSET_SOURCE, offset_js)
    compile_source(perry, OFFSET_SOURCE, offset_bin)

    node_proc = run([node, str(offset_js)])
    perry_proc = run([str(offset_bin)])
    node_checksum = parse_offset_output(node_proc.stdout, "node_offset_reference")
    perry_checksum = parse_offset_output(perry_proc.stdout, "perry_offset")
    if float(node_checksum) != float(perry_checksum):
        raise RuntimeError(
            f"offset checksum mismatch: node={node_checksum} perry={perry_checksum}"
        )
    return {
        "status": "pass",
        "source": str(OFFSET_SOURCE.relative_to(REPO_ROOT)),
        "expected_checksum": EXPECTED_OFFSET_CHECKSUM,
        "node_checksum": node_checksum,
        "perry_checksum": perry_checksum,
        "node_stdout": node_proc.stdout.strip().splitlines(),
        "perry_stdout": perry_proc.stdout.strip().splitlines(),
    }


def gc_trace(cmd: list[str]) -> dict[str, object]:
    proc = run(cmd, env={"PERRY_GC_TRACE": "1", "PERRY_GC_DIAG": "1"})
    parse_output(proc.stdout, "gc_trace")
    events = []
    for stream in (proc.stdout, proc.stderr):
        for line in stream.splitlines():
            line = line.strip()
            if not line.startswith("{"):
                continue
            try:
                events.append(json.loads(line))
            except json.JSONDecodeError:
                pass
    pauses: list[float] = []
    total_gc_ms = 0.0
    young_allocated_bytes = 0
    copied_minor_success = 0
    copied_minor_eligible = 0
    for event in events:
        text = json.dumps(event, sort_keys=True)
        if "copied" in text and "minor" in text:
            copied_minor_eligible += 1
            if "success" in text or '"copied_minor_success":true' in text:
                copied_minor_success += 1
        for key in ("pause_ms", "duration_ms", "elapsed_ms", "total_ms"):
            value = event.get(key)
            if isinstance(value, (int, float)):
                pauses.append(float(value))
                total_gc_ms += float(value)
                break
        for key in ("young_allocated_bytes", "allocated_bytes"):
            value = event.get(key)
            if isinstance(value, int):
                young_allocated_bytes += value
                break
    pauses_sorted = sorted(pauses)

    def percentile(p: float) -> float:
        if not pauses_sorted:
            return 0.0
        idx = min(len(pauses_sorted) - 1, int(round((len(pauses_sorted) - 1) * p)))
        return pauses_sorted[idx]

    success_rate = (
        copied_minor_success / copied_minor_eligible if copied_minor_eligible else None
    )
    return {
        "events": len(events),
        "gc_cycles": len(pauses),
        "total_gc_time_ms": total_gc_ms,
        "p95_pause_ms": percentile(0.95),
        "p99_pause_ms": percentile(0.99),
        "young_allocated_bytes_observed": young_allocated_bytes,
        "copied_minor_eligible_cycles": copied_minor_eligible,
        "copied_minor_success_cycles": copied_minor_success,
        "copied_minor_success_rate": success_rate,
    }


def main() -> int:
    runs = int(os.environ.get("PERRY_TYPEDARRAY_PARAM_RUNS", "9"))
    min_speedup = float(os.environ.get("PERRY_TYPEDARRAY_PARAM_MIN_SPEEDUP", DEFAULT_MIN_SPEEDUP))
    perry = ensure_perry()
    node = node_binary()
    node_version = run([node, "--version"]).stdout.strip()
    with tempfile.TemporaryDirectory(prefix="perry_typedarray_param_") as tmp_s:
        tmp = Path(tmp_s)
        fast = tmp / "perry_typedarray_param_fast"
        disabled = tmp / "perry_typedarray_param_disabled"
        node_js = tmp / "bench_typedarray_param_sum.js"
        strip_types(SOURCE, node_js)
        fast_static_pressure = compile_benchmark(perry, fast)
        disabled_static_pressure = compile_benchmark(perry, disabled, disabled=True)
        reference = run_timed([node, str(node_js)], "node_reference", runs)
        fast_result = run_timed([str(fast)], "perry_fast", runs)
        disabled_result = run_timed([str(disabled)], "perry_disabled", runs)
        if fast_result["checksum"] != reference["checksum"]:
            raise RuntimeError("Perry fast checksum did not match Node reference")
        if disabled_result["checksum"] != reference["checksum"]:
            raise RuntimeError("Perry disabled checksum did not match Node reference")
        speedup = disabled_result["median_ms"] / fast_result["median_ms"]
        rss_regression_pct = (
            (fast_result["max_rss_kib"] - disabled_result["max_rss_kib"])
            / disabled_result["max_rss_kib"]
            * 100.0
            if disabled_result["max_rss_kib"]
            else 0.0
        )
        report = {
            "source": str(SOURCE.relative_to(REPO_ROOT)),
            "loads_per_run": LOADS_PER_RUN,
            "runs": runs,
            "min_speedup_threshold": min_speedup,
            "node_binary": node,
            "node_version": node_version,
            "node_reference": reference,
            "perry_fast": fast_result,
            "perry_disabled_fast_path_baseline": disabled_result,
            "static_pressure": {
                "perry_fast": fast_static_pressure,
                "perry_disabled_fast_path_baseline": disabled_static_pressure,
                "typed_array_slow_path_reduction_static": (
                    disabled_static_pressure["buffer_slow_path_accesses_static"]
                    - fast_static_pressure["buffer_slow_path_accesses_static"]
                ),
                "typed_array_element_helper_reduction_static": (
                    disabled_static_pressure["typed_array_element_helper_calls_static"]
                    - fast_static_pressure["typed_array_element_helper_calls_static"]
                ),
            },
            "speedup_fast_vs_disabled": speedup,
            "rss_regression_pct_fast_vs_disabled": rss_regression_pct,
            "offset_correctness": run_offset_correctness(perry, node, tmp),
            "gc_trace_fast": gc_trace([str(fast)]),
            "gc_trace_disabled": gc_trace([str(disabled)]),
        }
        print(json.dumps(report, indent=2, sort_keys=True))
        if speedup < min_speedup:
            raise RuntimeError(
                f"typed-array param speedup below target: {speedup:.2f}x < {min_speedup:.2f}x"
            )
        return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:
        print(f"typedarray_param_lowering_runner_error: {exc}", file=sys.stderr)
        raise
