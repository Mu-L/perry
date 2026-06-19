#!/usr/bin/env python3
"""Build a small gated evidence packet for the typed-array lowering win.

This packet is intentionally narrower than the #1090 GC packet. It proves the
material type-lowering row even when the broad GC comparison packet is blocked
by an unrelated baseline/build issue.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUT = REPO_ROOT / "tmp" / "type-lowering-evidence"
DEFAULT_WORKLOAD = "typedarray_param_sum"
DEFAULT_SPEEDUP_THRESHOLD = 8.0
DEFAULT_RSS_REGRESSION_THRESHOLD_PCT = 5.0
EXPECTED_SUM_CHECKSUM = "6323324000"
EXPECTED_OFFSET_CHECKSUM = "98"


def utc_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def run_command(
    cmd: list[str],
    *,
    cwd: Path = REPO_ROOT,
    env: dict[str, str] | None = None,
    stdout_path: Path | None = None,
    stderr_path: Path | None = None,
    timeout: int | None = None,
) -> dict[str, Any]:
    merged_env = os.environ.copy()
    if env:
        merged_env.update(env)
    proc = subprocess.run(
        cmd,
        cwd=cwd,
        env=merged_env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
    )
    if stdout_path is not None:
        stdout_path.parent.mkdir(parents=True, exist_ok=True)
        stdout_path.write_text(proc.stdout, encoding="utf-8")
    if stderr_path is not None:
        stderr_path.parent.mkdir(parents=True, exist_ok=True)
        stderr_path.write_text(proc.stderr, encoding="utf-8")
    return {
        "cmd": cmd,
        "exit_code": proc.returncode,
        "stdout": proc.stdout,
        "stderr": proc.stderr,
        "stdout_path": str(stdout_path) if stdout_path else None,
        "stderr_path": str(stderr_path) if stderr_path else None,
    }


def git_output(args: list[str]) -> str:
    proc = subprocess.run(
        ["git", *args],
        cwd=REPO_ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=True,
    )
    return proc.stdout.strip()


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str | None:
    try:
        return sha256_bytes(path.read_bytes())
    except OSError:
        return None


def source_state(out_dir: Path) -> dict[str, Any]:
    status = run_command(["git", "status", "--porcelain=v1", "--untracked-files=all"])
    diff = run_command(["git", "diff", "--binary", "HEAD", "--"])
    status_path = out_dir / "source-status.txt"
    diff_path = out_dir / "source-diff.patch"
    status_path.write_text(status["stdout"], encoding="utf-8")
    diff_path.write_text(diff["stdout"], encoding="utf-8")
    untracked = capture_untracked_sources(out_dir, status["stdout"].splitlines())
    return {
        "head_sha": git_output(["rev-parse", "--verify", "HEAD^{commit}"]),
        "head_tree_sha": git_output(["show", "-s", "--format=%T", "HEAD"]),
        "dirty": bool(status["stdout"].splitlines()),
        "status_path": str(status_path),
        "tracked_diff_path": str(diff_path),
        "tracked_diff_sha256": sha256_bytes(diff["stdout"].encode("utf-8")),
        "status_porcelain_v1": status["stdout"].splitlines(),
        "untracked_files": untracked,
    }


def capture_untracked_sources(out_dir: Path, status_lines: list[str]) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    snapshot_root = out_dir / "source-untracked"
    for line in status_lines:
        if not line.startswith("?? "):
            continue
        rel = line[3:]
        source = REPO_ROOT / rel
        files = [source]
        if source.is_dir():
            files = [path for path in source.rglob("*") if path.is_file()]
        for path in files:
            if not path.is_file():
                continue
            try:
                rel_path = path.relative_to(REPO_ROOT)
            except ValueError:
                continue
            dest = snapshot_root / rel_path
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, dest)
            data = path.read_bytes()
            rows.append(
                {
                    "path": str(rel_path),
                    "snapshot_path": str(dest),
                    "sha256": sha256_bytes(data),
                    "bytes": len(data),
                }
            )
    return rows


def parse_json_stdout(stdout: str, label: str) -> dict[str, Any]:
    text = stdout.strip()
    if not text:
        raise ValueError(f"{label} produced empty stdout")
    try:
        return json.loads(text)
    except json.JSONDecodeError as exc:
        raise ValueError(f"{label} did not produce JSON stdout: {exc}") from exc


def compiler_capture_command(args: argparse.Namespace, compiler_out: Path) -> list[str]:
    cmd = [
        sys.executable,
        "scripts/compiler_output_regression.py",
        "capture",
        "--workload",
        args.workload,
        "--out-dir",
        str(compiler_out),
        "--benchmark-mode",
        args.benchmark_mode,
        "--runs",
        str(args.compiler_runs),
        "--perf-counters",
        "off",
        "--gate",
        "--print-summary",
    ]
    if args.perry:
        cmd.extend(["--perry", args.perry])
    return cmd


def run_compiler_gate(args: argparse.Namespace, out_dir: Path) -> dict[str, Any]:
    compiler_out = out_dir / "compiler-output"
    logs = out_dir / "logs"
    result = run_command(
        compiler_capture_command(args, compiler_out),
        stdout_path=logs / "compiler-output.stdout",
        stderr_path=logs / "compiler-output.stderr",
        timeout=args.timeout,
    )
    summary: dict[str, Any] | None = None
    if result["stdout"].strip():
        try:
            summary = parse_json_stdout(result["stdout"], "compiler-output gate")
        except ValueError:
            summary = None
    manifest_path = compiler_out / "manifest.json"
    structural_report_path = compiler_out / "structural-report.json"
    manifest = (
        json.loads(manifest_path.read_text(encoding="utf-8"))
        if manifest_path.exists()
        else None
    )
    structural_report = (
        json.loads(structural_report_path.read_text(encoding="utf-8"))
        if structural_report_path.exists()
        else None
    )
    return {
        "status": "pass" if result["exit_code"] == 0 else "fail",
        "exit_code": result["exit_code"],
        "summary": summary,
        "manifest_path": str(manifest_path) if manifest_path.exists() else None,
        "structural_report_path": (
            str(structural_report_path) if structural_report_path.exists() else None
        ),
        "manifest": manifest,
        "structural_report": structural_report,
        "stdout_path": result["stdout_path"],
        "stderr_path": result["stderr_path"],
    }


def selected_perry_path(args: argparse.Namespace) -> Path:
    return Path(args.perry) if args.perry else REPO_ROOT / "target/release/perry"


def run_release_build(args: argparse.Namespace, out_dir: Path) -> dict[str, Any]:
    if args.perry:
        perry = selected_perry_path(args)
        return {
            "status": "external",
            "exit_code": 0 if perry.exists() else 1,
            "perry": str(perry),
            "perry_sha256": sha256_file(perry),
            "detail": "--perry supplied; cargo build skipped",
        }
    logs = out_dir / "logs"
    result = run_command(
        ["cargo", "build", "--release", "-p", "perry"],
        stdout_path=logs / "cargo-build-release.stdout",
        stderr_path=logs / "cargo-build-release.stderr",
        timeout=args.timeout,
    )
    return {
        "status": "pass" if result["exit_code"] == 0 else "fail",
        "exit_code": result["exit_code"],
        "perry": str(selected_perry_path(args)),
        "perry_sha256": sha256_file(selected_perry_path(args)),
        "stdout_path": result["stdout_path"],
        "stderr_path": result["stderr_path"],
    }


def run_ab_benchmark(args: argparse.Namespace, out_dir: Path) -> dict[str, Any]:
    logs = out_dir / "logs"
    env = {
        "PERRY_BIN": str(selected_perry_path(args)),
        "PERRY_TYPEDARRAY_PARAM_RUNS": str(args.runs),
        "PERRY_TYPEDARRAY_PARAM_MIN_SPEEDUP": str(args.speedup_threshold),
    }
    result = run_command(
        [sys.executable, "benchmarks/suite/run_typedarray_param_lowering.py"],
        env=env,
        stdout_path=logs / "typedarray-param-lowering.stdout",
        stderr_path=logs / "typedarray-param-lowering.stderr",
        timeout=args.timeout,
    )
    report: dict[str, Any] | None = None
    if result["stdout"].strip():
        try:
            report = parse_json_stdout(result["stdout"], "typed-array A/B runner")
        except ValueError:
            report = None
    report_path = out_dir / "typedarray-param-lowering.json"
    if report is not None:
        report_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return {
        "status": "pass" if result["exit_code"] == 0 else "fail",
        "exit_code": result["exit_code"],
        "report_path": str(report_path) if report is not None else None,
        "report": report,
        "stdout_path": result["stdout_path"],
        "stderr_path": result["stderr_path"],
    }


def check(
    checks: list[dict[str, Any]],
    name: str,
    passed: bool,
    detail: str,
    *,
    severity: str = "error",
) -> None:
    checks.append(
        {
            "name": name,
            "status": "pass" if passed else "fail",
            "severity": severity,
            "detail": detail,
        }
    )


def nested(data: dict[str, Any] | None, *keys: str) -> Any:
    value: Any = data
    for key in keys:
        if not isinstance(value, dict):
            return None
        value = value.get(key)
    return value


def checksum_set(report: dict[str, Any] | None) -> set[str]:
    values = set()
    for key in ("node_reference", "perry_fast", "perry_disabled_fast_path_baseline"):
        value = nested(report, key, "checksum")
        if value is not None:
            values.add(str(value))
    return values


def as_float(value: Any) -> float | None:
    if value is None:
        return None
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def offset_correctness_detail(report: dict[str, Any] | None) -> tuple[bool, str, Any]:
    offset = nested(report, "offset_correctness")
    status = nested(report, "offset_correctness", "status")
    node_checksum = nested(report, "offset_correctness", "node_checksum")
    perry_checksum = nested(report, "offset_correctness", "perry_checksum")
    expected_checksum = nested(report, "offset_correctness", "expected_checksum")
    node_numeric = as_float(node_checksum)
    perry_numeric = as_float(perry_checksum)
    expected_numeric = as_float(expected_checksum)
    passed = (
        isinstance(offset, dict)
        and status == "pass"
        and node_numeric is not None
        and perry_numeric is not None
        and node_numeric == perry_numeric
        and node_numeric == as_float(EXPECTED_OFFSET_CHECKSUM)
        and (expected_checksum is None or expected_numeric == as_float(EXPECTED_OFFSET_CHECKSUM))
    )
    return (
        passed,
        (
            f"status={status} node_checksum={node_checksum} "
            f"perry_checksum={perry_checksum} expected_checksum={expected_checksum}"
        ),
        node_checksum,
    )


def evaluate_packet(
    build: dict[str, Any],
    compiler: dict[str, Any],
    benchmark: dict[str, Any],
    *,
    speedup_threshold: float = DEFAULT_SPEEDUP_THRESHOLD,
    rss_regression_threshold_pct: float = DEFAULT_RSS_REGRESSION_THRESHOLD_PCT,
) -> dict[str, Any]:
    checks: list[dict[str, Any]] = []
    compiler_report = compiler.get("structural_report") or compiler.get("summary")
    benchmark_report = benchmark.get("report")

    check(
        checks,
        "release_build_current_source",
        build.get("status") in {"pass", "external"} and build.get("exit_code") == 0,
        f"status={build.get('status')} exit_code={build.get('exit_code')} perry={build.get('perry')}",
    )

    check(
        checks,
        "compiler_output_gate",
        compiler.get("status") == "pass"
        and nested(compiler_report, "status") == "pass",
        f"status={compiler.get('status')} report_status={nested(compiler_report, 'status')}",
    )
    check(
        checks,
        "typedarray_ab_runner",
        benchmark.get("status") == "pass" and isinstance(benchmark_report, dict),
        f"status={benchmark.get('status')} report_present={isinstance(benchmark_report, dict)}",
    )

    checksums = checksum_set(benchmark_report)
    check(
        checks,
        "node_perry_checksum_parity",
        len(checksums) == 1 and bool(checksums),
        f"checksums={sorted(checksums)}",
    )
    check(
        checks,
        "expected_sum_checksum",
        checksums == {EXPECTED_SUM_CHECKSUM},
        f"checksums={sorted(checksums)} expected={EXPECTED_SUM_CHECKSUM}",
    )

    offset_passed, offset_detail, offset_checksum = offset_correctness_detail(benchmark_report)
    check(
        checks,
        "offset_subarray_checksum_parity",
        offset_passed,
        offset_detail,
    )

    speedup = nested(benchmark_report, "speedup_fast_vs_disabled")
    check(
        checks,
        "speedup_fast_vs_disabled",
        isinstance(speedup, (int, float)) and speedup >= speedup_threshold,
        f"actual={speedup} threshold={speedup_threshold}",
    )

    rss_regression = nested(benchmark_report, "rss_regression_pct_fast_vs_disabled")
    check(
        checks,
        "rss_regression_pct_fast_vs_disabled",
        isinstance(rss_regression, (int, float))
        and rss_regression <= rss_regression_threshold_pct,
        f"actual={rss_regression} threshold={rss_regression_threshold_pct}",
    )

    loads = nested(benchmark_report, "loads_per_run")
    check(
        checks,
        "loads_per_run_material",
        isinstance(loads, int) and loads >= 100_000_000,
        f"loads_per_run={loads}",
    )

    fast_gc_cycles = nested(benchmark_report, "gc_trace_fast", "gc_cycles")
    disabled_gc_cycles = nested(benchmark_report, "gc_trace_disabled", "gc_cycles")
    check(
        checks,
        "typed_row_no_gc_cycles",
        fast_gc_cycles == 0 and disabled_gc_cycles == 0,
        f"fast_gc_cycles={fast_gc_cycles} disabled_gc_cycles={disabled_gc_cycles}",
    )

    success_rate = nested(benchmark_report, "gc_trace_fast", "copied_minor_success_rate")
    eligible = nested(benchmark_report, "gc_trace_fast", "copied_minor_eligible_cycles")
    check(
        checks,
        "copied_minor_not_claimed_without_eligible_cycles",
        (eligible == 0 and success_rate is None)
        or (isinstance(success_rate, (int, float)) and success_rate >= 0.99),
        f"eligible={eligible} success_rate={success_rate}",
    )

    fast_slow_path_static = nested(
        benchmark_report,
        "static_pressure",
        "perry_fast",
        "buffer_slow_path_accesses_static",
    )
    disabled_slow_path_static = nested(
        benchmark_report,
        "static_pressure",
        "perry_disabled_fast_path_baseline",
        "buffer_slow_path_accesses_static",
    )
    slow_path_reduction_static = nested(
        benchmark_report, "static_pressure", "typed_array_slow_path_reduction_static"
    )
    fast_element_helpers_static = nested(
        benchmark_report,
        "static_pressure",
        "perry_fast",
        "typed_array_element_helper_calls_static",
    )
    disabled_element_helpers_static = nested(
        benchmark_report,
        "static_pressure",
        "perry_disabled_fast_path_baseline",
        "typed_array_element_helper_calls_static",
    )
    element_helper_reduction_static = nested(
        benchmark_report,
        "static_pressure",
        "typed_array_element_helper_reduction_static",
    )
    fast_boxed_allocations_static = nested(
        benchmark_report,
        "static_pressure",
        "perry_fast",
        "boxed_number_allocations_static",
    )
    disabled_boxed_allocations_static = nested(
        benchmark_report,
        "static_pressure",
        "perry_disabled_fast_path_baseline",
        "boxed_number_allocations_static",
    )
    check(
        checks,
        "typedarray_static_slow_path_pressure_reduced",
        fast_slow_path_static == 0
        and isinstance(disabled_slow_path_static, int)
        and disabled_slow_path_static > 0
        and isinstance(slow_path_reduction_static, int)
        and slow_path_reduction_static > 0,
        (
            f"fast={fast_slow_path_static} disabled={disabled_slow_path_static} "
            f"reduction={slow_path_reduction_static}"
        ),
    )
    check(
        checks,
        "typedarray_static_element_helpers_eliminated",
        fast_element_helpers_static == 0
        and isinstance(disabled_element_helpers_static, int)
        and disabled_element_helpers_static > 0
        and isinstance(element_helper_reduction_static, int)
        and element_helper_reduction_static > 0,
        (
            f"fast={fast_element_helpers_static} "
            f"disabled={disabled_element_helpers_static} "
            f"reduction={element_helper_reduction_static}"
        ),
    )
    check(
        checks,
        "typedarray_static_boxed_allocations_fast_zero",
        fast_boxed_allocations_static == 0,
        (
            f"fast={fast_boxed_allocations_static} "
            f"disabled={disabled_boxed_allocations_static}"
        ),
    )

    structural_checks = nested(compiler_report, "checks") or []
    by_name = {
        row.get("name"): row
        for row in structural_checks
        if isinstance(row, dict) and row.get("name")
    }
    for required in (
        "typedarray_param_data_ptr_hoisted",
        "typedarray_param_direct_f64_load",
        "typedarray_param_inner_scan_loop_no_metadata_calls",
        "native_reps_required_typedarray_param_f64_read",
        "runtime_budget_gc_collections_traced",
        "runtime_budget_boxed_number_allocations_static",
        "runtime_budget_buffer_slow_path_accesses_static",
    ):
        row = by_name.get(required)
        check(
            checks,
            required,
            isinstance(row, dict) and row.get("status") == "pass",
            row.get("detail", "missing") if isinstance(row, dict) else "missing",
        )

    errors = [row for row in checks if row["severity"] == "error" and row["status"] != "pass"]
    status = "pass" if not errors else "fail"
    return {
        "status": status,
        "checks": checks,
        "errors": [f"{row['name']}: {row['detail']}" for row in errors],
        "summary": {
            "runs": nested(benchmark_report, "runs"),
            "node_binary": nested(benchmark_report, "node_binary"),
            "node_version": nested(benchmark_report, "node_version"),
            "speedup_threshold": speedup_threshold,
            "rss_regression_threshold_pct": rss_regression_threshold_pct,
            "speedup_fast_vs_disabled": speedup,
            "rss_regression_pct_fast_vs_disabled": rss_regression,
            "loads_per_run": loads,
            "fast_median_ms": nested(benchmark_report, "perry_fast", "median_ms"),
            "disabled_median_ms": nested(
                benchmark_report, "perry_disabled_fast_path_baseline", "median_ms"
            ),
            "node_median_ms": nested(benchmark_report, "node_reference", "median_ms"),
            "fast_throughput_million_loads_per_s": nested(
                benchmark_report, "perry_fast", "throughput_million_loads_per_s"
            ),
            "disabled_throughput_million_loads_per_s": nested(
                benchmark_report,
                "perry_disabled_fast_path_baseline",
                "throughput_million_loads_per_s",
            ),
            "node_throughput_million_loads_per_s": nested(
                benchmark_report, "node_reference", "throughput_million_loads_per_s"
            ),
            "checksum": next(iter(checksums)) if len(checksums) == 1 else None,
            "offset_subarray_checksum": offset_checksum,
            "fast_gc_cycles": fast_gc_cycles,
            "copied_minor_eligible_cycles": eligible,
            "copied_minor_success_rate": success_rate,
            "fast_buffer_slow_path_accesses_static": fast_slow_path_static,
            "disabled_buffer_slow_path_accesses_static": disabled_slow_path_static,
            "typed_array_slow_path_reduction_static": slow_path_reduction_static,
            "fast_typed_array_element_helpers_static": fast_element_helpers_static,
            "disabled_typed_array_element_helpers_static": disabled_element_helpers_static,
            "typed_array_element_helper_reduction_static": element_helper_reduction_static,
            "fast_boxed_number_allocations_static": fast_boxed_allocations_static,
            "disabled_boxed_number_allocations_static": disabled_boxed_allocations_static,
        },
    }


def markdown(packet: dict[str, Any]) -> str:
    summary = packet.get("summary", {})
    lines = [
        f"# Type-Lowering Evidence Packet: {packet.get('status', 'unknown').upper()}",
        "",
        f"- Generated: `{packet.get('generated_at')}`",
        f"- Head: `{nested(packet, 'source_state', 'head_sha')}`",
        f"- Dirty source snapshot: `{nested(packet, 'source_state', 'dirty')}`",
        f"- Workload: `{packet.get('workload')}`",
        f"- Runs: `{summary.get('runs', 'missing')}`",
        f"- Node reference: `{summary.get('node_binary', 'missing')}` `{summary.get('node_version', 'missing')}`",
        "",
        "## Material Row",
        "",
        "| Metric | Value |",
        "|---|---:|",
        f"| Loads/run | {summary.get('loads_per_run', 'missing')} |",
        f"| Node median ms | {summary.get('node_median_ms', 'missing')} |",
        f"| Perry fast median ms | {summary.get('fast_median_ms', 'missing')} |",
        f"| Perry disabled median ms | {summary.get('disabled_median_ms', 'missing')} |",
        f"| Node M loads/sec | {summary.get('node_throughput_million_loads_per_s', 'missing')} |",
        f"| Perry fast M loads/sec | {summary.get('fast_throughput_million_loads_per_s', 'missing')} |",
        f"| Perry disabled M loads/sec | {summary.get('disabled_throughput_million_loads_per_s', 'missing')} |",
        f"| Speedup vs disabled | {summary.get('speedup_fast_vs_disabled', 'missing')} |",
        f"| Speedup threshold | {summary.get('speedup_threshold', 'missing')} |",
        f"| RSS regression pct | {summary.get('rss_regression_pct_fast_vs_disabled', 'missing')} |",
        f"| RSS regression threshold pct | {summary.get('rss_regression_threshold_pct', 'missing')} |",
        f"| Checksum | {summary.get('checksum', 'missing')} |",
        f"| Offset/subarray checksum | {summary.get('offset_subarray_checksum', 'missing')} |",
        f"| Fast GC cycles | {summary.get('fast_gc_cycles', 'missing')} |",
        f"| Fast slow-path helper calls | {summary.get('fast_buffer_slow_path_accesses_static', 'missing')} |",
        f"| Disabled slow-path helper calls | {summary.get('disabled_buffer_slow_path_accesses_static', 'missing')} |",
        f"| Slow-path helper call reduction | {summary.get('typed_array_slow_path_reduction_static', 'missing')} |",
        f"| Fast typed-array element helpers | {summary.get('fast_typed_array_element_helpers_static', 'missing')} |",
        f"| Disabled typed-array element helpers | {summary.get('disabled_typed_array_element_helpers_static', 'missing')} |",
        f"| Fast boxed-number allocations | {summary.get('fast_boxed_number_allocations_static', 'missing')} |",
        "",
        "## Gate Summary",
        "",
    ]
    for row in packet.get("checks", []):
        prefix = "PASS" if row.get("status") == "pass" else "FAIL"
        lines.append(f"- {prefix}: {row.get('name')}: {row.get('detail')}")
    if packet.get("errors"):
        lines.extend(["", "## Errors", ""])
        lines.extend(f"- {error}" for error in packet["errors"])
    lines.append("")
    return "\n".join(lines)


def build_packet(args: argparse.Namespace) -> tuple[dict[str, Any], Path, Path]:
    out_dir = Path(args.out)
    if not out_dir.is_absolute():
        out_dir = REPO_ROOT / out_dir
    out_dir.mkdir(parents=True, exist_ok=True)

    build = run_release_build(args, out_dir)
    compiler = run_compiler_gate(args, out_dir)
    benchmark = run_ab_benchmark(args, out_dir)
    evaluation = evaluate_packet(
        build,
        compiler,
        benchmark,
        speedup_threshold=args.speedup_threshold,
        rss_regression_threshold_pct=args.rss_regression_threshold_pct,
    )
    packet = {
        "schema_version": 1,
        "generated_at": utc_now(),
        "workload": args.workload,
        "source_state": source_state(out_dir),
        "release_build": build,
        "compiler_output": {
            key: value
            for key, value in compiler.items()
            if key not in {"manifest", "structural_report", "summary"}
        },
        "typedarray_param_lowering": {
            key: value for key, value in benchmark.items() if key != "report"
        },
        **evaluation,
    }
    json_path = out_dir / "type-lowering-evidence.json"
    md_path = out_dir / "type-lowering-evidence.md"
    json_path.write_text(json.dumps(packet, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    md_path.write_text(markdown(packet), encoding="utf-8")
    return packet, json_path, md_path


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Generate a gated evidence packet for Perry type-lowering improvements."
    )
    parser.add_argument("--out", default=str(DEFAULT_OUT))
    parser.add_argument("--workload", default=DEFAULT_WORKLOAD)
    parser.add_argument("--runs", type=int, default=3, help="A/B benchmark samples")
    parser.add_argument("--compiler-runs", type=int, default=1)
    parser.add_argument("--benchmark-mode", default="smoke")
    parser.add_argument("--perry")
    parser.add_argument("--timeout", type=int, default=600)
    parser.add_argument("--speedup-threshold", type=float, default=DEFAULT_SPEEDUP_THRESHOLD)
    parser.add_argument(
        "--rss-regression-threshold-pct",
        type=float,
        default=DEFAULT_RSS_REGRESSION_THRESHOLD_PCT,
    )
    parser.add_argument("--gate", action="store_true")
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    if args.runs < 1 or args.compiler_runs < 1:
        parser.error("--runs and --compiler-runs must be positive")
    packet, json_path, md_path = build_packet(args)
    print(f"packet markdown: {md_path}")
    print(f"packet json:     {json_path}")
    if args.gate and packet["status"] != "pass":
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
