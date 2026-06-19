#!/usr/bin/env python3
"""Build a PR-ready #1090 GC evidence packet from exact-head artifacts."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


EXACT_SHA_RE = re.compile(r"^[0-9a-f]{40}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")

REQUIRED_BENCHMARKS = (
    "bench_json_roundtrip",
    "bench_gc_pressure",
    "07_object_create",
    "12_binary_trees",
)

SCALING_FACTORS = (1, 2, 4, 8)
YOUNG_ONLY_SCALING_WORKLOADS = tuple(
    f"young_only_{factor}x" for factor in SCALING_FACTORS
)
DEAD_YOUNG_SCALING_WORKLOADS = tuple(
    f"dead_young_{factor}x" for factor in SCALING_FACTORS
)
FIXED_DIRTY_SCALING_WORKLOADS = tuple(
    f"fixed_dirty_edge_{factor}x" for factor in SCALING_FACTORS
)

STRICT_COPIED_MINOR_WORKLOADS = (
    "json_roundtrip",
    "string_churn",
    "object_property_churn",
    "mixed_request_shaping",
    "map_set_churn",
    "promise_churn",
)

OPTIONAL_COPIED_MINOR_WORKLOADS = (
    *YOUNG_ONLY_SCALING_WORKLOADS,
    *DEAD_YOUNG_SCALING_WORKLOADS,
    "simple_object_churn",
    *FIXED_DIRTY_SCALING_WORKLOADS,
    "map_object_key_identity_after_gc",
    "buffer_churn",
    "typed_array_churn",
    "huge_string_churn",
    "native_resource_owner_churn",
    "async_resource_churn",
    "async_promise_closures",
    "verify_copied_young",
    "verify_dirty_old_to_young",
    "verify_promise_churn",
)

FALLBACK_REASONS = (
    "none",
    "copy_only_roots",
    "barriers_inactive",
    "conservative_stack",
    "conservative_stack_truncated",
    "conservative_stack_unbounded",
    "unattributed_root_source",
    "malloc_registry_unavailable",
    "old_page_defrag_selected",
    "pinned_young_root",
    "pinned_young_dirty_slot",
    "pinned_young_transitive",
    "not_attempted",
)

COPIED_MINOR_FALLBACK_TRACE_GROUP = "copied-minor-fallback"
REQUIRED_BENCHMARK_TRACE_GROUP = "required-benchmark"
TARGET_COLLECTOR_TRACE_GROUP = "target-collector"
COPIED_MINOR_SCALING_TRACE_GROUP = "copied-minor-scaling"
COPIED_MINOR_DOMINANCE_TRACE_GROUPS = (
    COPIED_MINOR_FALLBACK_TRACE_GROUP,
    REQUIRED_BENCHMARK_TRACE_GROUP,
    TARGET_COLLECTOR_TRACE_GROUP,
    COPIED_MINOR_SCALING_TRACE_GROUP,
)
REQUIRED_COPIED_MINOR_DOMINANCE_TRACE_GROUPS = (
    COPIED_MINOR_FALLBACK_TRACE_GROUP,
    REQUIRED_BENCHMARK_TRACE_GROUP,
    TARGET_COLLECTOR_TRACE_GROUP,
    COPIED_MINOR_SCALING_TRACE_GROUP,
)
COPIED_MINOR_DOMINANCE_THRESHOLD_PCT = 99.0
MIN_COPIED_MINOR_DOMINANCE_ELIGIBLE_MINOR_COLLECTIONS = 100
AUTOMATIC_COPIED_MINOR_SUCCESS_TRIGGER_KINDS = ("arena_bytes", "malloc_count")
NON_AUTOMATIC_GC_TRIGGER_KINDS = ("direct", "manual")
SURVIVOR_PROMOTION_TRIGGER_KIND = "survivor_promotion_bytes"
SPEED_THRESHOLD_PCT = 15.0
MEMORY_THRESHOLD_PCT = 25.0
MIN_SPEED_DELTA_MS = 20
MIN_MEMORY_DELTA_KB = 2048
OLD_PAGE_RSS_IMPROVEMENT_PCT = 20.0
OLD_PAGE_RSS_IMPROVEMENT_KB = 10 * 1024
OLD_PAGE_BASELINE_SMALL_KB = 64 * 1024
OLD_GEN_CHURN_PLATEAU_ALLOWANCE_KB = 64 * 1024
TYPE_LOWERING_MATERIAL_SPEEDUP_THRESHOLD = 8.0
TYPE_LOWERING_MATERIAL_LOADS_PER_RUN = 100_000_000


def load_json(path: Path, default: Any = None) -> Any:
    if not path.exists():
        return default
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def write_json(path: Path, data: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        json.dump(data, handle, indent=2, sort_keys=True)
        handle.write("\n")


def nested(obj: Any, *path: str, default: Any = None) -> Any:
    cur = obj
    for key in path:
        if not isinstance(cur, dict):
            return default
        cur = cur.get(key, default)
    return cur


def int_value(value: Any) -> int:
    if isinstance(value, bool):
        return 0
    if isinstance(value, int):
        return value
    return 0


def positive_int(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value > 0


def pct_delta(base: int | None, head: int | None) -> float | None:
    if base is None or head is None or base <= 0:
        return None
    return ((head - base) / base) * 100.0


def ratio_delta(base: int | None, head: int | None) -> dict[str, Any]:
    pct = pct_delta(base, head)
    return {
        "base": base,
        "head": head,
        "delta": None if base is None or head is None else head - base,
        "delta_pct": None if pct is None else round(pct, 1),
    }


def empty_trace_totals() -> dict[str, Any]:
    return {
        "trace_file_count": 0,
        "gc_cycle_count": 0,
        "dominance_cycle_count": 0,
        "excluded_non_automatic_cycles": 0,
        "minor_collections": 0,
        "eligible_minor_collections": 0,
        "runtime_eligible_minor_collections": 0,
        "copied_nursery_successes": 0,
        "copied_objects": 0,
        "copied_bytes": 0,
        "promoted_objects": 0,
        "promoted_bytes": 0,
        "malformed_json_lines": 0,
        "missing_fallback_reason_cycles": 0,
        "missing_eligible_flag_cycles": 0,
        "missing_collection_kind_cycles": 0,
        "missing_trigger_kind_cycles": 0,
        "full_gc_failures": 0,
        "minor_fallback_failures": 0,
        "not_attempted_failures": 0,
        "survivor_promotion_handoff_failures": 0,
        "fallback_reason_counts": normalize_reason_counts({}),
        "collection_kind_counts": {},
        "trigger_kind_counts": {},
    }


def add_trace_totals(dst: dict[str, Any], src: dict[str, Any]) -> None:
    for field in (
        "trace_file_count",
        "gc_cycle_count",
        "dominance_cycle_count",
        "excluded_non_automatic_cycles",
        "minor_collections",
        "eligible_minor_collections",
        "runtime_eligible_minor_collections",
        "copied_nursery_successes",
        "copied_objects",
        "copied_bytes",
        "promoted_objects",
        "promoted_bytes",
        "malformed_json_lines",
        "missing_fallback_reason_cycles",
        "missing_eligible_flag_cycles",
        "missing_collection_kind_cycles",
        "missing_trigger_kind_cycles",
        "full_gc_failures",
        "minor_fallback_failures",
        "not_attempted_failures",
        "survivor_promotion_handoff_failures",
    ):
        dst[field] = int_value(dst.get(field)) + int_value(src.get(field))
    for reason, count in src.get("fallback_reason_counts", {}).items():
        if isinstance(reason, str):
            dst.setdefault("fallback_reason_counts", {})
            dst["fallback_reason_counts"][reason] = int_value(
                dst["fallback_reason_counts"].get(reason)
            ) + int_value(count)
    for kind, count in src.get("collection_kind_counts", {}).items():
        if isinstance(kind, str):
            dst.setdefault("collection_kind_counts", {})
            dst["collection_kind_counts"][kind] = int_value(
                dst["collection_kind_counts"].get(kind)
            ) + int_value(count)
    for kind, count in src.get("trigger_kind_counts", {}).items():
        if isinstance(kind, str):
            dst.setdefault("trigger_kind_counts", {})
            dst["trigger_kind_counts"][kind] = int_value(
                dst["trigger_kind_counts"].get(kind)
            ) + int_value(count)


def finalize_trace_totals(totals: dict[str, Any]) -> dict[str, Any]:
    dominance_cycles = int_value(totals.get("dominance_cycle_count"))
    successes = int_value(totals.get("copied_nursery_successes"))
    rate = (
        None
        if dominance_cycles == 0
        else round((successes / dominance_cycles) * 100.0, 2)
    )
    result = dict(totals)
    result["fallback_reason_counts"] = normalize_reason_counts(
        result.get("fallback_reason_counts", {})
    )
    result["collection_kind_counts"] = dict(
        sorted(result.get("collection_kind_counts", {}).items())
    )
    result["trigger_kind_counts"] = dict(
        sorted(result.get("trigger_kind_counts", {}).items())
    )
    result["dominance_failures"] = max(dominance_cycles - successes, 0)
    result["copied_minor_success_rate_pct"] = rate
    result["threshold_pct"] = COPIED_MINOR_DOMINANCE_THRESHOLD_PCT
    result["gate"] = (
        "missing"
        if dominance_cycles == 0
        else "pass"
        if rate is not None and rate >= COPIED_MINOR_DOMINANCE_THRESHOLD_PCT
        else "fail"
    )
    return result


def finalize_dominance_totals(
    totals: dict[str, Any],
    *,
    included_groups: list[str],
    missing_groups: list[str],
) -> dict[str, Any]:
    result = finalize_trace_totals(totals)
    dominance_cycles = int_value(result.get("dominance_cycle_count"))
    result["included_groups"] = included_groups
    result["missing_groups"] = missing_groups
    result["minimum_dominance_cycles"] = (
        MIN_COPIED_MINOR_DOMINANCE_ELIGIBLE_MINOR_COLLECTIONS
    )
    result["minimum_eligible_minor_collections"] = (
        MIN_COPIED_MINOR_DOMINANCE_ELIGIBLE_MINOR_COLLECTIONS
    )
    if (
        dominance_cycles
        and dominance_cycles < MIN_COPIED_MINOR_DOMINANCE_ELIGIBLE_MINOR_COLLECTIONS
    ):
        result["gate"] = "insufficient"
    return result


def command_exit(metadata: dict[str, Any], label: str, command: str) -> int | None:
    exit_code = nested(metadata, "commands", label, command, "exit_code")
    return exit_code if isinstance(exit_code, int) else None


def command_status(metadata: dict[str, Any], label: str, command: str) -> str:
    status = nested(metadata, "commands", label, command, "status")
    if isinstance(status, str):
        return status
    exit_code = command_exit(metadata, label, command)
    if exit_code is None:
        return "missing"
    return "pass" if exit_code == 0 else "fail"


def exact_sha(value: Any) -> bool:
    return isinstance(value, str) and EXACT_SHA_RE.fullmatch(value) is not None


def exact_sha256(value: Any) -> bool:
    return isinstance(value, str) and SHA256_RE.fullmatch(value) is not None


def file_sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def label_paths(root: Path, label: str) -> dict[str, Path]:
    base = root / label
    return {
        "benchmarks": base / "benchmarks" / "full.json",
        "memory_summary": base / "memory" / "reports" / "memory_stability_summary.json",
        "copied_minor": base / "memory" / "reports" / "copied_minor_fallback_report.json",
        "target_collector": base / "memory" / "reports" / "target_collector_gates_report.json",
    }


def memory_summary(root: Path, label: str) -> dict[str, Any]:
    summary = load_json(label_paths(root, label)["memory_summary"], {})
    return {
        "passed": int_value(summary.get("passed")) if isinstance(summary, dict) else 0,
        "failed": int_value(summary.get("failed")) if isinstance(summary, dict) else 0,
        "skipped": int_value(summary.get("skipped")) if isinstance(summary, dict) else 0,
        "path": str(label_paths(root, label)["memory_summary"]),
        "present": bool(summary),
    }


def benchmark_entry(benchmarks: dict[str, Any], name: str) -> dict[str, Any]:
    entry = nested(benchmarks, "benchmarks", name, default={})
    return entry if isinstance(entry, dict) else {}


def benchmark_node_unavailable(report: dict[str, Any]) -> str | None:
    node_reference = report.get("node_reference") if isinstance(report, dict) else None
    if not isinstance(node_reference, dict) or node_reference.get("available") is not False:
        return None
    error = node_reference.get("error")
    if isinstance(error, str) and error:
        return error
    tried = node_reference.get("tried")
    if isinstance(tried, list) and tried:
        return f"no usable Node.js runtime found; tried {len(tried)} candidate(s)"
    return "no usable Node.js runtime found"


def validate_node_correctness_artifact(
    report_label: str,
    name: str,
    entry: dict[str, Any],
    errors: list[str],
    warnings: list[str],
    *,
    gate: bool,
) -> None:
    correctness = entry.get("correctness")
    target = errors if gate else warnings
    if not isinstance(correctness, dict):
        return

    if correctness.get("reference") != "node":
        target.append(
            f"{report_label}:{name}: Node reference comparison artifact missing "
            f"(reference={correctness.get('reference')!r})"
        )

    actual = correctness.get("actual_lines")
    expected = correctness.get("expected_lines")
    actual_ok = isinstance(actual, list) and all(isinstance(line, str) for line in actual)
    expected_ok = isinstance(expected, list) and all(isinstance(line, str) for line in expected)
    if not actual_ok or not expected_ok:
        target.append(f"{report_label}:{name}: Node comparison lines are missing or malformed")
    elif not actual or not expected:
        target.append(f"{report_label}:{name}: Node comparison lines are empty")
    elif actual != expected:
        errors.append(
            f"{report_label}:{name}: Node comparison artifact mismatch: "
            f"expected={expected!r}; actual={actual!r}"
        )

    if not positive_int(entry.get("node_ms")):
        target.append(f"{report_label}:{name}: Node timing artifact missing")
    if not positive_int(entry.get("node_rss_kb")):
        target.append(f"{report_label}:{name}: Node RSS artifact missing")


def validate_node_line_comparison(
    *,
    report_label: str,
    name: str,
    correctness: Any,
    context: str,
    errors: list[str],
    warnings: list[str],
    gate: bool,
) -> None:
    target = errors if gate else warnings
    if not isinstance(correctness, dict):
        target.append(f"{report_label}:{name}: {context} correctness artifact is missing")
        return

    status = correctness.get("status")
    if status != "pass":
        target.append(f"{report_label}:{name}: {context} correctness status is {status}")

    if correctness.get("reference") != "node":
        target.append(
            f"{report_label}:{name}: {context} Node reference comparison artifact missing "
            f"(reference={correctness.get('reference')!r})"
        )

    actual = correctness.get("actual_lines")
    expected = correctness.get("expected_lines")
    actual_ok = isinstance(actual, list) and all(isinstance(line, str) for line in actual)
    expected_ok = isinstance(expected, list) and all(isinstance(line, str) for line in expected)
    if not actual_ok or not expected_ok:
        target.append(
            f"{report_label}:{name}: {context} Node comparison lines are missing or malformed"
        )
    elif not actual or not expected:
        target.append(f"{report_label}:{name}: {context} Node comparison lines are empty")
    elif actual != expected:
        target.append(
            f"{report_label}:{name}: {context} Node comparison artifact mismatch: "
            f"expected={expected!r}; actual={actual!r}"
        )


def benchmark_gc_trace_correctness(
    root: Path,
    label: str,
    errors: list[str],
    warnings: list[str],
    *,
    gate: bool,
) -> dict[str, Any]:
    trace_root = root / label / "benchmark-gc-traces"
    rows: dict[str, Any] = {}
    target = errors if gate else warnings

    for name in REQUIRED_BENCHMARKS:
        stdout_path = trace_root / "stdout" / f"{name}.out"
        reference_stdout_path = trace_root / "reference-stdout" / f"{name}.out"
        correctness_path = trace_root / "correctness" / f"{name}.json"
        correctness = load_json(correctness_path, None)
        row = {
            "stdout_path": str(stdout_path),
            "reference_stdout_path": str(reference_stdout_path),
            "correctness_path": str(correctness_path),
            "stdout_present": stdout_path.exists(),
            "reference_stdout_present": reference_stdout_path.exists(),
            "correctness_present": correctness_path.exists(),
            "status": nested(correctness, "status", default="missing"),
            "reference": nested(correctness, "reference", default="missing"),
            "actual_lines": nested(correctness, "actual_lines", default=[]),
            "expected_lines": nested(correctness, "expected_lines", default=[]),
        }
        rows[name] = row

        if not stdout_path.exists():
            target.append(f"{label}:{name}: traced benchmark Perry stdout is missing")
        if not reference_stdout_path.exists():
            target.append(f"{label}:{name}: traced benchmark Node reference stdout is missing")
        if not correctness_path.exists():
            target.append(f"{label}:{name}: traced benchmark correctness artifact is missing")

        validate_node_line_comparison(
            report_label=label,
            name=name,
            correctness=correctness,
            context="traced benchmark",
            errors=errors,
            warnings=warnings,
            gate=gate,
        )

    failed_rows = [
        name
        for name, row in rows.items()
        if row.get("status") != "pass"
        or row.get("reference") != "node"
        or not row.get("stdout_present")
        or not row.get("reference_stdout_present")
        or not row.get("correctness_present")
        or not row.get("actual_lines")
        or row.get("actual_lines") != row.get("expected_lines")
    ]
    return {
        "path": str(trace_root),
        "status": "fail" if failed_rows else "pass",
        "failed_rows": failed_rows,
        "rows": rows,
    }


def benchmark_matrix(
    root: Path,
    base_label: str,
    head_label: str,
    errors: list[str],
    warnings: list[str],
    *,
    gate: bool = False,
) -> dict[str, Any]:
    base = load_json(label_paths(root, base_label)["benchmarks"], {})
    head = load_json(label_paths(root, head_label)["benchmarks"], {})
    matrix: dict[str, Any] = {}

    for report_label, report in ((base_label, base), (head_label, head)):
        require_full_correctness = gate and report_label == head_label
        node_unavailable_reason = benchmark_node_unavailable(report)
        if node_unavailable_reason is not None:
            target = errors if require_full_correctness else warnings
            target.append(
                f"{report_label}: benchmark Node reference unavailable: "
                f"{node_unavailable_reason}"
            )
        if not report:
            errors.append(f"{report_label}: benchmark JSON is missing")
            continue
        for name, entry in nested(report, "benchmarks", default={}).items():
            correctness = entry.get("correctness", {})
            status = correctness.get("status") if isinstance(correctness, dict) else None
            if node_unavailable_reason is not None and status == "unchecked":
                continue
            if require_full_correctness and not isinstance(correctness, dict):
                errors.append(f"{report_label}:{name}: correctness output missing")
            elif require_full_correctness and status != "pass":
                errors.append(f"{report_label}:{name}: correctness status is {status}")
            elif status == "fail":
                errors.append(
                    f"{report_label}:{name}: correctness failed: "
                    f"{correctness.get('reason', 'semantic output mismatch')}"
                )
            elif status == "unchecked":
                warnings.append(f"{report_label}:{name}: correctness unchecked")
            validate_node_correctness_artifact(
                report_label,
                name,
                entry,
                errors,
                warnings,
                gate=require_full_correctness,
            )

    for name in REQUIRED_BENCHMARKS:
        base_entry = benchmark_entry(base, name)
        head_entry = benchmark_entry(head, name)
        if not base_entry:
            errors.append(f"{base_label}:{name}: required benchmark missing")
        if not head_entry:
            errors.append(f"{head_label}:{name}: required benchmark missing")

        base_ms = base_entry.get("perry_ms")
        head_ms = head_entry.get("perry_ms")
        base_rss = base_entry.get("perry_rss_kb")
        head_rss = head_entry.get("perry_rss_kb")
        time = ratio_delta(base_ms, head_ms)
        rss = ratio_delta(base_rss, head_rss)

        time_regression = (
            time["delta_pct"] is not None
            and time["delta_pct"] > SPEED_THRESHOLD_PCT
            and abs(time["delta"] or 0) >= MIN_SPEED_DELTA_MS
        )
        rss_regression = (
            rss["delta_pct"] is not None
            and rss["delta_pct"] > MEMORY_THRESHOLD_PCT
            and abs(rss["delta"] or 0) >= MIN_MEMORY_DELTA_KB
        )
        if time_regression:
            errors.append(
                f"{name}: time regression {base_ms}ms -> {head_ms}ms "
                f"({time['delta_pct']:+.1f}%)"
            )
        if rss_regression:
            errors.append(
                f"{name}: RSS regression {base_rss}KB -> {head_rss}KB "
                f"({rss['delta_pct']:+.1f}%)"
            )

        matrix[name] = {
            "time_ms": time,
            "rss_kb": rss,
            "base_correctness": nested(base_entry, "correctness", "status", default="missing"),
            "head_correctness": nested(head_entry, "correctness", "status", default="missing"),
            "gate": "fail" if time_regression or rss_regression else "pass",
        }

    return matrix


def normalize_reason_counts(counts: Any) -> dict[str, int]:
    result = {reason: 0 for reason in FALLBACK_REASONS}
    if isinstance(counts, dict):
        for key, value in counts.items():
            if isinstance(key, str):
                result[key] = int_value(value)
    return result


def trace_group(root: Path, label: str, trace_file: Path) -> str:
    traces_root = root / label / "memory" / "traces"
    try:
        rel = trace_file.relative_to(traces_root)
    except ValueError:
        return "_unknown"
    return rel.parts[0] if rel.parts else "_root"


def iter_trace_files(root: Path, label: str) -> list[Path]:
    traces_root = root / label / "memory" / "traces"
    if not traces_root.exists():
        return []
    return sorted(path for path in traces_root.rglob("*") if path.is_file())


def normalized_trigger_kind(value: Any) -> str | None:
    if not isinstance(value, str) or not value:
        return None
    aliases = {
        "ArenaBytes": "arena_bytes",
        "MallocCount": "malloc_count",
        "OldGenBytes": "old_gen_bytes",
        "SurvivorPromotionBytes": SURVIVOR_PROMOTION_TRIGGER_KIND,
        "Emergency": "emergency",
        "Manual": "manual",
        "Direct": "direct",
    }
    return aliases.get(value, value)


def trace_trigger_kind(event: dict[str, Any]) -> str | None:
    trigger = event.get("trigger")
    if isinstance(trigger, dict):
        kind = normalized_trigger_kind(trigger.get("kind"))
        if kind is not None:
            return kind
    return normalized_trigger_kind(event.get("trigger_kind"))


def explicitly_non_automatic_gc_cycle(
    event: dict[str, Any],
    trigger_kind: str | None,
) -> bool:
    if trigger_kind in NON_AUTOMATIC_GC_TRIGGER_KINDS:
        return True
    trigger = event.get("trigger")
    trigger = trigger if isinstance(trigger, dict) else {}
    for value in (
        event.get("automatic"),
        trigger.get("automatic"),
    ):
        if value is False:
            return True
    for value in (
        event.get("classification"),
        trigger.get("classification"),
        trigger.get("class"),
    ):
        if isinstance(value, str) and value in (
            "non_automatic",
            "non-automatic",
            "direct",
            "manual",
        ):
            return True
    return False


def summarize_gc_trace_file(trace_file: Path) -> dict[str, Any]:
    totals = empty_trace_totals()
    totals["trace_file_count"] = 1
    totals["path"] = str(trace_file)

    try:
        handle = trace_file.open("r", encoding="utf-8", errors="replace")
    except OSError as exc:
        totals["read_error"] = str(exc)
        return finalize_trace_totals(totals)

    with handle:
        for line in handle:
            line = line.strip()
            if not line.startswith("{"):
                continue
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                totals["malformed_json_lines"] += 1
                continue
            if not isinstance(event, dict) or event.get("event") != "gc_cycle":
                continue

            totals["gc_cycle_count"] += 1
            collection_kind = event.get("collection_kind")
            if isinstance(collection_kind, str):
                kind_key = collection_kind
            else:
                kind_key = "_missing"
            totals["collection_kind_counts"][kind_key] = int_value(
                totals["collection_kind_counts"].get(kind_key)
            ) + 1

            trigger_kind = trace_trigger_kind(event)
            if trigger_kind is None:
                trigger_key = "_missing"
            else:
                trigger_key = trigger_kind
            totals["trigger_kind_counts"][trigger_key] = int_value(
                totals["trigger_kind_counts"].get(trigger_key)
            ) + 1

            if explicitly_non_automatic_gc_cycle(event, trigger_kind):
                totals["excluded_non_automatic_cycles"] += 1
                continue

            totals["dominance_cycle_count"] += 1
            if trigger_kind is None:
                totals["missing_trigger_kind_cycles"] += 1

            copying = event.get("copying_nursery")
            if not isinstance(copying, dict):
                copying = {}
            reason = copying.get("fallback_reason")
            if isinstance(reason, str):
                reason_key = reason
            else:
                reason_key = "_missing"
                totals["missing_fallback_reason_cycles"] += 1

            totals["fallback_reason_counts"][reason_key] = int_value(
                totals["fallback_reason_counts"].get(reason_key)
            ) + 1

            eligible_flag = copying.get("eligible")
            is_minor = collection_kind == "minor"
            if is_minor:
                totals["minor_collections"] += 1
                if eligible_flag is True:
                    totals["eligible_minor_collections"] += 1
                    totals["runtime_eligible_minor_collections"] += 1
                elif eligible_flag is not False:
                    totals["missing_eligible_flag_cycles"] += 1
            elif not isinstance(collection_kind, str):
                totals["missing_collection_kind_cycles"] += 1
                if eligible_flag is not True and eligible_flag is not False:
                    totals["missing_eligible_flag_cycles"] += 1
            elif collection_kind == "full":
                totals["full_gc_failures"] += 1

            if trigger_kind == SURVIVOR_PROMOTION_TRIGGER_KIND:
                totals["survivor_promotion_handoff_failures"] += 1
            if reason_key == "not_attempted":
                totals["not_attempted_failures"] += 1

            success = (
                trigger_kind in AUTOMATIC_COPIED_MINOR_SUCCESS_TRIGGER_KINDS
                and is_minor
                and eligible_flag is True
                and reason_key == "none"
            )
            if success:
                totals["copied_nursery_successes"] += 1
            elif is_minor:
                totals["minor_fallback_failures"] += 1

            totals["copied_objects"] += int_value(copying.get("copied_objects"))
            totals["copied_bytes"] += int_value(copying.get("copied_bytes"))
            totals["promoted_objects"] += int_value(copying.get("promoted_objects"))
            totals["promoted_bytes"] += int_value(copying.get("promoted_bytes"))

    return finalize_trace_totals(totals)


def trace_row_name_from_path(path_value: Any) -> str | None:
    if not isinstance(path_value, str) or not path_value:
        return None
    name = Path(path_value).stem
    return re.sub(r"^\d+_", "", name)


def copied_minor_trace_evidence(root: Path, label: str) -> dict[str, Any]:
    traces_root = root / label / "memory" / "traces"
    files = iter_trace_files(root, label)
    totals = empty_trace_totals()
    groups: dict[str, dict[str, Any]] = {}
    file_summaries: list[dict[str, Any]] = []

    for trace_file in files:
        summary = summarize_gc_trace_file(trace_file)
        add_trace_totals(totals, summary)
        group_name = trace_group(root, label, trace_file)
        summary["group"] = group_name
        summary["row"] = trace_row_name_from_path(str(trace_file))
        file_summaries.append(summary)
        group_totals = groups.setdefault(group_name, empty_trace_totals())
        add_trace_totals(group_totals, summary)

    finalized_groups = {
        name: finalize_trace_totals(group_totals)
        for name, group_totals in sorted(groups.items())
    }
    dominance_totals = empty_trace_totals()
    included_dominance_groups: list[str] = []
    missing_dominance_groups: list[str] = []
    for group_name in COPIED_MINOR_DOMINANCE_TRACE_GROUPS:
        group_totals = finalized_groups.get(group_name)
        if isinstance(group_totals, dict):
            included_dominance_groups.append(group_name)
            add_trace_totals(dominance_totals, group_totals)
        else:
            missing_dominance_groups.append(group_name)

    dominance = finalize_dominance_totals(
        dominance_totals,
        included_groups=included_dominance_groups,
        missing_groups=missing_dominance_groups,
    )
    return {
        "present": bool(files),
        "path": str(traces_root),
        "dominance_groups": list(COPIED_MINOR_DOMINANCE_TRACE_GROUPS),
        "required_dominance_groups": list(REQUIRED_COPIED_MINOR_DOMINANCE_TRACE_GROUPS),
        "totals": finalize_trace_totals(totals),
        "dominance": dominance,
        "groups": finalized_groups,
        "files": file_summaries,
    }


def gate_copied_minor_dominance(
    label: str,
    trace_evidence: dict[str, Any],
    errors: list[str],
    warnings: list[str],
    *,
    gate: bool,
) -> None:
    target = errors if gate else warnings
    if not trace_evidence.get("present"):
        target.append(f"{label}: GC trace artifacts are missing")
        return

    totals = trace_evidence.get("totals", {})
    malformed = int_value(totals.get("malformed_json_lines")) if isinstance(totals, dict) else 0
    if malformed:
        target.append(f"{label}: GC trace artifacts contain malformed JSON lines={malformed}")

    dominance = trace_evidence.get("dominance", {})
    if not isinstance(dominance, dict):
        target.append(f"{label}: copied-minor dominance trace summary is missing")
        return

    groups = trace_evidence.get("groups", {})
    groups = groups if isinstance(groups, dict) else {}
    for group_name in REQUIRED_COPIED_MINOR_DOMINANCE_TRACE_GROUPS:
        group_summary = groups.get(group_name)
        if not isinstance(group_summary, dict):
            target.append(
                f"{label}: required GC trace evidence group {group_name!r} is missing"
            )
            continue
        group_dominance_cycles = int_value(group_summary.get("dominance_cycle_count"))
        if group_dominance_cycles == 0:
            target.append(
                f"{label}: required GC trace evidence group {group_name!r} "
                "has no automatic dominance cycles"
            )

    files = trace_evidence.get("files", [])
    files = files if isinstance(files, list) else []
    required_benchmark_rows: dict[str, int] = {name: 0 for name in REQUIRED_BENCHMARKS}
    for summary in files:
        if not isinstance(summary, dict):
            continue
        if summary.get("group") != REQUIRED_BENCHMARK_TRACE_GROUP:
            continue
        row = summary.get("row")
        if row in required_benchmark_rows:
            required_benchmark_rows[row] += int_value(
                summary.get("dominance_cycle_count")
            )
    for row, dominance_count in required_benchmark_rows.items():
        if dominance_count == 0:
            target.append(
                f"{label}: required benchmark GC trace row {row!r} is missing "
                "or has no automatic dominance cycles"
            )

    dominance_cycles = int_value(dominance.get("dominance_cycle_count"))
    successes = int_value(dominance.get("copied_nursery_successes"))
    rate = dominance.get("copied_minor_success_rate_pct")
    if dominance_cycles == 0:
        target.append(
            f"{label}: copied-minor dominance traces have no automatic dominance cycles"
        )
        return
    for field, description in (
        ("missing_trigger_kind_cycles", "trigger.kind"),
        ("missing_collection_kind_cycles", "collection_kind"),
        ("missing_eligible_flag_cycles", "copying_nursery.eligible"),
        ("missing_fallback_reason_cycles", "copying_nursery.fallback_reason"),
    ):
        missing = int_value(dominance.get(field))
        if missing:
            target.append(
                f"{label}: copied-minor dominance traces have {missing} cycles "
                f"missing {description}"
            )
    if dominance_cycles < MIN_COPIED_MINOR_DOMINANCE_ELIGIBLE_MINOR_COLLECTIONS:
        target.append(
            f"{label}: copied-minor dominance automatic cycle sample too small: "
            f"{dominance_cycles} < {MIN_COPIED_MINOR_DOMINANCE_ELIGIBLE_MINOR_COLLECTIONS}"
        )
    if not isinstance(rate, (int, float)) or rate < COPIED_MINOR_DOMINANCE_THRESHOLD_PCT:
        errors.append(
            f"{label}: copied-minor dominance below "
            f"{COPIED_MINOR_DOMINANCE_THRESHOLD_PCT:.1f}%: "
            f"{successes}/{dominance_cycles} automatic dominance cycles copied "
            f"({rate}%)"
        )


def copied_report_summary(root: Path, label: str) -> dict[str, Any]:
    report = load_json(label_paths(root, label)["copied_minor"], {})
    summary = report.get("summary", {}) if isinstance(report, dict) else {}
    workloads = report.get("workloads", {}) if isinstance(report, dict) else {}
    legacy_summary = (
        summary.get("legacy_copy_only_scanner_pinned", {})
        if isinstance(summary, dict)
        else {}
    )
    return {
        "present": bool(report),
        "path": str(label_paths(root, label)["copied_minor"]),
        "summary": {
            "cycles": int_value(summary.get("cycles")) if isinstance(summary, dict) else 0,
            "fallback_reason_counts": normalize_reason_counts(
                summary.get("fallback_reason_counts") if isinstance(summary, dict) else {}
            ),
            "conservative_pinned_bytes": int_value(
                summary.get("conservative_pinned_bytes") if isinstance(summary, dict) else 0
            ),
            "compiled_frame_conservative_pinned_bytes": int_value(
                summary.get("compiled_frame_conservative_pinned_bytes")
                if isinstance(summary, dict)
                else 0
            ),
            "legacy_copy_only_scanner_pinned_bytes": int_value(
                nested(summary, "legacy_copy_only_scanner_pinned", "bytes", default=0)
            ),
            "legacy_copy_only_scanner_emitted_young_roots": int_value(
                nested(
                    summary,
                    "legacy_copy_only_scanner_pinned",
                    "emitted_young_roots",
                    default=0,
                )
            ),
            "legacy_copy_only_scanner_emitted_malloc_roots": int_value(
                nested(
                    summary,
                    "legacy_copy_only_scanner_pinned",
                    "emitted_malloc_roots",
                    default=0,
                )
            ),
            "legacy_copy_only_scanner_unattributed_roots": int_value(
                nested(
                    legacy_summary,
                    "sources",
                    "unattributed",
                    "emitted_roots",
                    default=0,
                )
            ),
            "conservative_stack_truncated_cycles": int_value(
                nested(summary, "conservative_stack", "truncated_cycles", default=0)
            ),
            "conservative_stack_unbounded_cycles": int_value(
                nested(summary, "conservative_stack", "unbounded_cycles", default=0)
            ),
            "copied_objects": int_value(nested(summary, "copying_nursery", "copied_objects", default=0)),
            "copied_bytes": int_value(nested(summary, "copying_nursery", "copied_bytes", default=0)),
            "promoted_objects": int_value(nested(summary, "copying_nursery", "promoted_objects", default=0)),
            "promoted_bytes": int_value(nested(summary, "copying_nursery", "promoted_bytes", default=0)),
            "malloc_registry_rebuilds": int_value(
                nested(summary, "copying_nursery", "malloc_registry_rebuilds", default=0)
            ),
            "external_live_bytes_last": int_value(
                nested(summary, "external_memory", "live_bytes", "last", default=0)
            ),
            "external_cache_reserved_bytes_last": int_value(
                nested(
                    summary,
                    "external_memory",
                    "cache_reserved_bytes",
                    "last",
                    default=0,
                )
            ),
            "external_registered_bytes": int_value(
                nested(summary, "external_memory", "registered_bytes", default=0)
            ),
            "external_finalized_bytes": int_value(
                nested(summary, "external_memory", "finalized_bytes", default=0)
            ),
            "external_owner_moves": int_value(
                nested(summary, "external_memory", "owner_moves", default=0)
            ),
            "external_copied_minor_young_owner_checks": int_value(
                nested(
                    summary,
                    "external_memory",
                    "copied_minor_young_owner_checks",
                    default=0,
                )
            ),
            "remembered_set_stale_entries": int_value(
                nested(summary, "remembered_set", "stale_entries", default=0)
            ),
        },
        "workloads": workloads if isinstance(workloads, dict) else {},
        "scaling": report.get("scaling", {}) if isinstance(report, dict) else {},
    }


def target_collector_summary(root: Path, label: str) -> dict[str, Any]:
    report = load_json(label_paths(root, label)["target_collector"], {})
    summary = report.get("summary", {}) if isinstance(report, dict) else {}
    return {
        "present": bool(report),
        "path": str(label_paths(root, label)["target_collector"]),
        "cycles": int_value(summary.get("cycles")) if isinstance(summary, dict) else 0,
        "fallback_reason_counts": normalize_reason_counts(
            summary.get("fallback_reason_counts") if isinstance(summary, dict) else {}
        ),
        "copied_objects": int_value(nested(summary, "copying_nursery", "copied_objects", default=0)),
        "copied_bytes": int_value(nested(summary, "copying_nursery", "copied_bytes", default=0)),
        "promoted_objects": int_value(nested(summary, "copying_nursery", "promoted_objects", default=0)),
        "promoted_bytes": int_value(nested(summary, "copying_nursery", "promoted_bytes", default=0)),
        "malloc_registry_rebuilds": int_value(
            nested(summary, "copying_nursery", "malloc_registry_rebuilds", default=0)
        ),
        "external_live_bytes_last": int_value(
            nested(summary, "external_memory", "live_bytes", "last", default=0)
        ),
        "external_copied_minor_young_owner_checks": int_value(
            nested(
                summary,
                "external_memory",
                "copied_minor_young_owner_checks",
                default=0,
            )
        ),
        "old_page_accounting": summary.get("old_page_accounting", {})
        if isinstance(summary, dict)
        else {},
    }


def workload_counts(workload: dict[str, Any]) -> dict[str, Any]:
    return {
        "fallback_reason_counts": normalize_reason_counts(
            workload.get("fallback_reason_counts", {})
        ),
        "conservative_pinned_bytes": int_value(workload.get("conservative_pinned_bytes")),
        "compiled_frame_conservative_pinned_bytes": int_value(
            workload.get("compiled_frame_conservative_pinned_bytes")
        ),
        "legacy_copy_only_scanner_pinned_bytes": int_value(
            nested(workload, "legacy_copy_only_scanner_pinned", "bytes", default=0)
        ),
        "legacy_copy_only_scanner_emitted_young_roots": int_value(
            nested(
                workload,
                "legacy_copy_only_scanner_pinned",
                "emitted_young_roots",
                default=0,
            )
        ),
        "legacy_copy_only_scanner_emitted_malloc_roots": int_value(
            nested(
                workload,
                "legacy_copy_only_scanner_pinned",
                "emitted_malloc_roots",
                default=0,
            )
        ),
        "legacy_copy_only_scanner_unattributed_roots": int_value(
            nested(
                workload,
                "legacy_copy_only_scanner_pinned",
                "sources",
                "unattributed",
                "emitted_roots",
                default=0,
            )
        ),
        "conservative_stack_truncated_cycles": int_value(
            nested(workload, "conservative_stack", "truncated_cycles", default=0)
        ),
        "conservative_stack_unbounded_cycles": int_value(
            nested(workload, "conservative_stack", "unbounded_cycles", default=0)
        ),
        "malloc_registry_rebuilds": int_value(
            nested(workload, "copying_nursery", "malloc_registry_rebuilds", default=0)
        ),
        "malloc_sweep_due": int_value(
            nested(workload, "copying_nursery", "malloc_sweep_due", default=0)
        ),
        "external_live_bytes_max": int_value(
            nested(workload, "external_memory", "live_bytes", "max", default=0)
        ),
        "external_young_owner_count_max": int_value(
            nested(workload, "external_memory", "young_owner_count", "max", default=0)
        ),
        "external_copied_minor_young_owner_checks": int_value(
            nested(
                workload,
                "external_memory",
                "copied_minor_young_owner_checks",
                default=0,
            )
        ),
        "ineligible_cycles": int_value(
            nested(workload, "copying_nursery", "ineligible_cycles", default=0)
        ),
        "non_minor_cycles": int_value(
            nested(workload, "non_minor_cycles", default=0)
        ),
        "phase_sweep_us": int_value(
            nested(workload, "phase_us", "sweep", default=0)
        ),
        "phase_block_persistence_us": int_value(
            nested(workload, "phase_us", "block_persistence", default=0)
        ),
        "phase_root_marking_us": int_value(
            nested(workload, "phase_us", "root_marking", default=0)
        ),
        "phase_trace_worklist_us": int_value(
            nested(workload, "phase_us", "trace_worklist", default=0)
        ),
        "phase_reference_rewrite_us": int_value(
            nested(workload, "phase_us", "reference_rewrite", default=0)
        ),
        "block_persist_iterations": int_value(
            nested(workload, "block_persist", "iterations", default=0)
        ),
        "block_persist_candidate_blocks": int_value(
            nested(workload, "block_persist", "candidate_blocks", default=0)
        ),
        "block_persist_live_blocks": int_value(
            nested(workload, "block_persist", "live_blocks", default=0)
        ),
        "block_persist_marked_objects": int_value(
            nested(workload, "block_persist", "marked_objects", default=0)
        ),
        "mutable_root_slots_first": int_value(
            nested(workload, "root_growth", "mutable_slots_scanned", "first", default=0)
        ),
        "mutable_root_slots_max": int_value(
            nested(workload, "root_growth", "mutable_slots_scanned", "max", default=0)
        ),
        "mutable_registered_slots_first": int_value(
            nested(
                workload,
                "root_growth",
                "mutable_registered_slots_scanned",
                "first",
                default=0,
            )
        ),
        "mutable_registered_slots_max": int_value(
            nested(
                workload,
                "root_growth",
                "mutable_registered_slots_scanned",
                "max",
                default=0,
            )
        ),
        "remembered_set_stale_entries": int_value(
            nested(workload, "remembered_set", "stale_entries", default=0)
        ),
        "dirty_pages_scanned": int_value(
            nested(workload, "remembered_set", "dirty_pages_scanned", default=0)
        ),
        "dirty_slots_scanned": int_value(
            nested(workload, "remembered_set", "dirty_slots_scanned", default=0)
        ),
        "old_objects_considered": int_value(
            nested(workload, "remembered_set", "old_objects_considered", default=0)
        ),
        "mutable_root_slots_scanned": int_value(
            nested(workload, "mutable_roots", "slots_scanned", default=0)
        ),
        "mutable_registered_slots_scanned": int_value(
            nested(workload, "mutable_roots", "registered_slots_scanned", default=0)
        ),
        "copied_objects": int_value(
            nested(workload, "copying_nursery", "copied_objects", default=0)
        ),
        "promoted_objects": int_value(
            nested(workload, "copying_nursery", "promoted_objects", default=0)
        ),
        "pause_us": int_value(workload.get("pause_us")),
    }


def gate_copied_minor(
    head_copied: dict[str, Any],
    errors: list[str],
    warnings: list[str],
) -> dict[str, Any]:
    if not head_copied["present"]:
        errors.append("head: copied-minor fallback report is missing")
        return {}

    workload_results: dict[str, Any] = {}
    workloads = head_copied["workloads"]
    for name in (*STRICT_COPIED_MINOR_WORKLOADS, *OPTIONAL_COPIED_MINOR_WORKLOADS):
        workload = workloads.get(name)
        if not isinstance(workload, dict):
            if name in STRICT_COPIED_MINOR_WORKLOADS:
                errors.append(f"head:{name}: strict copied-minor workload missing")
            continue
        counts = workload_counts(workload)
        workload_results[name] = counts
        non_none = {
            reason: count
            for reason, count in counts["fallback_reason_counts"].items()
            if reason != "none" and count > 0
        }
        if non_none:
            errors.append(f"head:{name}: fallback reasons other than none: {non_none}")
        barriers_inactive = counts["fallback_reason_counts"].get("barriers_inactive", 0)
        if barriers_inactive:
            errors.append(
                f"head:{name}: barriers_inactive fallback cycles={barriers_inactive}, want 0"
            )
        if counts["ineligible_cycles"] != 0:
            errors.append(
                f"head:{name}: copied-minor ineligible cycles="
                f"{counts['ineligible_cycles']}, want 0"
            )
        if counts["remembered_set_stale_entries"] != 0:
            errors.append(
                f"head:{name}: remembered_set.stale_entries="
                f"{counts['remembered_set_stale_entries']}, want 0"
            )
        if counts["conservative_pinned_bytes"] != 0:
            errors.append(
                f"head:{name}: conservative_pinned_bytes="
                f"{counts['conservative_pinned_bytes']}, want 0"
            )
        if counts["compiled_frame_conservative_pinned_bytes"] != 0:
            errors.append(
                f"head:{name}: compiled_frame_conservative_pinned_bytes="
                f"{counts['compiled_frame_conservative_pinned_bytes']}, want 0"
            )
        if counts["conservative_stack_truncated_cycles"] != 0:
            errors.append(
                f"head:{name}: conservative_stack_truncated cycles="
                f"{counts['conservative_stack_truncated_cycles']}, want 0"
            )
        if counts["conservative_stack_unbounded_cycles"] != 0:
            errors.append(
                f"head:{name}: conservative_stack_unbounded cycles="
                f"{counts['conservative_stack_unbounded_cycles']}, want 0"
            )
        if counts["legacy_copy_only_scanner_pinned_bytes"] != 0:
            errors.append(
                f"head:{name}: legacy_copy_only_scanner_pinned.bytes="
                f"{counts['legacy_copy_only_scanner_pinned_bytes']}, want 0"
            )
        if counts["legacy_copy_only_scanner_emitted_young_roots"] != 0:
            errors.append(
                f"head:{name}: legacy_copy_only_scanner_pinned.emitted_young_roots="
                f"{counts['legacy_copy_only_scanner_emitted_young_roots']}, want 0"
            )
        if counts["legacy_copy_only_scanner_emitted_malloc_roots"] != 0:
            errors.append(
                f"head:{name}: legacy_copy_only_scanner_pinned.emitted_malloc_roots="
                f"{counts['legacy_copy_only_scanner_emitted_malloc_roots']}, want 0"
            )
        if counts["legacy_copy_only_scanner_unattributed_roots"] != 0:
            errors.append(
                f"head:{name}: unattributed root scanner emitted roots="
                f"{counts['legacy_copy_only_scanner_unattributed_roots']}, want 0"
            )
        if counts["malloc_registry_rebuilds"] != 0:
            errors.append(
                f"head:{name}: malloc_registry_rebuilds="
                f"{counts['malloc_registry_rebuilds']}, want 0"
            )
        if counts["malloc_sweep_due"] != 0:
            errors.append(
                f"head:{name}: malloc_sweep_due cycles="
                f"{counts['malloc_sweep_due']}, want 0"
            )
        if (
            counts["external_young_owner_count_max"] == 0
            and counts["external_copied_minor_young_owner_checks"] != 0
        ):
            errors.append(
                f"head:{name}: copied-minor external young-owner checks="
                f"{counts['external_copied_minor_young_owner_checks']} "
                "with no young external owners"
            )
        if counts["non_minor_cycles"] != 0:
            errors.append(
                f"head:{name}: non-minor gc cycles="
                f"{counts['non_minor_cycles']}, want 0"
            )
        if counts["phase_sweep_us"] != 0:
            errors.append(
                f"head:{name}: phase_us.sweep="
                f"{counts['phase_sweep_us']}, want 0"
            )
        old_walk_us = (
            counts["phase_root_marking_us"]
            + counts["phase_trace_worklist_us"]
            + counts["phase_reference_rewrite_us"]
        )
        if old_walk_us != 0:
            errors.append(
                f"head:{name}: broad old-gen walk phase_us={old_walk_us}, want 0"
            )
        block_persist_work = (
            counts["phase_block_persistence_us"]
            + counts["block_persist_iterations"]
            + counts["block_persist_candidate_blocks"]
            + counts["block_persist_live_blocks"]
            + counts["block_persist_marked_objects"]
        )
        if block_persist_work != 0:
            errors.append(
                f"head:{name}: block_persistence work={block_persist_work}, want 0"
            )
        for label, first_key, max_key in (
            (
                "mutable_slots_scanned",
                "mutable_root_slots_first",
                "mutable_root_slots_max",
            ),
            (
                "mutable_registered_slots_scanned",
                "mutable_registered_slots_first",
                "mutable_registered_slots_max",
            ),
        ):
            first = counts[first_key]
            max_value = counts[max_key]
            allowance = max(first * 8, first + 2048, 64)
            if max_value > allowance:
                errors.append(
                    f"head:{name}: root_growth.{label}.max={max_value} "
                    f"exceeds bounded allowance {allowance} from first={first}"
                )
        if counts["copied_objects"] + counts["promoted_objects"] == 0:
            errors.append(f"head:{name}: no copied-minor cycle copied or promoted an object")

    gate_copied_minor_scaling(workload_results, errors)

    return workload_results


def gate_copied_minor_scaling(
    workload_results: dict[str, dict[str, Any]],
    errors: list[str],
) -> None:
    if all(name in workload_results for name in DEAD_YOUNG_SCALING_WORKLOADS):
        pause_1x = workload_results["dead_young_1x"]["pause_us"]
        pause_8x = workload_results["dead_young_8x"]["pause_us"]
        if pause_1x <= 0:
            errors.append("head:dead_young_1x: pause_us must be > 0 for scaling gate")
        elif pause_8x >= pause_1x * 8:
            errors.append(
                "head:dead_young_8x: pause_us="
                f"{pause_8x} is not sublinear versus dead_young_1x={pause_1x}"
            )

    bounded_fields = (
        "dirty_pages_scanned",
        "dirty_slots_scanned",
        "old_objects_considered",
        "mutable_root_slots_scanned",
        "mutable_registered_slots_scanned",
    )
    for group_name, names in (
        ("young_only", YOUNG_ONLY_SCALING_WORKLOADS),
        ("dead_young", DEAD_YOUNG_SCALING_WORKLOADS),
        ("fixed_dirty_edge", FIXED_DIRTY_SCALING_WORKLOADS),
    ):
        if not all(name in workload_results for name in names):
            continue
        base = workload_results[names[0]]
        for field in bounded_fields:
            base_value = int_value(base.get(field))
            allowance = max(base_value + 8, base_value * 2)
            for name in names[1:]:
                value = int_value(workload_results[name].get(field))
                if value > allowance:
                    errors.append(
                        f"head:{name}: {field}={value} exceeds bounded "
                        f"{group_name} allowance {allowance} from {names[0]}={base_value}"
                    )


def perf_summary(metadata: dict[str, Any], base_label: str, head_label: str) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for label in (base_label, head_label):
        entry = nested(metadata, "commands", label, "perf_comprehensive", default={})
        if not isinstance(entry, dict):
            entry = {}
        entry = dict(entry)
        log = entry.get("log")
        outlier_lines: list[str] = []
        if isinstance(log, str) and log:
            log_path = Path(log)
            if log_path.exists():
                for line in log_path.read_text(
                    encoding="utf-8", errors="replace"
                ).splitlines():
                    lowered = line.lower()
                    if "gc" in lowered or "outlier" in lowered:
                        outlier_lines.append(line)
                    if len(outlier_lines) >= 20:
                        break
        entry["outlier_lines"] = outlier_lines
        result[label] = entry
    return result


def perf_frontier_summary(root: Path, metadata: dict[str, Any], errors: list[str], warnings: list[str], *, gate: bool) -> dict[str, Any]:
    path = root / "perf-frontier" / "perf-frontier-packet.json"
    packet = load_json(path, {})
    command = nested(metadata, "commands", "packet", "perf_frontier", default={})
    summary = {
        "present": bool(packet),
        "path": str(path),
        "command": command if isinstance(command, dict) else {},
        "status": packet.get("status") if isinstance(packet, dict) else "missing",
        "errors": packet.get("errors", []) if isinstance(packet, dict) else [],
        "warnings": packet.get("warnings", []) if isinstance(packet, dict) else [],
        "classification": packet.get("classification", {}) if isinstance(packet, dict) else {},
        "profile_summary": packet.get("profile_summary", {}) if isinstance(packet, dict) else {},
        "baseline": packet.get("baseline", {}) if isinstance(packet, dict) else {},
    }
    command_status_value = command.get("status") if isinstance(command, dict) else "missing"
    if gate and command_status_value != "pass":
        errors.append(f"packet: perf_frontier command status is {command_status_value}")
    elif command_status_value not in ("pass", "skipped"):
        warnings.append(f"packet: perf_frontier command status is {command_status_value}")
    if gate and not packet:
        errors.append("perf frontier packet is missing")
    elif not packet:
        warnings.append("perf frontier packet is missing")
    elif packet.get("status") != "pass":
        message = f"perf frontier packet status is {packet.get('status')}"
        if gate:
            errors.append(message)
        else:
            warnings.append(message)
    if gate and isinstance(packet, dict):
        profile = packet.get("profile_summary", {})
        if not isinstance(profile, dict) or not profile.get("top_non_gc_costs"):
            errors.append("perf frontier profiler attribution is missing")
        classification = packet.get("classification", {})
        for name in REQUIRED_BENCHMARKS:
            if not isinstance(classification, dict) or name not in classification:
                errors.append(f"perf frontier classification missing for {name}")
    return summary


def type_lowering_summary(
    root: Path,
    metadata: dict[str, Any],
    errors: list[str],
    warnings: list[str],
    *,
    gate: bool,
) -> dict[str, Any]:
    path = root / "type-lowering" / "type-lowering-evidence.json"
    packet = load_json(path, {})
    command = nested(metadata, "commands", "packet", "type_lowering", default={})
    command_status_value = command.get("status") if isinstance(command, dict) else "missing"
    summary = packet.get("summary", {}) if isinstance(packet, dict) else {}
    result = {
        "present": bool(packet),
        "path": str(path),
        "command": command if isinstance(command, dict) else {},
        "status": packet.get("status") if isinstance(packet, dict) else "missing",
        "errors": packet.get("errors", []) if isinstance(packet, dict) else [],
        "summary": summary if isinstance(summary, dict) else {},
    }

    if gate and command_status_value != "pass":
        errors.append(f"packet: type_lowering command status is {command_status_value}")
    elif command_status_value not in ("pass", "skipped"):
        warnings.append(f"packet: type_lowering command status is {command_status_value}")

    if gate and not packet:
        errors.append("type-lowering evidence packet is missing")
        return result
    if not packet:
        warnings.append("type-lowering evidence packet is missing")
        return result

    if packet.get("status") != "pass":
        message = f"type-lowering evidence packet status is {packet.get('status')}"
        if gate:
            errors.append(message)
        else:
            warnings.append(message)

    if gate:
        typed_summary = summary if isinstance(summary, dict) else {}
        speedup = typed_summary.get("speedup_fast_vs_disabled")
        speedup_threshold = typed_summary.get("speedup_threshold")
        loads = typed_summary.get("loads_per_run")
        fast_gc_cycles = typed_summary.get("fast_gc_cycles")
        checksum = typed_summary.get("checksum")
        offset_checksum = typed_summary.get("offset_subarray_checksum")
        fast_slow_path_static = typed_summary.get("fast_buffer_slow_path_accesses_static")
        disabled_slow_path_static = typed_summary.get(
            "disabled_buffer_slow_path_accesses_static"
        )
        slow_path_reduction_static = typed_summary.get(
            "typed_array_slow_path_reduction_static"
        )
        fast_element_helpers_static = typed_summary.get(
            "fast_typed_array_element_helpers_static"
        )
        disabled_element_helpers_static = typed_summary.get(
            "disabled_typed_array_element_helpers_static"
        )
        element_helper_reduction_static = typed_summary.get(
            "typed_array_element_helper_reduction_static"
        )
        fast_boxed_allocations_static = typed_summary.get(
            "fast_boxed_number_allocations_static"
        )
        if not isinstance(speedup_threshold, (int, float)):
            errors.append("type-lowering speedup threshold metadata is missing")
            effective_speedup_threshold = TYPE_LOWERING_MATERIAL_SPEEDUP_THRESHOLD
        else:
            effective_speedup_threshold = max(
                TYPE_LOWERING_MATERIAL_SPEEDUP_THRESHOLD, float(speedup_threshold)
            )
            if speedup_threshold < TYPE_LOWERING_MATERIAL_SPEEDUP_THRESHOLD:
                errors.append(
                    "type-lowering packet speedup threshold below "
                    f"{TYPE_LOWERING_MATERIAL_SPEEDUP_THRESHOLD:.1f}x: "
                    f"{speedup_threshold}"
                )
        if not isinstance(speedup, (int, float)) or speedup < effective_speedup_threshold:
            errors.append(
                "type-lowering speedup below material threshold: "
                f"speedup={speedup} threshold={effective_speedup_threshold:.1f}"
            )
        if not isinstance(loads, int) or loads < TYPE_LOWERING_MATERIAL_LOADS_PER_RUN:
            errors.append(f"type-lowering loads/run below material threshold: {loads}")
        if fast_gc_cycles != 0:
            errors.append(f"type-lowering fast row GC cycles={fast_gc_cycles}, want 0")
        if checksum is None:
            errors.append("type-lowering checksum parity summary is missing")
        if offset_checksum is None:
            errors.append("type-lowering offset/subarray checksum parity summary is missing")
        if not (
            fast_slow_path_static == 0
            and isinstance(disabled_slow_path_static, int)
            and disabled_slow_path_static > 0
            and isinstance(slow_path_reduction_static, int)
            and slow_path_reduction_static > 0
        ):
            errors.append(
                "type-lowering static slow-path helper pressure proof is missing "
                "or not reduced: "
                f"fast={fast_slow_path_static} disabled={disabled_slow_path_static} "
                f"reduction={slow_path_reduction_static}"
            )
        if not (
            fast_element_helpers_static == 0
            and isinstance(disabled_element_helpers_static, int)
            and disabled_element_helpers_static > 0
            and isinstance(element_helper_reduction_static, int)
            and element_helper_reduction_static > 0
        ):
            errors.append(
                "type-lowering typed-array element helper proof is missing "
                "or not eliminated: "
                f"fast={fast_element_helpers_static} "
                f"disabled={disabled_element_helpers_static} "
                f"reduction={element_helper_reduction_static}"
            )
        if fast_boxed_allocations_static != 0:
            errors.append(
                "type-lowering fast row boxed-number allocations static="
                f"{fast_boxed_allocations_static}, want 0"
            )

    return result


def gc_store_inventory_summary(
    root: Path,
    metadata: dict[str, Any],
    errors: list[str],
    warnings: list[str],
    *,
    gate: bool,
) -> dict[str, Any]:
    path = root / "gc-store-site-inventory.json"
    inventory = load_json(path, {})
    command = nested(metadata, "commands", "packet", "gc_store_inventory", default={})
    command_status_value = command.get("status") if isinstance(command, dict) else "missing"
    summary = inventory.get("summary", {}) if isinstance(inventory, dict) else {}
    result = {
        "present": bool(inventory),
        "path": str(path),
        "command": command if isinstance(command, dict) else {},
        "status": inventory.get("status") if isinstance(inventory, dict) else "missing",
        "summary": summary if isinstance(summary, dict) else {},
        "errors": inventory.get("errors", []) if isinstance(inventory, dict) else [],
    }

    if gate and command_status_value != "pass":
        errors.append(f"packet: gc_store_inventory command status is {command_status_value}")
    elif command_status_value not in ("pass", "skipped"):
        warnings.append(f"packet: gc_store_inventory command status is {command_status_value}")

    if gate and not inventory:
        errors.append("GC store-site inventory is missing")
        return result
    if not inventory:
        warnings.append("GC store-site inventory is missing")
        return result

    if inventory.get("status") != "pass":
        errors.append(f"GC store-site inventory status is {inventory.get('status')}")

    for field in (
        "unaudited_sites",
        "invalid_annotations",
        "stale_annotations",
        "missing_gc_type_metadata",
        "duplicate_gc_type_metadata",
    ):
        count = int_value(summary.get(field)) if isinstance(summary, dict) else 0
        if count != 0:
            errors.append(f"GC store-site inventory {field}={count}, want 0")

    return result


def old_page_policy_path(root: Path) -> Path:
    return root / "old-page-policy.json"


def material_improvement(delta: dict[str, Any]) -> bool:
    pct = delta.get("delta_pct")
    raw = delta.get("delta")
    return (
        pct is not None
        and raw is not None
        and pct <= -OLD_PAGE_RSS_IMPROVEMENT_PCT
        and abs(raw) >= OLD_PAGE_RSS_IMPROVEMENT_KB
    )


def material_regression(delta: dict[str, Any]) -> bool:
    pct = delta.get("delta_pct")
    raw = delta.get("delta")
    return (
        pct is not None
        and raw is not None
        and pct > 0
        and raw >= OLD_PAGE_RSS_IMPROVEMENT_KB
    )


def retained_rss_kb(entry: dict[str, Any]) -> int | None:
    value = entry.get("retained_rss_kb")
    if isinstance(value, int) and not isinstance(value, bool):
        return value
    value = entry.get("retained_rss_bytes")
    if isinstance(value, int) and not isinstance(value, bool):
        return (value + 1023) // 1024
    return None


def old_page_totals_from_policy(policy: dict[str, Any]) -> dict[str, int]:
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

    def add(source: Any) -> None:
        if not isinstance(source, dict):
            return
        aliases = {
            "reclaimable_bytes": ("reclaimable_bytes", "old_page_reclaimable_bytes"),
            "old_page_moved_bytes": ("old_page_moved_bytes",),
            "old_page_moved_objects": ("old_page_moved_objects",),
        }
        for key in totals:
            keys = aliases.get(key, (key,))
            for source_key in keys:
                value = source.get(source_key)
                if isinstance(value, int) and not isinstance(value, bool) and value > 0:
                    totals[key] += value
                    break

    for label in ("head",):
        entry = nested(policy, "bench_json_roundtrip_retained", label, default={})
        add(nested(entry, "old_page", default={}))
        add(nested(entry, "trace_totals", default={}))
    add(nested(policy, "old_gen_churn_retained", "old_page", default={}))
    add(nested(policy, "old_gen_churn_retained", "trace_totals", default={}))
    add(policy.get("head_old_page_accounting", {}))
    add(policy.get("head_structural_old_page", {}))
    return totals


def merge_old_page_totals(*sources: dict[str, Any]) -> dict[str, int]:
    totals = old_page_totals_from_policy({})
    for source in sources:
        if not isinstance(source, dict):
            continue
        for key in totals:
            value = source.get(key)
            if isinstance(value, int) and not isinstance(value, bool) and value > 0:
                totals[key] += value
    return totals


def old_page_structural_proof(
    policy: dict[str, Any],
    head_target: dict[str, Any],
) -> dict[str, Any]:
    policy_totals = old_page_totals_from_policy(policy)
    target_totals = (
        head_target.get("old_page_accounting", {}) if isinstance(head_target, dict) else {}
    )
    totals = merge_old_page_totals(policy_totals, target_totals)
    reusable_or_returned = (
        totals["released_original_reusable_bytes"]
        + totals["released_original_returned_bytes"]
        + totals["reusable_bytes"]
        + totals["returned_bytes"]
    )
    moved_and_released = totals["old_page_moved_bytes"] > 0 and totals["released_original_bytes"] > 0
    returned_or_reused = reusable_or_returned > 0
    passed = (
        totals["selected_pages"] > 0
        and returned_or_reused
        and (moved_and_released or totals["reclaimable_bytes"] > 0)
    )
    return {
        "status": "pass" if passed else "fail",
        "totals": totals,
        "requirements": {
            "selected_pages": totals["selected_pages"] > 0,
            "moved_and_released_bytes": moved_and_released,
            "reclaimable_bytes": totals["reclaimable_bytes"] > 0,
            "reusable_or_returned_bytes": returned_or_reused,
            "moved_or_reclaimable_returned_pages": moved_and_released
            or (totals["reclaimable_bytes"] > 0 and returned_or_reused),
        },
    }


def old_gen_churn_summary(policy: dict[str, Any]) -> dict[str, Any]:
    churn = policy.get("old_gen_churn_retained", {})
    if not isinstance(churn, dict):
        churn = {}
    samples = churn.get("samples_rss_kb")
    if not isinstance(samples, list):
        samples = []
    samples = [
        value for value in samples
        if isinstance(value, int) and not isinstance(value, bool) and value >= 0
    ]
    warmup = churn.get("warmup_samples", 2)
    if not isinstance(warmup, int) or isinstance(warmup, bool) or warmup < 0:
        warmup = 2
    allowance = churn.get("plateau_allowance_kb", OLD_GEN_CHURN_PLATEAU_ALLOWANCE_KB)
    if not isinstance(allowance, int) or isinstance(allowance, bool) or allowance < 0:
        allowance = OLD_GEN_CHURN_PLATEAU_ALLOWANCE_KB
    plateau_samples = samples[warmup:]
    plateau_delta = None
    if plateau_samples:
        plateau_delta = max(plateau_samples) - min(plateau_samples)
    passed = len(plateau_samples) >= 3 and plateau_delta is not None and plateau_delta <= allowance
    result = dict(churn)
    result.update({
        "samples_rss_kb": samples,
        "warmup_samples": warmup,
        "plateau_allowance_kb": allowance,
        "plateau_delta_kb": plateau_delta,
        "status": "pass" if passed else "fail",
    })
    return result


def old_page_policy_summary(
    root: Path,
    benchmarks: dict[str, Any],
    head_target: dict[str, Any],
    metadata: dict[str, Any],
    errors: list[str],
    warnings: list[str],
    *,
    gate: bool,
) -> dict[str, Any]:
    path = old_page_policy_path(root)
    policy = load_json(path, {})
    command = nested(metadata, "commands", "packet", "old_page_policy", default={})
    command_status_value = command.get("status") if isinstance(command, dict) else "missing"
    result: dict[str, Any] = {
        "present": bool(policy),
        "path": str(path),
        "command": command if isinstance(command, dict) else {},
        "status": "missing",
        "bench_json_roundtrip": {},
        "structural_old_page": {},
        "old_gen_churn_retained": {},
        "raw": policy if isinstance(policy, dict) else {},
    }

    if gate and command_status_value != "pass":
        errors.append(f"packet: old_page_policy command status is {command_status_value}")
    elif command_status_value not in ("pass", "skipped", "missing"):
        warnings.append(f"packet: old_page_policy command status is {command_status_value}")

    if not isinstance(policy, dict) or not policy:
        if gate:
            errors.append("old-page policy evidence is missing")
        else:
            warnings.append("old-page policy evidence is missing")
        return result

    bench = policy.get("bench_json_roundtrip_retained", {})
    if not isinstance(bench, dict):
        bench = {}
    base = bench.get("base", {})
    head = bench.get("head", {})
    if not isinstance(base, dict):
        base = {}
    if not isinstance(head, dict):
        head = {}

    matrix_peak = benchmarks.get("bench_json_roundtrip", {}).get("rss_kb", {})
    base_peak = base.get("peak_rss_kb")
    head_peak = head.get("peak_rss_kb")
    if not isinstance(base_peak, int) or isinstance(base_peak, bool):
        base_peak = matrix_peak.get("base") if isinstance(matrix_peak, dict) else None
    if not isinstance(head_peak, int) or isinstance(head_peak, bool):
        head_peak = matrix_peak.get("head") if isinstance(matrix_peak, dict) else None
    base_retained = retained_rss_kb(base)
    head_retained = retained_rss_kb(head)
    peak_delta = ratio_delta(base_peak, head_peak)
    retained_delta = ratio_delta(base_retained, head_retained)

    structural = old_page_structural_proof(policy, head_target)
    churn = old_gen_churn_summary(policy)
    checksum_match = base.get("checksum") is not None and base.get("checksum") == head.get("checksum")
    peak_pass = material_improvement(peak_delta)
    retained_pass = material_improvement(retained_delta)
    small_baseline = isinstance(base_peak, int) and base_peak < OLD_PAGE_BASELINE_SMALL_KB
    no_material_peak_regression = not material_regression(peak_delta)
    no_material_retained_regression = not material_regression(retained_delta)
    same_exact_ref = (
        metadata.get("base_sha") == metadata.get("head_sha")
        and exact_sha(metadata.get("base_sha"))
    )

    if same_exact_ref:
        rss_gate = no_material_peak_regression and no_material_retained_regression
        gate_reason = "same_ref_non_regression"
    elif small_baseline:
        rss_gate = (
            no_material_peak_regression
            and no_material_retained_regression
            and structural["status"] == "pass"
        )
        gate_reason = "small_baseline_non_regression"
    else:
        rss_gate = peak_pass or retained_pass
        gate_reason = "peak_improved" if peak_pass else "retained_improved" if retained_pass else "missing_improvement"

    result.update({
        "status": "pass" if rss_gate and structural["status"] == "pass" and churn["status"] == "pass" and checksum_match else "fail",
        "bench_json_roundtrip": {
            "base_checksum": base.get("checksum"),
            "head_checksum": head.get("checksum"),
            "checksum_match": checksum_match,
            "peak_rss_kb": peak_delta,
            "retained_rss_kb": retained_delta,
            "small_baseline": small_baseline,
            "peak_gate": "pass" if peak_pass else "fail",
            "retained_gate": "pass" if retained_pass else "fail",
            "rss_gate": "pass" if rss_gate else "fail",
            "gate_reason": gate_reason,
            "base_trace_path": base.get("trace_path"),
            "head_trace_path": head.get("trace_path"),
        },
        "structural_old_page": structural,
        "old_gen_churn_retained": churn,
    })

    if gate:
        if not checksum_match:
            errors.append("old-page policy bench_json_roundtrip_retained checksum mismatch")
        if not rss_gate:
            if small_baseline:
                errors.append(
                    "old-page policy RSS gate failed: base peak RSS is below 64MB, "
                    "but head did not preserve non-regression with structural proof"
                )
            else:
                errors.append(
                    "old-page policy RSS gate failed: bench_json_roundtrip improved by "
                    "neither peak RSS nor retained RSS at >=20% and >=10MB"
                )
        if structural["status"] != "pass":
            errors.append(
                "old-page policy structural proof missing selected moved-or-reclaimable "
                "and reusable/returned evidence"
            )
        if churn["status"] != "pass":
            errors.append(
                "old-page policy old_gen_churn_retained RSS did not plateau within "
                f"{churn.get('plateau_allowance_kb', OLD_GEN_CHURN_PLATEAU_ALLOWANCE_KB)}KB"
            )

    return result


def collect_report(root: Path, base_label: str, head_label: str, *, gate: bool = False) -> dict[str, Any]:
    metadata = load_json(root / "metadata.json", {})
    errors: list[str] = []
    warnings: list[str] = []
    early_abort = nested(metadata, "commands", "packet", "early_abort", default={})
    if isinstance(early_abort, dict) and early_abort.get("status") == "fail":
        reason = early_abort.get("reason")
        if isinstance(reason, str) and reason:
            errors.append(f"packet: early abort: {reason}")
        else:
            errors.append("packet: early abort")
    source_state_raw = metadata.get("source_state")
    source_state = source_state_raw if isinstance(source_state_raw, dict) else {}

    for key in ("base_sha", "head_sha"):
        if not exact_sha(metadata.get(key)):
            (errors if gate else warnings).append(
                f"metadata {key} is not an exact 40-char SHA"
            )

    if gate and not isinstance(source_state_raw, dict):
        errors.append("source_state metadata is missing or invalid")
    if isinstance(source_state_raw, dict):
        source_mode = source_state.get("mode")
        status_lines = source_state.get("status_porcelain_v1")
        status_lines_valid = isinstance(status_lines, list) and all(
            isinstance(line, str) for line in status_lines
        )
        if gate and not status_lines_valid:
            errors.append("source_state status_porcelain_v1 is missing or malformed")
        status_dirty = bool(status_lines) if status_lines_valid else bool(source_state.get("dirty"))
        source_dirty = bool(source_state.get("dirty"))
        if gate and source_dirty != status_dirty:
            errors.append(
                "source_state dirty flag does not match recorded status_porcelain_v1"
            )
        effective_dirty = source_dirty or status_dirty
        if gate and source_mode not in ("exact-ref", "worktree-snapshot"):
            errors.append(f"source_state mode is unknown: {source_mode!r}")
        if gate:
            for field in ("original_head_sha", "current_head_sha", "tested_head_sha"):
                if not exact_sha(source_state.get(field)):
                    errors.append(f"source_state {field} is not an exact 40-char SHA")
            if (
                exact_sha(source_state.get("tested_head_sha"))
                and exact_sha(metadata.get("head_sha"))
                and source_state.get("tested_head_sha") != metadata.get("head_sha")
            ):
                errors.append("source_state tested_head_sha does not match metadata head_sha")
        if gate and effective_dirty and source_mode != "worktree-snapshot":
            errors.append(
                "source_state is dirty but mode is not worktree-snapshot; "
                "gated evidence would not prove current worktree changes"
            )
        if gate and source_mode == "worktree-snapshot":
            if not exact_sha(source_state.get("tested_head_tree_sha")):
                errors.append("source_state tested_head_tree_sha is not an exact 40-char SHA")
            diff_path_value = source_state.get("tested_head_diff_path")
            diff_hash_value = source_state.get("tested_head_diff_sha256")
            if not exact_sha256(diff_hash_value):
                errors.append("source_state tested_head_diff_sha256 is missing or invalid")
            if not isinstance(diff_path_value, str) or not diff_path_value:
                errors.append("source_state tested_head_diff_path is missing")
            else:
                diff_path_parts = Path(diff_path_value).parts
                if Path(diff_path_value).is_absolute() or ".." in diff_path_parts:
                    errors.append("source_state tested_head_diff_path must stay inside packet root")
                    diff_path = None
                else:
                    diff_path = root / diff_path_value
                if diff_path is not None:
                    if effective_dirty and diff_path.exists() and diff_path.stat().st_size == 0:
                        errors.append(
                            "source_state dirty worktree snapshot has an empty tested head diff"
                        )
                    if not diff_path.exists():
                        errors.append(f"source_state tested head diff missing: {diff_path_value}")
                    elif exact_sha256(diff_hash_value):
                        actual = file_sha256(diff_path)
                        if actual != diff_hash_value:
                            errors.append(
                                "source_state tested head diff hash mismatch: "
                                f"{actual} != {diff_hash_value}"
                            )

    for label in (base_label, head_label):
        if command_exit(metadata, label, "build") not in (0, None):
            errors.append(f"{label}: release build failed")
        memory_exit = command_exit(metadata, label, "memory_stability")
        if memory_exit not in (0, None):
            errors.append(f"{label}: memory stability command failed with {memory_exit}")
        bench_exit = command_exit(metadata, label, "benchmarks")
        if bench_exit not in (0, None):
            warnings.append(
                f"{label}: benchmark command exited {bench_exit}; "
                "required benchmark gates are evaluated from JSON"
            )
        trace_status = command_status(metadata, label, "benchmark_gc_traces")
        if gate and trace_status != "pass":
            errors.append(f"{label}: benchmark_gc_traces command status is {trace_status}")
        elif trace_status not in ("pass", "skipped", "missing"):
            warnings.append(f"{label}: benchmark_gc_traces command status is {trace_status}")

    memory = {
        base_label: memory_summary(root, base_label),
        head_label: memory_summary(root, head_label),
    }
    for label, summary in memory.items():
        if not summary["present"]:
            errors.append(f"{label}: memory stability summary missing")
        if summary["failed"] != 0:
            errors.append(f"{label}: memory stability failed={summary['failed']}")

    benchmarks = benchmark_matrix(root, base_label, head_label, errors, warnings, gate=gate)
    copied_minor = {
        base_label: copied_report_summary(root, base_label),
        head_label: copied_report_summary(root, head_label),
    }
    copied_minor_traces = {
        base_label: copied_minor_trace_evidence(root, base_label),
        head_label: copied_minor_trace_evidence(root, head_label),
    }
    for label, trace_evidence in copied_minor_traces.items():
        copied_minor[label]["trace_evidence"] = trace_evidence
    benchmark_trace_correctness = {
        label: benchmark_gc_trace_correctness(
            root,
            label,
            errors,
            warnings,
            gate=gate,
        )
        for label in (base_label, head_label)
    }
    target_collector = {
        base_label: target_collector_summary(root, base_label),
        head_label: target_collector_summary(root, head_label),
    }
    strict_workloads = gate_copied_minor(copied_minor[head_label], errors, warnings)
    gate_copied_minor_dominance(
        head_label,
        copied_minor_traces[head_label],
        errors,
        warnings,
        gate=gate,
    )

    perf = perf_summary(metadata, base_label, head_label)
    gc_store_inventory = gc_store_inventory_summary(root, metadata, errors, warnings, gate=gate)
    type_lowering = type_lowering_summary(root, metadata, errors, warnings, gate=gate)
    perf_frontier = perf_frontier_summary(root, metadata, errors, warnings, gate=gate)
    old_page_policy = old_page_policy_summary(
        root,
        benchmarks,
        target_collector[head_label],
        metadata,
        errors,
        warnings,
        gate=gate,
    )

    packet = {
        "schema_version": 1,
        "generated_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "status": "fail" if errors else "pass",
        "errors": errors,
        "warnings": warnings,
        "refs": {
            "base": {
                "label": base_label,
                "ref": metadata.get("base_ref"),
                "sha": metadata.get("base_sha"),
            },
            "head": {
                "label": head_label,
                "ref": metadata.get("head_ref"),
                "sha": metadata.get("head_sha"),
            },
        },
        "source_state": source_state if isinstance(source_state, dict) else {},
        "tool_versions": metadata.get("tool_versions", {}),
        "commands": metadata.get("commands", {}),
        "memory_stability": memory,
        "benchmarks": benchmarks,
        "benchmark_gc_trace_correctness": benchmark_trace_correctness,
        "copied_minor": copied_minor,
        "copied_minor_trace_evidence": copied_minor_traces,
        "strict_head_workloads": strict_workloads,
        "target_collector": target_collector,
        "gc_store_inventory": gc_store_inventory,
        "type_lowering": type_lowering,
        "old_page_policy": old_page_policy,
        "perf_comprehensive": perf,
        "perf_frontier": perf_frontier,
    }
    return packet


def fmt_delta(entry: dict[str, Any], unit: str) -> str:
    base = entry.get("base")
    head = entry.get("head")
    delta = entry.get("delta")
    pct = entry.get("delta_pct")
    if base is None or head is None or delta is None or pct is None:
        return "missing"
    sign = "+" if delta >= 0 else ""
    return f"{base}{unit} -> {head}{unit} ({sign}{delta}{unit}, {pct:+.1f}%)"


def reason_summary(counts: dict[str, int]) -> str:
    nonzero = {key: value for key, value in counts.items() if value}
    if not nonzero:
        return "none"
    return ", ".join(f"{key}={value}" for key, value in sorted(nonzero.items()))


def render_markdown(packet: dict[str, Any]) -> str:
    status = packet["status"].upper()
    base_sha = nested(packet, "refs", "base", "sha", default="?")
    head_sha = nested(packet, "refs", "head", "sha", default="?")
    source_state = packet.get("source_state", {})
    source_mode = source_state.get("mode", "unknown") if isinstance(source_state, dict) else "unknown"
    lines = [
        f"# #1090 GC Evidence Packet: {status}",
        "",
        f"- Base: `{base_sha}`",
        f"- Head: `{head_sha}`",
        f"- Source mode: `{source_mode}`",
        f"- Generated: `{packet['generated_at']}`",
        "",
        "## Gate Summary",
    ]
    if isinstance(source_state, dict) and source_mode == "worktree-snapshot":
        lines.insert(5, f"- Original head: `{source_state.get('original_head_sha', '?')}`")
    if packet["errors"]:
        lines.extend(f"- FAIL: {error}" for error in packet["errors"])
    else:
        lines.append("- PASS: all hard gates passed")
    if packet["warnings"]:
        lines.extend(f"- WARN: {warning}" for warning in packet["warnings"])

    lines.extend(
        [
            "",
            "## Required Benchmarks",
            "",
            "| Benchmark | Correct | Time | RSS | Gate |",
            "|---|---|---:|---:|---|",
        ]
    )
    for name in REQUIRED_BENCHMARKS:
        entry = packet["benchmarks"].get(name, {})
        correct = f"{entry.get('base_correctness', '?')} -> {entry.get('head_correctness', '?')}"
        lines.append(
            f"| `{name}` | {correct} | {fmt_delta(entry.get('time_ms', {}), 'ms')} "
            f"| {fmt_delta(entry.get('rss_kb', {}), 'KB')} | {entry.get('gate', 'missing')} |"
        )

    old_page_policy = packet.get("old_page_policy", {})
    lines.extend(["", "## Old-Page Policy Evidence", ""])
    if isinstance(old_page_policy, dict):
        bench = old_page_policy.get("bench_json_roundtrip", {})
        structural = old_page_policy.get("structural_old_page", {})
        churn = old_page_policy.get("old_gen_churn_retained", {})
        lines.append(
            f"- Status: `{old_page_policy.get('status', 'missing')}` "
            f"packet: `{old_page_policy.get('path', '')}`"
        )
        if isinstance(bench, dict):
            lines.append(
                f"- `bench_json_roundtrip`: peak {fmt_delta(bench.get('peak_rss_kb', {}), 'KB')}; "
                f"retained {fmt_delta(bench.get('retained_rss_kb', {}), 'KB')}; "
                f"gate `{bench.get('rss_gate', 'missing')}` ({bench.get('gate_reason', 'missing')})"
            )
        if isinstance(structural, dict):
            totals = structural.get("totals", {})
            if isinstance(totals, dict):
                lines.append(
                    f"- Structural old-page proof: `{structural.get('status', 'missing')}` "
                    f"selected_pages={totals.get('selected_pages', 0)} "
                    f"moved_bytes={totals.get('old_page_moved_bytes', 0)} "
                    f"released_bytes={totals.get('released_original_bytes', 0)} "
                    f"reusable_or_returned={totals.get('released_original_reusable_bytes', 0) + totals.get('released_original_returned_bytes', 0) + totals.get('reusable_bytes', 0) + totals.get('returned_bytes', 0)}"
                )
        if isinstance(churn, dict):
            lines.append(
                f"- `old_gen_churn_retained`: `{churn.get('status', 'missing')}` "
                f"plateau_delta_kb={churn.get('plateau_delta_kb')} "
                f"allowance_kb={churn.get('plateau_allowance_kb', OLD_GEN_CHURN_PLATEAU_ALLOWANCE_KB)}"
            )

    lines.extend(["", "## Memory Stability", "", "| Ref | Passed | Failed | Skipped |"])
    lines.append("|---|---:|---:|---:|")
    for label, summary in packet["memory_stability"].items():
        lines.append(
            f"| `{label}` | {summary['passed']} | {summary['failed']} | {summary['skipped']} |"
        )

    lines.extend(
        [
            "",
            "## Copied-Minor Evidence",
            "",
            f"- Dominance denominator groups: "
            f"{', '.join(f'`{group}`' for group in COPIED_MINOR_DOMINANCE_TRACE_GROUPS)}; "
            f"minimum automatic dominance cycles: "
            f"`{MIN_COPIED_MINOR_DOMINANCE_ELIGIBLE_MINOR_COLLECTIONS}`",
            "",
            "| Ref | Trace Dominance Cycles | Trace Copied Success | Trace Success Rate | Fallback Reasons | Conservative Pinned Bytes | Compiled-Frame Pinned Bytes | Copy-Only Pinned Bytes | Copied/Promoted Objects | Copied/Promoted Bytes | Malloc Registry Rebuilds | External Live Bytes | External Young-Owner Checks |",
            "|---|---:|---:|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|",
        ]
    )
    for label, report in packet["copied_minor"].items():
        summary = report["summary"]
        trace = nested(report, "trace_evidence", "dominance", default={})
        copied_promoted = summary["copied_objects"] + summary["promoted_objects"]
        copied_promoted_bytes = summary["copied_bytes"] + summary["promoted_bytes"]
        trace_rate = trace.get("copied_minor_success_rate_pct") if isinstance(trace, dict) else None
        trace_rate_text = "missing" if trace_rate is None else f"{trace_rate:.2f}%"
        lines.append(
            f"| `{label}` "
            f"| {trace.get('dominance_cycle_count', 0) if isinstance(trace, dict) else 0} "
            f"| {trace.get('copied_nursery_successes', 0) if isinstance(trace, dict) else 0} "
            f"| {trace_rate_text} "
            f"| {reason_summary(summary['fallback_reason_counts'])} "
            f"| {summary['conservative_pinned_bytes']} "
            f"| {summary['compiled_frame_conservative_pinned_bytes']} "
            f"| {summary['legacy_copy_only_scanner_pinned_bytes']} "
            f"| {copied_promoted} "
            f"| {copied_promoted_bytes} "
            f"| {summary['malloc_registry_rebuilds']} "
            f"| {summary['external_live_bytes_last']} "
            f"| {summary['external_copied_minor_young_owner_checks']} |"
        )

    lines.extend(
        [
            "",
            "## Target Collector Gates",
            "",
            "| Ref | Present | Fallback Reasons | Copied Objects | Copied Bytes | Promoted Objects | Promoted Bytes | Malloc Registry Rebuilds | External Live Bytes | External Young-Owner Checks |",
            "|---|---|---|---:|---:|---:|---:|---:|---:|---:|",
        ]
    )
    for label, report in packet["target_collector"].items():
        lines.append(
            f"| `{label}` | {report['present']} "
            f"| {reason_summary(report['fallback_reason_counts'])} "
            f"| {report['copied_objects']} | {report['copied_bytes']} "
            f"| {report['promoted_objects']} | {report['promoted_bytes']} "
            f"| {report['malloc_registry_rebuilds']} "
            f"| {report['external_live_bytes_last']} "
            f"| {report['external_copied_minor_young_owner_checks']} |"
        )

    inventory = packet.get("gc_store_inventory", {})
    lines.extend(["", "## GC Store-Site Inventory", ""])
    if isinstance(inventory, dict):
        summary = inventory.get("summary", {})
        lines.append(
            f"- Status: `{inventory.get('status', 'missing')}` "
            f"audited_sites={summary.get('audited_sites', 0) if isinstance(summary, dict) else 0} "
            f"unaudited_sites={summary.get('unaudited_sites', 0) if isinstance(summary, dict) else 0} "
            f"missing_gc_type_metadata={summary.get('missing_gc_type_metadata', 0) if isinstance(summary, dict) else 0} "
            f"packet: `{inventory.get('path', '')}`"
        )

    type_lowering = packet.get("type_lowering", {})
    lines.extend(["", "## Type-Lowering Evidence", ""])
    if isinstance(type_lowering, dict):
        summary = type_lowering.get("summary", {})
        if not isinstance(summary, dict):
            summary = {}
        lines.append(
            f"- Status: `{type_lowering.get('status', 'missing')}` "
            f"packet: `{type_lowering.get('path', '')}`"
        )
        lines.append(
            f"- Typed-array parameter row: speedup="
            f"`{summary.get('speedup_fast_vs_disabled', 'missing')}` "
            f"loads/run=`{summary.get('loads_per_run', 'missing')}` "
            f"fast_gc_cycles=`{summary.get('fast_gc_cycles', 'missing')}` "
            f"checksum=`{summary.get('checksum', 'missing')}` "
            f"offset/subarray=`{summary.get('offset_subarray_checksum', 'missing')}`"
        )

    lines.extend(["", "## Perf-Comprehensive Outlier Check", ""])
    for label, perf in packet["perf_comprehensive"].items():
        status = perf.get("status", "missing") if isinstance(perf, dict) else "missing"
        reason = perf.get("reason", "") if isinstance(perf, dict) else ""
        log = perf.get("log", "") if isinstance(perf, dict) else ""
        suffix = f" ({reason})" if reason else ""
        log_part = f" log: `{log}`" if log else ""
        lines.append(f"- `{label}`: {status}{suffix}{log_part}")
        for outlier in perf.get("outlier_lines", []) if isinstance(perf, dict) else []:
            lines.append(f"  - `{outlier}`")

    frontier = packet.get("perf_frontier", {})
    lines.extend(["", "## Perf Frontier", ""])
    if isinstance(frontier, dict):
        lines.append(
            f"- Status: `{frontier.get('status', 'missing')}` packet: `{frontier.get('path', '')}`"
        )
        baseline = frontier.get("baseline", {})
        if isinstance(baseline, dict) and baseline:
            lines.append(
                f"- Baseline reference: `{baseline.get('input_path')}` "
                f"sha=`{baseline.get('baseline_sha', 'missing')}`"
            )
        profile = frontier.get("profile_summary", {})
        if isinstance(profile, dict):
            lines.append(
                f"- Profiled typed row: `{profile.get('row', 'missing')}`"
            )
            for row in profile.get("top_non_gc_costs", [])[:3]:
                if isinstance(row, dict):
                    lines.append(f"  - `{row.get('symbol')}` samples={row.get('samples')}")

    lines.append("")
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", required=True, help="Evidence output root")
    parser.add_argument("--base-label", default="base")
    parser.add_argument("--head-label", default="head")
    parser.add_argument("--json-out", help="Packet JSON path")
    parser.add_argument("--md-out", help="Packet Markdown path")
    parser.add_argument("--gate", action="store_true", help="Enable strict evidence gates")
    args = parser.parse_args(argv)

    root = Path(args.root)
    packet = collect_report(root, args.base_label, args.head_label, gate=args.gate)

    json_out = Path(args.json_out) if args.json_out else root / "gc-1090-packet.json"
    md_out = Path(args.md_out) if args.md_out else root / "gc-1090-packet.md"
    write_json(json_out, packet)
    md_out.parent.mkdir(parents=True, exist_ok=True)
    md_out.write_text(render_markdown(packet), encoding="utf-8")

    return 1 if packet["status"] == "fail" else 0


if __name__ == "__main__":
    raise SystemExit(main())
