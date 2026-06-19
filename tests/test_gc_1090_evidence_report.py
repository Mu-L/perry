import importlib.util
import hashlib
import json
import shutil
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "gc_1090_evidence_report.py"

SPEC = importlib.util.spec_from_file_location("gc_1090_evidence_report", SCRIPT_PATH)
assert SPEC is not None
REPORT = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(REPORT)


REQUIRED_BENCHMARKS = (
    "bench_json_roundtrip",
    "bench_gc_pressure",
    "07_object_create",
    "12_binary_trees",
)


def write_json(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")


def copied_workload(
    *,
    fallback_reason="none",
    ineligible_cycles=0,
    conservative_pinned_bytes=0,
    compiled_frame_conservative_pinned_bytes=0,
    conservative_stack_truncated_cycles=0,
    conservative_stack_unbounded_cycles=0,
    copy_only_pinned_bytes=0,
    copy_only_young_roots=0,
    copy_only_malloc_roots=0,
    unattributed_roots=0,
    malloc_registry_rebuilds=0,
    non_minor_cycles=0,
    phase_sweep_us=0,
    phase_block_persistence_us=0,
    phase_root_marking_us=0,
    phase_trace_worklist_us=0,
    phase_reference_rewrite_us=0,
    block_persist_iterations=0,
    block_persist_candidate_blocks=0,
    block_persist_live_blocks=0,
    block_persist_marked_objects=0,
    root_growth_mutable_first=1,
    root_growth_mutable_max=1,
    root_growth_registered_first=0,
    root_growth_registered_max=0,
    remembered_set_stale_entries=0,
    pause_us=10,
    dirty_pages_scanned=0,
    dirty_slots_scanned=0,
    old_objects_considered=0,
    mutable_root_slots_scanned=1,
    mutable_registered_slots_scanned=0,
    external_live_bytes_last=0,
    external_live_bytes_max=0,
    external_cache_reserved_bytes_last=0,
    external_registered_bytes=0,
    external_finalized_bytes=0,
    external_owner_moves=0,
    external_young_owner_count_max=0,
    external_young_owner_checks=0,
):
    counts = {reason: 0 for reason in REPORT.FALLBACK_REASONS}
    counts[fallback_reason] = 1
    return {
        "fallback_reason_counts": counts,
        "conservative_pinned_bytes": conservative_pinned_bytes,
        "compiled_frame_conservative_pinned_bytes": (
            compiled_frame_conservative_pinned_bytes
        ),
        "conservative_stack": {
            "truncated_cycles": conservative_stack_truncated_cycles,
            "unbounded_cycles": conservative_stack_unbounded_cycles,
        },
        "legacy_copy_only_scanner_pinned": {
            "bytes": copy_only_pinned_bytes,
            "emitted_young_roots": copy_only_young_roots,
            "emitted_malloc_roots": copy_only_malloc_roots,
            "sources": {
                "unattributed": {"emitted_roots": unattributed_roots}
            },
        },
        "copying_nursery": {
            "copied_objects": 1,
            "copied_bytes": 16,
            "promoted_objects": 0,
            "promoted_bytes": 0,
            "malloc_registry_rebuilds": malloc_registry_rebuilds,
            "ineligible_cycles": ineligible_cycles,
        },
        "non_minor_cycles": non_minor_cycles,
        "phase_us": {
            "sweep": phase_sweep_us,
            "block_persistence": phase_block_persistence_us,
            "root_marking": phase_root_marking_us,
            "trace_worklist": phase_trace_worklist_us,
            "reference_rewrite": phase_reference_rewrite_us,
        },
        "block_persist": {
            "iterations": block_persist_iterations,
            "candidate_blocks": block_persist_candidate_blocks,
            "live_blocks": block_persist_live_blocks,
            "marked_objects": block_persist_marked_objects,
        },
        "root_growth": {
            "mutable_slots_scanned": {
                "first": root_growth_mutable_first,
                "last": root_growth_mutable_max,
                "min": root_growth_mutable_first,
                "max": root_growth_mutable_max,
            },
            "mutable_registered_slots_scanned": {
                "first": root_growth_registered_first,
                "last": root_growth_registered_max,
                "min": root_growth_registered_first,
                "max": root_growth_registered_max,
            },
        },
        "pause_us": pause_us,
        "remembered_set": {
            "stale_entries": remembered_set_stale_entries,
            "dirty_old_pages_before": 0,
            "external_dirty_slot_pages_before": 0,
            "external_dirty_entries_before": 0,
            "dirty_pages_scanned": dirty_pages_scanned,
            "dirty_slots_scanned": dirty_slots_scanned,
            "old_objects_considered": old_objects_considered,
        },
        "external_memory": {
            "live_bytes": {
                "first": external_live_bytes_last,
                "last": external_live_bytes_last,
                "min": 0,
                "max": external_live_bytes_max,
            },
            "cache_reserved_bytes": {
                "first": external_cache_reserved_bytes_last,
                "last": external_cache_reserved_bytes_last,
                "min": 0,
                "max": external_cache_reserved_bytes_last,
            },
            "young_owner_count": {
                "first": external_young_owner_count_max,
                "last": external_young_owner_count_max,
                "min": 0,
                "max": external_young_owner_count_max,
            },
            "registered_bytes": external_registered_bytes,
            "finalized_bytes": external_finalized_bytes,
            "owner_moves": external_owner_moves,
            "copied_minor_young_owner_checks": external_young_owner_checks,
            "kinds": {},
        },
        "mutable_roots": {
            "slots_scanned": mutable_root_slots_scanned,
            "nonzero_slots": 1 if mutable_root_slots_scanned else 0,
            "pointer_roots": 1 if mutable_root_slots_scanned else 0,
            "rewritten_slots": 1 if mutable_root_slots_scanned else 0,
            "shadow_slots_scanned": 1 if mutable_root_slots_scanned else 0,
            "global_slots_scanned": 0,
            "registered_slots_scanned": mutable_registered_slots_scanned,
            "metadata_slots_scanned": 0,
        },
    }


def copied_report(**overrides):
    workloads = {
        name: copied_workload()
        for name in REPORT.STRICT_COPIED_MINOR_WORKLOADS
    }
    workloads.update(overrides)
    fallback_reason_counts = {reason: 0 for reason in REPORT.FALLBACK_REASONS}
    for workload in workloads.values():
        counts = workload.get("fallback_reason_counts", {})
        if not isinstance(counts, dict):
            continue
        for reason, count in counts.items():
            if isinstance(reason, str):
                fallback_reason_counts[reason] = fallback_reason_counts.get(
                    reason, 0
                ) + REPORT.int_value(count)
    external_registered_bytes = sum(
        REPORT.nested(workload, "external_memory", "registered_bytes", default=0)
        for workload in workloads.values()
    )
    external_finalized_bytes = sum(
        REPORT.nested(workload, "external_memory", "finalized_bytes", default=0)
        for workload in workloads.values()
    )
    external_owner_moves = sum(
        REPORT.nested(workload, "external_memory", "owner_moves", default=0)
        for workload in workloads.values()
    )
    external_young_owner_checks = sum(
        REPORT.nested(
            workload,
            "external_memory",
            "copied_minor_young_owner_checks",
            default=0,
        )
        for workload in workloads.values()
    )
    external_live_bytes_last = max(
        REPORT.nested(workload, "external_memory", "live_bytes", "last", default=0)
        for workload in workloads.values()
    )
    external_live_bytes_max = max(
        REPORT.nested(workload, "external_memory", "live_bytes", "max", default=0)
        for workload in workloads.values()
    )
    external_cache_reserved_bytes_last = max(
        REPORT.nested(workload, "external_memory", "cache_reserved_bytes", "last", default=0)
        for workload in workloads.values()
    )
    external_young_owner_count_max = max(
        REPORT.nested(workload, "external_memory", "young_owner_count", "max", default=0)
        for workload in workloads.values()
    )
    return {
        "summary": {
            "cycles": len(workloads),
            "fallback_reason_counts": fallback_reason_counts,
            "conservative_pinned_bytes": 0,
            "compiled_frame_conservative_pinned_bytes": 0,
            "conservative_stack": {
                "truncated_cycles": 0,
                "unbounded_cycles": 0,
            },
            "legacy_copy_only_scanner_pinned": {
                "bytes": 0,
                "emitted_young_roots": 0,
                "emitted_malloc_roots": 0,
                "sources": {"unattributed": {"emitted_roots": 0}},
            },
            "copying_nursery": {
                "copied_objects": len(workloads),
                "copied_bytes": len(workloads) * 16,
                "promoted_objects": 0,
                "promoted_bytes": 0,
                "malloc_registry_rebuilds": 0,
            },
            "external_memory": {
                "live_bytes": {
                    "first": 0,
                    "last": external_live_bytes_last,
                    "min": 0,
                    "max": external_live_bytes_max,
                },
                "cache_reserved_bytes": {
                    "first": 0,
                    "last": external_cache_reserved_bytes_last,
                    "min": 0,
                    "max": external_cache_reserved_bytes_last,
                },
                "young_owner_count": {
                    "first": 0,
                    "last": external_young_owner_count_max,
                    "min": 0,
                    "max": external_young_owner_count_max,
                },
                "registered_bytes": external_registered_bytes,
                "finalized_bytes": external_finalized_bytes,
                "owner_moves": external_owner_moves,
                "copied_minor_young_owner_checks": external_young_owner_checks,
            },
            "remembered_set": {
                "stale_entries": 0,
                "dirty_old_pages_before": 0,
                "external_dirty_slot_pages_before": 0,
                "external_dirty_entries_before": 0,
                "dirty_pages_scanned": 0,
                "dirty_slots_scanned": 0,
                "old_objects_considered": 0,
            },
            "mutable_roots": {
                "slots_scanned": len(workloads),
                "registered_slots_scanned": 0,
            },
        },
        "scaling": {},
        "workloads": workloads,
    }


def target_report():
    return {
        "summary": {
            "cycles": 1,
            "fallback_reason_counts": {"none": 1},
            "copying_nursery": {
                "copied_objects": 1,
                "copied_bytes": 16,
                "promoted_objects": 0,
                "promoted_bytes": 0,
                "malloc_registry_rebuilds": 0,
            },
            "old_page_accounting": {},
        }
    }


def benchmark_report(
    multiplier=1,
    correctness="pass",
    *,
    reference="node",
    actual_lines=None,
    expected_lines=None,
    include_node=True,
    node_reference=None,
):
    if actual_lines is None:
        actual_lines = ["checksum:1"]
    if expected_lines is None:
        expected_lines = ["checksum:1"]
    benchmarks = {}
    for name in REQUIRED_BENCHMARKS:
        entry = {
            "perry_ms": 100 * multiplier,
            "perry_rss_kb": 100_000 * multiplier,
            "correctness": {
                "status": correctness,
                "reference": reference,
                "reason": "matched",
                "actual_lines": list(actual_lines),
                "expected_lines": list(expected_lines),
            },
        }
        if include_node:
            entry["node_ms"] = 120 * multiplier
            entry["node_rss_kb"] = 120_000 * multiplier
        benchmarks[name] = entry
    if node_reference is None:
        if include_node:
            node_reference = {
                "available": True,
                "binary": "/usr/bin/node",
                "command": ["/usr/bin/node", "--experimental-strip-types"],
                "input_mode": "experimental-strip-types",
                "tried": ["/usr/bin/node"],
            }
        else:
            node_reference = {
                "available": False,
                "binary": None,
                "command": [],
                "input_mode": None,
                "tried": ["/missing/node"],
                "error": "no usable Node.js runtime found for benchmark references",
            }
    return {"commit": "abc", "node_reference": node_reference, "benchmarks": benchmarks}


def trace_cycle(
    fallback_reason="none",
    *,
    collection_kind="minor",
    eligible=True,
    trigger_kind="arena_bytes",
    copied_objects=1,
    copied_bytes=16,
    promoted_objects=0,
    promoted_bytes=0,
):
    return {
        "event": "gc_cycle",
        "collection_kind": collection_kind,
        "trigger": {"kind": trigger_kind},
        "copying_nursery": {
            "eligible": eligible,
            "fallback_reason": fallback_reason,
            "copied_objects": copied_objects,
            "copied_bytes": copied_bytes,
            "promoted_objects": promoted_objects,
            "promoted_bytes": promoted_bytes,
        },
    }


def first_fallback_reason(workload):
    counts = workload.get("fallback_reason_counts", {})
    if not isinstance(counts, dict):
        return "none"
    for reason, count in counts.items():
        if isinstance(reason, str) and isinstance(count, int) and count > 0:
            return reason
    return "none"


def write_copied_minor_traces(root, label, report, *, cycles_per_workload=10):
    traces_root = root / label / "memory" / "traces" / "copied-minor-fallback"
    if traces_root.exists():
        shutil.rmtree(traces_root)
    traces_root.mkdir(parents=True, exist_ok=True)
    workloads = report.get("workloads", {}) if isinstance(report, dict) else {}
    for index, (name, workload) in enumerate(sorted(workloads.items()), start=1):
        reason = first_fallback_reason(workload if isinstance(workload, dict) else {})
        copying = REPORT.nested(workload, "copying_nursery", default={})
        cycle = trace_cycle(
            reason,
            eligible=reason == "none",
            copied_objects=REPORT.int_value(copying.get("copied_objects"))
            if isinstance(copying, dict)
            else 0,
            copied_bytes=REPORT.int_value(copying.get("copied_bytes"))
            if isinstance(copying, dict)
            else 0,
            promoted_objects=REPORT.int_value(copying.get("promoted_objects"))
            if isinstance(copying, dict)
            else 0,
            promoted_bytes=REPORT.int_value(copying.get("promoted_bytes"))
            if isinstance(copying, dict)
            else 0,
        )
        path = traces_root / f"{index:03d}_{name}.log"
        with path.open("w", encoding="utf-8") as handle:
            for _ in range(cycles_per_workload):
                handle.write(json.dumps(cycle))
                handle.write("\n")


def write_default_expanded_dominance_traces(root, label):
    write_trace_group(
        root,
        label,
        "required-benchmark",
        {
            name: [trace_cycle() for _ in range(20)]
            for name in REQUIRED_BENCHMARKS
        },
    )
    write_trace_group(
        root,
        label,
        "target-collector",
        {
            "default_copying": [trace_cycle() for _ in range(40)],
            "string_heavy": [trace_cycle() for _ in range(20)],
        },
    )
    write_trace_group(
        root,
        label,
        "copied-minor-scaling",
        {
            "young_only_1x": [trace_cycle() for _ in range(10)],
            "young_only_2x": [trace_cycle() for _ in range(10)],
            "young_only_4x": [trace_cycle() for _ in range(10)],
            "young_only_8x": [trace_cycle() for _ in range(10)],
        },
    )


def write_trace_group(root, label, group, traces):
    traces_root = root / label / "memory" / "traces" / group
    if traces_root.exists():
        shutil.rmtree(traces_root)
    traces_root.mkdir(parents=True, exist_ok=True)
    for index, (name, cycles) in enumerate(sorted(traces.items()), start=1):
        path = traces_root / f"{index:03d}_{name}.log"
        with path.open("w", encoding="utf-8") as handle:
            for cycle in cycles:
                handle.write(json.dumps(cycle))
                handle.write("\n")


def write_benchmark_trace_correctness(root, label):
    trace_root = root / label / "benchmark-gc-traces"
    for subdir in ("stdout", "reference-stdout", "correctness"):
        (trace_root / subdir).mkdir(parents=True, exist_ok=True)
    for name in REQUIRED_BENCHMARKS:
        actual_lines = ["checksum:1"]
        expected_lines = ["checksum:1"]
        (trace_root / "stdout" / f"{name}.out").write_text(
            "\n".join(actual_lines) + "\n",
            encoding="utf-8",
        )
        (trace_root / "reference-stdout" / f"{name}.out").write_text(
            "\n".join(expected_lines) + "\n",
            encoding="utf-8",
        )
        write_json(
            trace_root / "correctness" / f"{name}.json",
            {
                "status": "pass",
                "reference": "node",
                "reason": "matched 1 semantic line(s)",
                "actual_lines": actual_lines,
                "expected_lines": expected_lines,
            },
        )


def perf_frontier_packet():
    classifications = {
        name: {
            "class": "numeric-representation-bound",
            "reasons": ["synthetic"],
            "evidence": {},
        }
        for name in REQUIRED_BENCHMARKS
    }
    return {
        "schema_version": 1,
        "status": "pass",
        "errors": [],
        "warnings": [],
        "classification": classifications,
        "profile_summary": {
            "status": "pass",
            "row": "class_method_no_field_access",
            "top_non_gc_costs": [
                {"symbol": "js_object_get_own_field_or_undef", "samples": 10}
            ],
        },
        "baseline": {
            "input_path": "tmp/perf-frontier-baseline.json",
            "baseline_sha": "c" * 40,
            "present": True,
        },
    }


def type_lowering_packet(**summary_overrides):
    summary = {
        "speedup_fast_vs_disabled": 8.3,
        "speedup_threshold": 8.0,
        "rss_regression_pct_fast_vs_disabled": 0.5,
        "loads_per_run": 131_072_000,
        "fast_median_ms": 95,
        "disabled_median_ms": 791,
        "node_median_ms": 79,
        "checksum": "6323324000",
        "offset_subarray_checksum": "98",
        "fast_gc_cycles": 0,
        "copied_minor_eligible_cycles": 0,
        "copied_minor_success_rate": None,
        "fast_buffer_slow_path_accesses_static": 0,
        "disabled_buffer_slow_path_accesses_static": 2,
        "typed_array_slow_path_reduction_static": 2,
        "fast_typed_array_element_helpers_static": 0,
        "disabled_typed_array_element_helpers_static": 2,
        "typed_array_element_helper_reduction_static": 2,
        "fast_boxed_number_allocations_static": 0,
        "disabled_boxed_number_allocations_static": 0,
    }
    summary.update(summary_overrides)
    return {
        "schema_version": 1,
        "status": "pass",
        "errors": [],
        "summary": summary,
    }


def gc_store_inventory_packet(**summary_overrides):
    summary = {
        "annotations": 61,
        "audited_sites": 76,
        "files_scanned": 333,
        "unaudited_sites": 0,
        "invalid_annotations": 0,
        "stale_annotations": 0,
        "missing_gc_type_metadata": 0,
        "duplicate_gc_type_metadata": 0,
    }
    summary.update(summary_overrides)
    return {
        "schema_version": 1,
        "status": "pass" if all(
            summary.get(field, 0) == 0
            for field in (
                "unaudited_sites",
                "invalid_annotations",
                "stale_annotations",
                "missing_gc_type_metadata",
                "duplicate_gc_type_metadata",
            )
        ) else "fail",
        "summary": summary,
        "errors": [],
    }


def old_page_policy_packet(
    *,
    base_peak_kb=120_000,
    head_peak_kb=90_000,
    base_retained_kb=120_000,
    head_retained_kb=90_000,
    checksum=42,
    structural=True,
    churn_samples=None,
):
    if churn_samples is None:
        churn_samples = [100_000, 104_000, 106_000, 107_000, 108_000, 109_000]
    old_page = {
        "selected_pages": 1 if structural else 0,
        "old_page_scanned_objects": 2 if structural else 0,
        "old_page_moved_objects": 1 if structural else 0,
        "old_page_moved_bytes": 64 if structural else 0,
        "released_original_bytes": 64 if structural else 0,
        "released_original_reusable_bytes": 128 if structural else 0,
        "released_original_returned_bytes": 0,
        "reusable_bytes": 128 if structural else 0,
        "returned_bytes": 0,
    }
    return {
        "schema_version": 1,
        "bench_json_roundtrip_retained": {
            "base": {
                "checksum": checksum,
                "peak_rss_kb": base_peak_kb,
                "retained_rss_kb": base_retained_kb,
                "trace_path": "base.trace",
                "old_page": {},
            },
            "head": {
                "checksum": checksum,
                "peak_rss_kb": head_peak_kb,
                "retained_rss_kb": head_retained_kb,
                "trace_path": "head.trace",
                "old_page": old_page,
            },
        },
        "old_gen_churn_retained": {
            "samples_rss_kb": churn_samples,
            "warmup_samples": 2,
            "plateau_allowance_kb": REPORT.OLD_GEN_CHURN_PLATEAU_ALLOWANCE_KB,
            "old_page": {},
        },
    }


class Gc1090EvidenceReportTests(unittest.TestCase):
    def make_root(
        self,
        *,
        head_copied=None,
        base_benchmarks=None,
        head_benchmarks=None,
        head_memory_failed=0,
    ):
        temp = tempfile.TemporaryDirectory()
        root = Path(temp.name)
        metadata = {
            "base_ref": "origin/main",
            "head_ref": "HEAD",
            "base_sha": "a" * 40,
            "head_sha": "b" * 40,
            "source_state": {
                "mode": "exact-ref",
                "original_head_sha": "b" * 40,
                "current_head_sha": "b" * 40,
                "tested_head_sha": "b" * 40,
                "dirty": False,
                "status_porcelain_v1": [],
            },
            "commands": {
                "base": {
                    "build": {"status": "pass", "exit_code": 0},
                    "memory_stability": {"status": "pass", "exit_code": 0},
                    "benchmarks": {"status": "pass", "exit_code": 0},
                    "benchmark_gc_traces": {"status": "pass", "exit_code": 0},
                },
                "head": {
                    "build": {"status": "pass", "exit_code": 0},
                    "memory_stability": {"status": "pass", "exit_code": 0},
                    "benchmarks": {"status": "pass", "exit_code": 0},
                    "benchmark_gc_traces": {"status": "pass", "exit_code": 0},
                },
            },
        }
        write_json(root / "metadata.json", metadata)
        for label in ("base", "head"):
            copied = (
                head_copied
                if label == "head" and head_copied is not None
                else copied_report()
            )
            write_json(
                root / label / "memory" / "reports" / "memory_stability_summary.json",
                {
                    "script": "run_memory_stability_tests.sh",
                    "passed": 58,
                    "failed": head_memory_failed if label == "head" else 0,
                    "skipped": 0,
                },
            )
            write_json(
                root / label / "memory" / "reports" / "copied_minor_fallback_report.json",
                copied,
            )
            write_copied_minor_traces(root, label, copied)
            write_default_expanded_dominance_traces(root, label)
            write_benchmark_trace_correctness(root, label)
            write_json(
                root / label / "memory" / "reports" / "target_collector_gates_report.json",
                target_report(),
            )
            write_json(
                root / label / "benchmarks" / "full.json",
                (
                    head_benchmarks
                    if label == "head" and head_benchmarks is not None
                    else base_benchmarks
                    if label == "base" and base_benchmarks is not None
                    else benchmark_report()
                ),
            )
        return temp, root

    def add_perf_frontier(self, root, *, old_page_policy=True, old_page_policy_data=None):
        metadata = json.loads((root / "metadata.json").read_text(encoding="utf-8"))
        metadata.setdefault("commands", {}).setdefault("packet", {})["perf_frontier"] = {
            "status": "pass",
            "exit_code": 0,
        }
        metadata.setdefault("commands", {}).setdefault("packet", {})["gc_store_inventory"] = {
            "status": "pass",
            "exit_code": 0,
        }
        metadata.setdefault("commands", {}).setdefault("packet", {})["type_lowering"] = {
            "status": "pass",
            "exit_code": 0,
        }
        if old_page_policy:
            metadata.setdefault("commands", {}).setdefault("packet", {})["old_page_policy"] = {
                "status": "pass",
                "exit_code": 0,
            }
        write_json(root / "metadata.json", metadata)
        write_json(root / "perf-frontier" / "perf-frontier-packet.json", perf_frontier_packet())
        write_json(root / "gc-store-site-inventory.json", gc_store_inventory_packet())
        write_json(root / "type-lowering" / "type-lowering-evidence.json", type_lowering_packet())
        if old_page_policy:
            write_json(
                root / "old-page-policy.json",
                old_page_policy_data
                if old_page_policy_data is not None
                else old_page_policy_packet(),
            )

    def collect(self, **kwargs):
        temp, root = self.make_root(**kwargs)
        self.addCleanup(temp.cleanup)
        return REPORT.collect_report(root, "base", "head")

    def collect_gate(self, **kwargs):
        temp, root = self.make_root(**kwargs)
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        return REPORT.collect_report(root, "base", "head", gate=True)

    def test_pass_case(self):
        packet = self.collect()
        self.assertEqual(packet["status"], "pass")
        self.assertEqual(packet["errors"], [])

    def test_main_writes_packet_files(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        exit_code = REPORT.main(["--root", str(root)])
        self.assertEqual(exit_code, 0)
        self.assertTrue((root / "gc-1090-packet.json").exists())
        self.assertTrue((root / "gc-1090-packet.md").exists())
        packet = json.loads((root / "gc-1090-packet.json").read_text(encoding="utf-8"))
        self.assertEqual(packet["status"], "pass")
        self.assertIn("# #1090 GC Evidence Packet: PASS", (root / "gc-1090-packet.md").read_text(encoding="utf-8"))

    def test_fails_conservative_stack(self):
        packet = self.collect(
            head_copied=copied_report(
                json_roundtrip=copied_workload(fallback_reason="conservative_stack")
            )
        )
        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any("fallback reasons other than none" in error for error in packet["errors"])
        )

    def test_fails_conservative_pinned_bytes(self):
        packet = self.collect(
            head_copied=copied_report(
                json_roundtrip=copied_workload(conservative_pinned_bytes=8)
            )
        )
        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any("conservative_pinned_bytes=8" in error for error in packet["errors"])
        )

    def test_fails_compiled_frame_pinned_bytes(self):
        packet = self.collect(
            head_copied=copied_report(
                json_roundtrip=copied_workload(
                    compiled_frame_conservative_pinned_bytes=8
                )
            )
        )
        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(
                "compiled_frame_conservative_pinned_bytes=8" in error
                for error in packet["errors"]
            )
        )

    def test_fails_truncated_unbounded_or_unattributed_roots(self):
        packet = self.collect(
            head_copied=copied_report(
                json_roundtrip=copied_workload(
                    conservative_stack_truncated_cycles=1,
                    conservative_stack_unbounded_cycles=1,
                    unattributed_roots=1,
                    copy_only_young_roots=1,
                    copy_only_malloc_roots=1,
                )
            )
        )
        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any("conservative_stack_truncated cycles=1" in error for error in packet["errors"])
        )
        self.assertTrue(
            any("conservative_stack_unbounded cycles=1" in error for error in packet["errors"])
        )
        self.assertTrue(
            any("unattributed root scanner emitted roots=1" in error for error in packet["errors"])
        )
        self.assertTrue(
            any("emitted_young_roots=1" in error for error in packet["errors"])
        )
        self.assertTrue(
            any("emitted_malloc_roots=1" in error for error in packet["errors"])
        )

    def test_fails_benchmark_correctness(self):
        packet = self.collect(head_benchmarks=benchmark_report(correctness="fail"))
        self.assertEqual(packet["status"], "fail")
        self.assertTrue(any("correctness failed" in error for error in packet["errors"]))

    def test_gate_fails_malformed_benchmark_correctness_artifact(self):
        head = benchmark_report()
        head["benchmarks"]["bench_json_roundtrip"]["correctness"] = "malformed"

        packet = self.collect_gate(head_benchmarks=head)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any("correctness output missing" in error for error in packet["errors"])
        )

    def test_fails_memory_stability(self):
        packet = self.collect(head_memory_failed=1)
        self.assertEqual(packet["status"], "fail")
        self.assertTrue(any("memory stability failed=1" in error for error in packet["errors"]))

    def test_gate_includes_perf_frontier_fields(self):
        packet = self.collect_gate()
        self.assertEqual(packet["status"], "pass")
        self.assertIn("tool_versions", packet)
        dominance = packet["copied_minor_trace_evidence"]["head"]["dominance"]
        self.assertEqual(dominance["included_groups"], [
            "copied-minor-fallback",
            "required-benchmark",
            "target-collector",
            "copied-minor-scaling",
        ])
        self.assertEqual(dominance["dominance_cycle_count"], 240)
        self.assertEqual(dominance["eligible_minor_collections"], 240)
        self.assertEqual(dominance["copied_nursery_successes"], 240)
        self.assertEqual(dominance["copied_minor_success_rate_pct"], 100.0)
        self.assertEqual(packet["gc_store_inventory"]["status"], "pass")
        self.assertEqual(packet["gc_store_inventory"]["summary"]["unaudited_sites"], 0)
        self.assertEqual(packet["type_lowering"]["status"], "pass")
        self.assertEqual(
            packet["type_lowering"]["summary"]["offset_subarray_checksum"], "98"
        )
        self.assertGreaterEqual(
            packet["type_lowering"]["summary"]["loads_per_run"], 100_000_000
        )
        self.assertEqual(packet["old_page_policy"]["status"], "pass")
        self.assertEqual(
            packet["old_page_policy"]["bench_json_roundtrip"]["rss_gate"],
            "pass",
        )
        self.assertEqual(packet["perf_frontier"]["status"], "pass")
        self.assertIn("bench_json_roundtrip", packet["perf_frontier"]["classification"])
        self.assertEqual(
            packet["perf_frontier"]["baseline"]["input_path"],
            "tmp/perf-frontier-baseline.json",
        )

    def test_gate_fails_missing_gc_trace_artifacts(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        shutil.rmtree(root / "head" / "memory" / "traces")

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any("GC trace artifacts are missing" in error for error in packet["errors"])
        )

    def test_gate_fails_missing_type_lowering_packet(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        (root / "type-lowering" / "type-lowering-evidence.json").unlink()

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(
                "type-lowering evidence packet is missing" in error
                for error in packet["errors"]
            )
        )

    def test_gate_fails_type_lowering_without_offset_checksum(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        write_json(
            root / "type-lowering" / "type-lowering-evidence.json",
            type_lowering_packet(offset_subarray_checksum=None),
        )

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(
                "type-lowering offset/subarray checksum parity summary is missing"
                in error
                for error in packet["errors"]
            )
        )

    def test_gate_fails_type_lowering_below_material_speedup(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        write_json(
            root / "type-lowering" / "type-lowering-evidence.json",
            type_lowering_packet(speedup_fast_vs_disabled=7.99),
        )

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(
                "type-lowering speedup below material threshold" in error
                for error in packet["errors"]
            ),
            packet["errors"],
        )

    def test_gate_fails_type_lowering_missing_speedup_threshold(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        packet_data = type_lowering_packet()
        del packet_data["summary"]["speedup_threshold"]
        write_json(root / "type-lowering" / "type-lowering-evidence.json", packet_data)

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(
                "type-lowering speedup threshold metadata is missing" in error
                for error in packet["errors"]
            ),
            packet["errors"],
        )

    def test_gate_fails_type_lowering_weak_packet_threshold(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        write_json(
            root / "type-lowering" / "type-lowering-evidence.json",
            type_lowering_packet(speedup_threshold=2.0),
        )

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(
                "type-lowering packet speedup threshold below 8.0x" in error
                for error in packet["errors"]
            ),
            packet["errors"],
        )

    def test_gate_fails_type_lowering_missing_static_helper_pressure(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        packet_data = type_lowering_packet(
            fast_buffer_slow_path_accesses_static=None,
            disabled_buffer_slow_path_accesses_static=None,
            typed_array_slow_path_reduction_static=None,
        )
        write_json(root / "type-lowering" / "type-lowering-evidence.json", packet_data)

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(
                "static slow-path helper pressure proof is missing" in error
                for error in packet["errors"]
            ),
            packet["errors"],
        )

    def test_gate_fails_missing_required_benchmark_trace_evidence(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        shutil.rmtree(root / "head" / "memory" / "traces" / "required-benchmark")

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(
                "required GC trace evidence group 'required-benchmark' is missing" in error
                for error in packet["errors"]
            )
        )

    def test_gate_fails_missing_copied_minor_scaling_trace_evidence(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        shutil.rmtree(root / "head" / "memory" / "traces" / "copied-minor-scaling")

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(
                "required GC trace evidence group 'copied-minor-scaling' is missing"
                in error
                for error in packet["errors"]
            )
        )

    def test_gate_fails_partial_required_benchmark_trace_rows(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        write_trace_group(
            root,
            "head",
            "required-benchmark",
            {"bench_json_roundtrip": [trace_cycle() for _ in range(120)]},
        )

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(
                "required benchmark GC trace row 'bench_gc_pressure' is missing"
                in error
                for error in packet["errors"]
            )
        )

    def test_gate_fails_schema_light_dominance_trace_cycles(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        schema_light_cycle = {
            "event": "gc_cycle",
            "trigger": {"kind": "arena_bytes"},
            "copying_nursery": {
                "fallback_reason": "none",
                "copied_objects": 1,
                "copied_bytes": 16,
            },
        }
        write_trace_group(
            root,
            "head",
            "copied-minor-scaling",
            {"young_only_1x": [schema_light_cycle for _ in range(120)]},
        )

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any("missing collection_kind" in error for error in packet["errors"])
        )
        self.assertTrue(
            any("missing copying_nursery.eligible" in error for error in packet["errors"])
        )

    def test_trace_dominance_counts_automatic_failures_and_excludes_direct_manual(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        trace_path = Path(temp.name) / "dominance.log"
        cycles = [
            trace_cycle(trigger_kind="arena_bytes"),
            trace_cycle(
                eligible=False,
                trigger_kind="arena_bytes",
                copied_objects=0,
                copied_bytes=0,
            ),
            trace_cycle(
                "not_attempted",
                eligible=False,
                trigger_kind="malloc_count",
                copied_objects=0,
                copied_bytes=0,
            ),
            trace_cycle(
                collection_kind="full",
                trigger_kind="arena_bytes",
                copied_objects=0,
                copied_bytes=0,
            ),
            trace_cycle(trigger_kind="survivor_promotion_bytes"),
            trace_cycle(
                "not_attempted",
                collection_kind="full",
                eligible=False,
                trigger_kind="direct",
                copied_objects=0,
                copied_bytes=0,
            ),
            trace_cycle(
                "conservative_stack",
                eligible=False,
                trigger_kind="manual",
                copied_objects=0,
                copied_bytes=0,
            ),
        ]
        with trace_path.open("w", encoding="utf-8") as handle:
            for cycle in cycles:
                handle.write(json.dumps(cycle))
                handle.write("\n")

        summary = REPORT.summarize_gc_trace_file(trace_path)

        self.assertEqual(summary["gc_cycle_count"], 7)
        self.assertEqual(summary["excluded_non_automatic_cycles"], 2)
        self.assertEqual(summary["dominance_cycle_count"], 5)
        self.assertEqual(summary["copied_nursery_successes"], 1)
        self.assertEqual(summary["dominance_failures"], 4)
        self.assertEqual(summary["minor_fallback_failures"], 3)
        self.assertEqual(summary["full_gc_failures"], 1)
        self.assertEqual(summary["not_attempted_failures"], 1)
        self.assertEqual(summary["survivor_promotion_handoff_failures"], 1)
        self.assertEqual(summary["copied_minor_success_rate_pct"], 20.0)

    def test_gate_fails_automatic_full_not_attempted_and_survivor_promotion_handoffs(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        benchmark_cycles = [trace_cycle() for _ in range(96)]
        benchmark_cycles.extend(
            [
                trace_cycle(
                    collection_kind="full",
                    trigger_kind="arena_bytes",
                    copied_objects=0,
                    copied_bytes=0,
                ),
                trace_cycle(
                    "not_attempted",
                    eligible=False,
                    trigger_kind="malloc_count",
                    copied_objects=0,
                    copied_bytes=0,
                ),
                trace_cycle(trigger_kind="survivor_promotion_bytes"),
                trace_cycle(
                    collection_kind="full",
                    trigger_kind="malloc_count",
                    copied_objects=0,
                    copied_bytes=0,
                ),
            ]
        )
        write_trace_group(
            root,
            "head",
            "required-benchmark",
            {
                "07_object_create": [trace_cycle() for _ in range(20)],
                "12_binary_trees": [trace_cycle() for _ in range(20)],
                "bench_gc_pressure": [trace_cycle() for _ in range(20)],
                "bench_json_roundtrip": benchmark_cycles,
            },
        )

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        dominance = packet["copied_minor_trace_evidence"]["head"]["dominance"]
        self.assertEqual(dominance["dominance_cycle_count"], 320)
        self.assertEqual(dominance["copied_nursery_successes"], 316)
        self.assertEqual(dominance["full_gc_failures"], 2)
        self.assertEqual(dominance["not_attempted_failures"], 1)
        self.assertEqual(dominance["survivor_promotion_handoff_failures"], 1)
        self.assertTrue(
            any("copied-minor dominance below 99.0%" in error for error in packet["errors"])
        )

    def test_gate_fails_low_copied_minor_dominance_sample_count(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        write_trace_group(
            root,
            "head",
            "copied-minor-fallback",
            {"json_roundtrip": [trace_cycle() for _ in range(10)]},
        )
        write_trace_group(
            root,
            "head",
            "required-benchmark",
            {"bench_json_roundtrip": [trace_cycle() for _ in range(10)]},
        )
        write_trace_group(
            root,
            "head",
            "target-collector",
            {"default_copying": [trace_cycle() for _ in range(10)]},
        )
        write_trace_group(
            root,
            "head",
            "copied-minor-scaling",
            {"young_only_1x": [trace_cycle() for _ in range(10)]},
        )

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        dominance = packet["copied_minor_trace_evidence"]["head"]["dominance"]
        self.assertEqual(dominance["dominance_cycle_count"], 40)
        self.assertEqual(dominance["gate"], "insufficient")
        self.assertTrue(
            any("automatic cycle sample too small: 40 < 100" in error for error in packet["errors"])
        )

    def test_gate_fails_copied_minor_dominance_below_threshold_with_expanded_denominator(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        benchmark_cycles = [trace_cycle() for _ in range(96)]
        benchmark_cycles.extend(
            [
                trace_cycle("copy_only_roots", eligible=False, copied_objects=0, copied_bytes=0),
                trace_cycle("conservative_stack", eligible=False, copied_objects=0, copied_bytes=0),
                trace_cycle("pinned_young_root", eligible=False, copied_objects=0, copied_bytes=0),
                trace_cycle("pinned_young_transitive", eligible=False, copied_objects=0, copied_bytes=0),
            ]
        )
        write_trace_group(
            root,
            "head",
            "required-benchmark",
            {"bench_json_roundtrip": benchmark_cycles},
        )

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        dominance = packet["copied_minor_trace_evidence"]["head"]["dominance"]
        self.assertEqual(dominance["dominance_cycle_count"], 260)
        self.assertEqual(dominance["eligible_minor_collections"], 256)
        self.assertEqual(dominance["copied_nursery_successes"], 256)
        self.assertTrue(
            any("copied-minor dominance below 99.0%" in error for error in packet["errors"])
        )

    def test_gate_fails_missing_node_reference_artifact(self):
        packet = self.collect_gate(
            head_benchmarks=benchmark_report(reference="none")
        )

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any("Node reference comparison artifact missing" in error for error in packet["errors"])
        )

    def test_gate_reports_node_unavailable_once(self):
        packet = self.collect_gate(
            head_benchmarks=benchmark_report(
                correctness="unchecked",
                reference="none",
                include_node=False,
                actual_lines=[],
                expected_lines=[],
            )
        )

        self.assertEqual(packet["status"], "fail")
        node_errors = [
            error
            for error in packet["errors"]
            if "benchmark Node reference unavailable" in error
        ]
        self.assertEqual(node_errors, [
            "head: benchmark Node reference unavailable: "
            "no usable Node.js runtime found for benchmark references"
        ])
        self.assertFalse(
            any("correctness status is unchecked" in error for error in packet["errors"])
        )
        self.assertFalse(
            any("Node reference comparison artifact missing" in error for error in packet["errors"])
        )

    def test_gate_fails_mismatched_node_comparison_artifact(self):
        packet = self.collect_gate(
            head_benchmarks=benchmark_report(
                actual_lines=["checksum:2"],
                expected_lines=["checksum:1"],
            )
        )

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any("Node comparison artifact mismatch" in error for error in packet["errors"])
        )

    def test_gate_fails_missing_traced_benchmark_node_reference_stdout(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        (
            root
            / "head"
            / "benchmark-gc-traces"
            / "reference-stdout"
            / "bench_json_roundtrip.out"
        ).unlink()

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(
                "traced benchmark Node reference stdout is missing" in error
                for error in packet["errors"]
            )
        )

    def test_gate_fails_mismatched_traced_benchmark_node_comparison(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        correctness_path = (
            root
            / "head"
            / "benchmark-gc-traces"
            / "correctness"
            / "bench_json_roundtrip.json"
        )
        correctness = json.loads(correctness_path.read_text(encoding="utf-8"))
        correctness["actual_lines"] = ["checksum:2"]
        write_json(correctness_path, correctness)

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(
                "traced benchmark Node comparison artifact mismatch" in error
                for error in packet["errors"]
            )
        )

    def test_old_page_policy_retained_rss_improvement_can_pass_when_peak_does_not(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(
            root,
            old_page_policy_data=old_page_policy_packet(
                base_peak_kb=120_000,
                head_peak_kb=119_000,
                base_retained_kb=120_000,
                head_retained_kb=90_000,
            ),
        )

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "pass")
        old_page = packet["old_page_policy"]["bench_json_roundtrip"]
        self.assertEqual(old_page["peak_gate"], "fail")
        self.assertEqual(old_page["retained_gate"], "pass")
        self.assertEqual(old_page["rss_gate"], "pass")
        self.assertEqual(old_page["gate_reason"], "retained_improved")

    def test_old_page_policy_fails_without_peak_or_retained_threshold(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(
            root,
            old_page_policy_data=old_page_policy_packet(
                base_peak_kb=120_000,
                head_peak_kb=112_000,
                base_retained_kb=120_000,
                head_retained_kb=111_000,
            ),
        )

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any("neither peak RSS nor retained RSS" in error for error in packet["errors"])
        )

    def test_old_page_policy_small_baseline_uses_non_regression_with_structural_proof(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(
            root,
            old_page_policy_data=old_page_policy_packet(
                base_peak_kb=60_000,
                head_peak_kb=59_000,
                base_retained_kb=58_000,
                head_retained_kb=57_000,
            ),
        )

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "pass")
        old_page = packet["old_page_policy"]["bench_json_roundtrip"]
        self.assertTrue(old_page["small_baseline"])
        self.assertEqual(old_page["rss_gate"], "pass")
        self.assertEqual(old_page["gate_reason"], "small_baseline_non_regression")

    def test_old_page_policy_accepts_reclaimable_returned_pages(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        packet_data = old_page_policy_packet(
            base_peak_kb=120_000,
            head_peak_kb=119_000,
            base_retained_kb=120_000,
            head_retained_kb=90_000,
        )
        old_page = packet_data["bench_json_roundtrip_retained"]["head"]["old_page"]
        old_page.update({
            "reclaimable_bytes": 256 * 1024,
            "old_page_moved_bytes": 0,
            "released_original_bytes": 0,
            "released_original_reusable_bytes": 0,
            "reusable_bytes": 0,
            "returned_bytes": 256 * 1024,
        })
        self.add_perf_frontier(root, old_page_policy_data=packet_data)

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "pass")
        structural = packet["old_page_policy"]["structural_old_page"]
        self.assertEqual(structural["status"], "pass")
        self.assertTrue(
            structural["requirements"]["moved_or_reclaimable_returned_pages"]
        )

    def test_old_page_policy_missing_evidence_fails_gate(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root, old_page_policy=False)

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any("old-page policy evidence is missing" in error for error in packet["errors"])
        )

    def test_old_page_policy_fails_old_gen_churn_plateau(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(
            root,
            old_page_policy_data=old_page_policy_packet(
                churn_samples=[100_000, 101_000, 102_000, 190_000, 250_000],
            ),
        )

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any("old_gen_churn_retained RSS did not plateau" in error for error in packet["errors"])
        )

    def test_gate_requires_exact_sha_and_perf_frontier(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        metadata = json.loads((root / "metadata.json").read_text(encoding="utf-8"))
        metadata["head_sha"] = "b" * 39
        write_json(root / "metadata.json", metadata)
        packet = REPORT.collect_report(root, "base", "head", gate=True)
        self.assertEqual(packet["status"], "fail")
        self.assertTrue(any("exact 40-char SHA" in error for error in packet["errors"]))
        self.assertTrue(any("perf frontier packet is missing" in error for error in packet["errors"]))
        self.assertTrue(any("GC store-site inventory is missing" in error for error in packet["errors"]))

    def test_gate_rejects_dirty_exact_ref_source_state(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        metadata = json.loads((root / "metadata.json").read_text(encoding="utf-8"))
        metadata["source_state"] = {
            "mode": "exact-ref",
            "original_head_sha": "b" * 40,
            "current_head_sha": "b" * 40,
            "tested_head_sha": "b" * 40,
            "dirty": True,
            "status_porcelain_v1": [" M crates/perry-runtime/src/gc/barrier.rs"],
        }
        write_json(root / "metadata.json", metadata)

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any("dirty but mode is not worktree-snapshot" in error for error in packet["errors"])
        )

    def test_gate_rejects_dirty_status_even_when_dirty_flag_false(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        metadata = json.loads((root / "metadata.json").read_text(encoding="utf-8"))
        metadata["source_state"] = {
            "mode": "exact-ref",
            "original_head_sha": "b" * 40,
            "current_head_sha": "b" * 40,
            "tested_head_sha": "b" * 40,
            "dirty": False,
            "status_porcelain_v1": [" M scripts/gc_1090_evidence_report.py"],
        }
        write_json(root / "metadata.json", metadata)

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any("dirty flag does not match" in error for error in packet["errors"])
        )
        self.assertTrue(
            any("dirty but mode is not worktree-snapshot" in error for error in packet["errors"])
        )

    def test_gate_accepts_dirty_worktree_snapshot_with_hashed_diff(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        diff_bytes = b"diff --git a/untracked.txt b/untracked.txt\nnew file mode 100644\n"
        (root / "source-tested-head.patch").write_bytes(diff_bytes)
        metadata = json.loads((root / "metadata.json").read_text(encoding="utf-8"))
        metadata["source_state"] = {
            "mode": "worktree-snapshot",
            "original_head_sha": "c" * 40,
            "current_head_sha": "c" * 40,
            "tested_head_sha": "b" * 40,
            "tested_head_tree_sha": "d" * 40,
            "dirty": True,
            "status_porcelain_v1": ["?? untracked.txt"],
            "tested_head_diff_path": "source-tested-head.patch",
            "tested_head_diff_sha256": hashlib.sha256(diff_bytes).hexdigest(),
        }
        write_json(root / "metadata.json", metadata)

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "pass", packet["errors"])
        self.assertEqual(
            packet["source_state"]["tested_head_diff_sha256"],
            hashlib.sha256(diff_bytes).hexdigest(),
        )

    def test_gate_rejects_worktree_snapshot_hash_mismatch(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        (root / "source-tested-head.patch").write_bytes(b"actual")
        metadata = json.loads((root / "metadata.json").read_text(encoding="utf-8"))
        metadata["source_state"] = {
            "mode": "worktree-snapshot",
            "original_head_sha": "c" * 40,
            "current_head_sha": "c" * 40,
            "tested_head_sha": "b" * 40,
            "tested_head_tree_sha": "d" * 40,
            "dirty": True,
            "status_porcelain_v1": ["?? untracked.txt"],
            "tested_head_diff_path": "source-tested-head.patch",
            "tested_head_diff_sha256": hashlib.sha256(b"expected").hexdigest(),
        }
        write_json(root / "metadata.json", metadata)

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(any("diff hash mismatch" in error for error in packet["errors"]))

    def test_gate_rejects_missing_source_state(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        metadata = json.loads((root / "metadata.json").read_text(encoding="utf-8"))
        metadata.pop("source_state")
        write_json(root / "metadata.json", metadata)

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(any("source_state metadata is missing" in error for error in packet["errors"]))

    def test_gate_rejects_worktree_snapshot_head_mismatch(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        diff_bytes = b"diff --git a/untracked.txt b/untracked.txt\nnew file mode 100644\n"
        (root / "source-tested-head.patch").write_bytes(diff_bytes)
        metadata = json.loads((root / "metadata.json").read_text(encoding="utf-8"))
        metadata["source_state"] = {
            "mode": "worktree-snapshot",
            "original_head_sha": "c" * 40,
            "current_head_sha": "c" * 40,
            "tested_head_sha": "e" * 40,
            "tested_head_tree_sha": "d" * 40,
            "dirty": True,
            "status_porcelain_v1": ["?? untracked.txt"],
            "tested_head_diff_path": "source-tested-head.patch",
            "tested_head_diff_sha256": hashlib.sha256(diff_bytes).hexdigest(),
        }
        write_json(root / "metadata.json", metadata)

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(any("tested_head_sha does not match" in error for error in packet["errors"]))

    def test_gate_rejects_worktree_snapshot_empty_dirty_diff(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        diff_bytes = b""
        (root / "source-tested-head.patch").write_bytes(diff_bytes)
        metadata = json.loads((root / "metadata.json").read_text(encoding="utf-8"))
        metadata["source_state"] = {
            "mode": "worktree-snapshot",
            "original_head_sha": "c" * 40,
            "current_head_sha": "c" * 40,
            "tested_head_sha": "b" * 40,
            "tested_head_tree_sha": "d" * 40,
            "dirty": True,
            "status_porcelain_v1": ["A  staged-new.txt"],
            "tested_head_diff_path": "source-tested-head.patch",
            "tested_head_diff_sha256": hashlib.sha256(diff_bytes).hexdigest(),
        }
        write_json(root / "metadata.json", metadata)

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(any("empty tested head diff" in error for error in packet["errors"]))

    def test_gate_rejects_worktree_snapshot_escaping_diff_path(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        metadata = json.loads((root / "metadata.json").read_text(encoding="utf-8"))
        metadata["source_state"] = {
            "mode": "worktree-snapshot",
            "original_head_sha": "c" * 40,
            "current_head_sha": "c" * 40,
            "tested_head_sha": "b" * 40,
            "tested_head_tree_sha": "d" * 40,
            "dirty": True,
            "status_porcelain_v1": ["?? untracked.txt"],
            "tested_head_diff_path": "../source-tested-head.patch",
            "tested_head_diff_sha256": hashlib.sha256(b"outside").hexdigest(),
        }
        write_json(root / "metadata.json", metadata)

        packet = REPORT.collect_report(root, "base", "head", gate=True)

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(any("must stay inside packet root" in error for error in packet["errors"]))

    def test_gate_fails_unaudited_store_sites(self):
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.add_perf_frontier(root)
        metadata = json.loads((root / "metadata.json").read_text(encoding="utf-8"))
        metadata["commands"]["packet"]["gc_store_inventory"] = {
            "status": "fail",
            "exit_code": 1,
        }
        write_json(root / "metadata.json", metadata)
        write_json(
            root / "gc-store-site-inventory.json",
            gc_store_inventory_packet(unaudited_sites=2),
        )
        packet = REPORT.collect_report(root, "base", "head", gate=True)
        self.assertEqual(packet["status"], "fail")
        self.assertTrue(any("gc_store_inventory command status is fail" in error for error in packet["errors"]))
        self.assertTrue(any("unaudited_sites=2" in error for error in packet["errors"]))

    def test_fails_remembered_set_stale_entries(self):
        packet = self.collect(
            head_copied=copied_report(
                json_roundtrip=copied_workload(remembered_set_stale_entries=1)
            )
        )
        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any("remembered_set.stale_entries=1" in error for error in packet["errors"])
        )

    def test_reports_external_memory_and_allows_young_owner_checks(self):
        packet = self.collect(
            head_copied=copied_report(
                buffer_churn=copied_workload(
                    external_live_bytes_last=1024,
                    external_live_bytes_max=2048,
                    external_registered_bytes=4096,
                    external_finalized_bytes=3072,
                    external_owner_moves=2,
                    external_young_owner_count_max=2,
                    external_young_owner_checks=3,
                )
            )
        )

        self.assertEqual(packet["status"], "pass")
        summary = packet["copied_minor"]["head"]["summary"]
        self.assertEqual(summary["external_live_bytes_last"], 1024)
        self.assertEqual(summary["external_registered_bytes"], 4096)
        self.assertEqual(summary["external_finalized_bytes"], 3072)
        self.assertEqual(summary["external_owner_moves"], 2)
        self.assertEqual(summary["external_copied_minor_young_owner_checks"], 3)
        self.assertEqual(
            packet["strict_head_workloads"]["buffer_churn"][
                "external_copied_minor_young_owner_checks"
            ],
            3,
        )

    def test_fails_external_checks_without_young_owners(self):
        packet = self.collect(
            head_copied=copied_report(
                buffer_churn=copied_workload(
                    external_young_owner_count_max=0,
                    external_young_owner_checks=1,
                )
            )
        )

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(
                "copied-minor external young-owner checks=1 with no young external owners"
                in error
                for error in packet["errors"]
            )
        )

    def test_fails_non_minor_and_full_old_gen_work(self):
        packet = self.collect(
            head_copied=copied_report(
                json_roundtrip=copied_workload(
                    non_minor_cycles=1,
                    phase_sweep_us=5,
                    phase_block_persistence_us=7,
                    phase_root_marking_us=11,
                    phase_trace_worklist_us=13,
                    phase_reference_rewrite_us=17,
                    block_persist_iterations=1,
                    root_growth_mutable_first=1,
                    root_growth_mutable_max=10000,
                )
            )
        )

        self.assertEqual(packet["status"], "fail")
        self.assertTrue(any("non-minor gc cycles=1" in error for error in packet["errors"]))
        self.assertTrue(any("phase_us.sweep=5" in error for error in packet["errors"]))
        self.assertTrue(any("broad old-gen walk phase_us=41" in error for error in packet["errors"]))
        self.assertTrue(any("block_persistence work=" in error for error in packet["errors"]))
        self.assertTrue(any("root_growth.mutable_slots_scanned.max=10000" in error for error in packet["errors"]))


if __name__ == "__main__":
    unittest.main()
