import importlib.util
import sys
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "type_lowering_evidence_packet.py"

SPEC = importlib.util.spec_from_file_location("type_lowering_evidence_packet", SCRIPT_PATH)
assert SPEC is not None
PACKET = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = PACKET
SPEC.loader.exec_module(PACKET)


def compiler_report(status="pass"):
    return {
        "status": status,
        "structural_report": {
            "status": status,
            "checks": [
                {
                    "name": "typedarray_param_data_ptr_hoisted",
                    "status": "pass",
                    "detail": "hoisted",
                },
                {
                    "name": "typedarray_param_direct_f64_load",
                    "status": "pass",
                    "detail": "direct load",
                },
                {
                    "name": "typedarray_param_inner_scan_loop_no_metadata_calls",
                    "status": "pass",
                    "detail": "inner scan loop",
                },
                {
                    "name": "native_reps_required_typedarray_param_f64_read",
                    "status": "pass",
                    "detail": "native rep matched",
                },
                {
                    "name": "runtime_budget_gc_collections_traced",
                    "status": "pass",
                    "detail": "actual=0 maximum=0",
                },
                {
                    "name": "runtime_budget_boxed_number_allocations_static",
                    "status": "pass",
                    "detail": "actual=0 maximum=0",
                },
                {
                    "name": "runtime_budget_buffer_slow_path_accesses_static",
                    "status": "pass",
                    "detail": "actual=0 maximum=0",
                },
            ],
        },
    }


def benchmark_report(**overrides):
    report = {
        "loads_per_run": 131_072_000,
        "runs": 3,
        "node_binary": "/usr/bin/node",
        "node_version": "v20.20.2",
        "node_reference": {
            "checksum": "6323324000",
            "median_ms": 74,
            "throughput_million_loads_per_s": 1771.2,
        },
        "perry_fast": {
            "checksum": "6323324000",
            "median_ms": 93,
            "throughput_million_loads_per_s": 1409.4,
        },
        "perry_disabled_fast_path_baseline": {
            "checksum": "6323324000",
            "median_ms": 797,
            "throughput_million_loads_per_s": 164.5,
        },
        "static_pressure": {
            "perry_fast": {
                "buffer_slow_path_accesses_static": 0,
                "typed_array_element_helper_calls_static": 0,
                "boxed_number_allocations_static": 0,
            },
            "perry_disabled_fast_path_baseline": {
                "buffer_slow_path_accesses_static": 2,
                "typed_array_element_helper_calls_static": 2,
                "boxed_number_allocations_static": 0,
            },
            "typed_array_slow_path_reduction_static": 2,
            "typed_array_element_helper_reduction_static": 2,
        },
        "speedup_fast_vs_disabled": 8.56,
        "rss_regression_pct_fast_vs_disabled": 0.74,
        "offset_correctness": {
            "status": "pass",
            "expected_checksum": "98",
            "node_checksum": "98",
            "perry_checksum": "98",
        },
        "gc_trace_fast": {
            "gc_cycles": 0,
            "copied_minor_eligible_cycles": 0,
            "copied_minor_success_rate": None,
        },
        "gc_trace_disabled": {"gc_cycles": 0},
    }
    report.update(overrides)
    return {"status": "pass", "report": report}


def release_build(status="pass", exit_code=0):
    return {"status": status, "exit_code": exit_code, "perry": "target/release/perry"}


class TypeLoweringEvidencePacketTests(unittest.TestCase):
    def test_accepts_material_typedarray_win_without_gc_cycles(self):
        packet = PACKET.evaluate_packet(release_build(), compiler_report(), benchmark_report())
        self.assertEqual(packet["status"], "pass", packet["errors"])
        self.assertEqual(packet["summary"]["copied_minor_eligible_cycles"], 0)
        self.assertIsNone(packet["summary"]["copied_minor_success_rate"])
        self.assertEqual(packet["summary"]["speedup_threshold"], 8.0)
        self.assertEqual(packet["summary"]["node_version"], "v20.20.2")
        self.assertEqual(packet["summary"]["fast_buffer_slow_path_accesses_static"], 0)
        self.assertEqual(
            packet["summary"]["disabled_buffer_slow_path_accesses_static"], 2
        )

    def test_rejects_small_speedup(self):
        packet = PACKET.evaluate_packet(
            release_build(),
            compiler_report(),
            benchmark_report(speedup_fast_vs_disabled=7.99),
        )
        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(error.startswith("speedup_fast_vs_disabled") for error in packet["errors"]),
            packet["errors"],
        )

    def test_rejects_checksum_drift(self):
        report = benchmark_report()["report"]
        report["perry_fast"] = {"checksum": "1", "median_ms": 93}
        packet = PACKET.evaluate_packet(
            release_build(),
            compiler_report(),
            {"status": "pass", "report": report},
        )
        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(error.startswith("node_perry_checksum_parity") for error in packet["errors"]),
            packet["errors"],
        )

    def test_rejects_equal_but_wrong_checksum(self):
        report = benchmark_report()["report"]
        report["node_reference"]["checksum"] = "1"
        report["perry_fast"]["checksum"] = "1"
        report["perry_disabled_fast_path_baseline"]["checksum"] = "1"
        packet = PACKET.evaluate_packet(
            release_build(),
            compiler_report(),
            {"status": "pass", "report": report},
        )
        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(error.startswith("expected_sum_checksum") for error in packet["errors"]),
            packet["errors"],
        )

    def test_rejects_missing_offset_correctness(self):
        report = benchmark_report()["report"]
        del report["offset_correctness"]
        packet = PACKET.evaluate_packet(
            release_build(),
            compiler_report(),
            {"status": "pass", "report": report},
        )
        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(
                error.startswith("offset_subarray_checksum_parity")
                for error in packet["errors"]
            ),
            packet["errors"],
        )

    def test_rejects_malformed_offset_correctness_without_crashing(self):
        report = benchmark_report()["report"]
        report["offset_correctness"]["perry_checksum"] = "not-a-number"
        packet = PACKET.evaluate_packet(
            release_build(),
            compiler_report(),
            {"status": "pass", "report": report},
        )
        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(
                error.startswith("offset_subarray_checksum_parity")
                for error in packet["errors"]
            ),
            packet["errors"],
        )

    def test_rejects_missing_direct_load_proof(self):
        compiler = compiler_report()
        checks = compiler["structural_report"]["checks"]
        checks[:] = [row for row in checks if row["name"] != "typedarray_param_direct_f64_load"]
        packet = PACKET.evaluate_packet(release_build(), compiler, benchmark_report())
        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(error.startswith("typedarray_param_direct_f64_load") for error in packet["errors"]),
            packet["errors"],
        )

    def test_rejects_missing_static_pressure_proof(self):
        report = benchmark_report()["report"]
        del report["static_pressure"]
        packet = PACKET.evaluate_packet(
            release_build(),
            compiler_report(),
            {"status": "pass", "report": report},
        )
        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(
                error.startswith("typedarray_static_slow_path_pressure_reduced")
                for error in packet["errors"]
            ),
            packet["errors"],
        )

    def test_rejects_fast_static_slow_path_helper(self):
        report = benchmark_report()["report"]
        report["static_pressure"]["perry_fast"]["buffer_slow_path_accesses_static"] = 1
        report["static_pressure"]["typed_array_slow_path_reduction_static"] = 1
        packet = PACKET.evaluate_packet(
            release_build(),
            compiler_report(),
            {"status": "pass", "report": report},
        )
        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(
                error.startswith("typedarray_static_slow_path_pressure_reduced")
                for error in packet["errors"]
            ),
            packet["errors"],
        )

    def test_rejects_material_rss_regression(self):
        packet = PACKET.evaluate_packet(
            release_build(),
            compiler_report(),
            benchmark_report(rss_regression_pct_fast_vs_disabled=9.0),
        )
        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(
                error.startswith("rss_regression_pct_fast_vs_disabled")
                for error in packet["errors"]
            ),
            packet["errors"],
        )

    def test_rejects_failed_release_build(self):
        packet = PACKET.evaluate_packet(
            release_build(status="fail", exit_code=101),
            compiler_report(),
            benchmark_report(),
        )
        self.assertEqual(packet["status"], "fail")
        self.assertTrue(
            any(error.startswith("release_build_current_source") for error in packet["errors"]),
            packet["errors"],
        )


if __name__ == "__main__":
    unittest.main()
