"""Focused standard-library tests for the S3-31c report validator."""

from __future__ import annotations

import copy
import hashlib
import unittest

from validate_s3_31c import (
    BASE_RIP,
    ORDERS,
    PREFLIGHT_PARTIALS,
    PROFILE_ID,
    STRATA,
    ValidationError,
    _initial_state,
    _partial_control_spec,
    _profile_case,
    validate_reports,
)


SOURCE_FINGERPRINTS = [
    ["profile", "0" * 64],
    ["cli", "1" * 64],
    ["lib", "2" * 64],
    ["dispatch", "3" * 64],
    ["block", "4" * 64],
    ["host", "5" * 64],
    ["frozen_fixture", "6" * 64],
]
HOST = {
    "architecture": "aarch64",
    "build": {
        "profile": "release",
        "rustc": "rustc test",
        "source_revision": "a" * 40,
        "source_sha256": "b" * 64,
        "source_tree_dirty": False,
        "target": "aarch64-apple-darwin",
    },
    "cpu_model": "Apple M3 Max",
    "execution": {
        "repository_revision": "a" * 40,
        "working_directory": "/repo",
        "working_tree_dirty": None,
    },
    "os": "macos",
    "os_version": "26.7.1",
}


def _sample(
    *, pair: int, order: str, seed: int, calls: int, budget: int,
    iterations: int, side: str, elapsed: int,
) -> dict:
    is_interpreter = side == "interpreter"
    return {
        "pair": pair,
        "order": order,
        "seed": seed,
        "calls": calls,
        "guest_instructions": calls * budget,
        "retired": calls * budget,
        "elapsed_ns": elapsed,
        "cache_delta": None if is_interpreter else {
            "hits": calls * iterations,
            "misses": 0,
            "compiles": 0,
            "publications": 0,
            "replacements": 0,
        },
    }


def _pair(pair: int, seed: int, budget: int, iterations: int, calls: int = 1) -> dict:
    order = ORDERS[pair % len(ORDERS)]
    baseline = _sample(
        pair=pair, order=order, seed=seed, calls=calls, budget=budget,
        iterations=iterations, side="baseline", elapsed=300_000_000,
    )
    candidate = _sample(
        pair=pair, order=order, seed=seed, calls=calls, budget=budget,
        iterations=iterations, side="candidate", elapsed=250_000_000,
    )
    interpreter = _sample(
        pair=pair, order=order, seed=seed, calls=calls, budget=budget,
        iterations=iterations, side="interpreter", elapsed=400_000_000,
    )
    return {
        "pair": pair,
        "order": order,
        "seed": seed,
        "baseline": baseline,
        "candidate": candidate,
        "interpreter": interpreter,
        "candidate_over_baseline_ratio": candidate["elapsed_ns"] / baseline["elapsed_ns"],
        "candidate_over_interpreter_ratio": candidate["elapsed_ns"] / interpreter["elapsed_ns"],
    }


def _observation(rip: int, retired: int, exit_name: str, remaining: int = 0) -> dict:
    return {
        "state": {"gpr": [0] * 16, "rflags": 0x202, "rip": rip},
        "exit": exit_name,
        "retired": retired,
        "remaining_budget": remaining,
        "blocks_executed": 1,
    }


def _reports() -> list[dict]:
    reports = []
    for process_run in (1, 2, 3):
        strata = []
        for stratum_id in STRATA:
            case = _profile_case(stratum_id)
            iterations = case["iterations"]
            budget = case["guest_work_per_call"]
            seed = case["seed"]
            code = bytes.fromhex(case["code_hex"])
            pair = _pair(0, seed, budget, iterations, calls=64)
            strata.append({
                "id": stratum_id,
                "code_hex": case["code_hex"],
                "code_sha256": case["code_sha256"],
                "seed": seed,
                "iterations": iterations,
                "guest_work_per_call": budget,
                "resident_entries": 1,
                "resident_code_bytes": len(code),
                "resident_logical_bytes": len(code),
                "warmup_calls_per_side": 32,
                "calibrated_calls_per_side": 64,
                "calibration": pair,
                "retained_pairs": [
                    _pair(pair_id, seed, budget, iterations, calls=64)
                    for pair_id in range(20)
                ],
                "discarded_restarts": [],
            })
        reports.append({
            "schema": 1,
            "profile_id": PROFILE_ID,
            "process_run": process_run,
            "measure_host": copy.deepcopy(HOST),
            "source_fingerprints": copy.deepcopy(SOURCE_FINGERPRINTS),
            "timed_scope": "full synchronous interpreter, ordinary cached dispatcher, or prepared-key candidate call",
            "sampling_protocol": "each sample side has at least 200 ms measured elapsed time",
            "ratio_contract": "candidate_over_baseline_ratio is elapsed candidate / elapsed comparator; below 1 is faster",
            "strata": strata,
            "total_elapsed_ns": 10_000_000_000,
        })
    return reports


def _preflight(reports: list[dict]) -> dict:
    strata = []
    for stratum in reports[0]["strata"]:
        iterations = stratum["iterations"]
        budget = stratum["guest_work_per_call"]
        state = _initial_state(stratum["seed"])
        baseline = _observation(BASE_RIP, budget, "BudgetExhausted", 0)
        baseline["state"] = copy.deepcopy(state)
        baseline["blocks_executed"] = iterations
        interpreter = copy.deepcopy(baseline)
        interpreter["exit"] = "InstructionBudgetExhausted"
        interpreter["blocks_executed"] = 0
        controls = []
        for control_id in sorted(PREFLIGHT_PARTIALS):
            spec = _partial_control_spec(stratum["id"], control_id)
            control_state = copy.deepcopy(spec["initial_state"])
            control_state["rip"] = spec["expected_rip"]
            control_base = _observation(
                spec["expected_rip"], spec["retired"], spec["native_exit"],
                spec["budget"] - spec["retired"],
            )
            control_base["state"] = control_state
            control_base["blocks_executed"] = spec["blocks_executed"]
            control_interpreter = copy.deepcopy(control_base)
            control_interpreter["exit"] = spec["interpreter_exit"]
            control_interpreter["blocks_executed"] = 0
            controls.append({
                "id": control_id,
                "budget": spec["budget"],
                "stop_rip": spec["stop_rip"],
                "initial_state": copy.deepcopy(spec["initial_state"]),
                "baseline": control_base,
                "candidate": copy.deepcopy(control_base),
                "interpreter": control_interpreter,
                "candidate_reuse_counts": {
                    "prepared_keys": 1,
                    "reused_keys": spec["blocks_executed"] - 1,
                },
            })
        strata.append({
            "id": stratum["id"],
            "code_hex": stratum["code_hex"],
            "code_sha256": stratum["code_sha256"],
            "seed": stratum["seed"],
            "iterations": iterations,
            "budget": budget,
            "baseline": baseline,
            "candidate": copy.deepcopy(baseline),
            "interpreter": interpreter,
            "reuse_counts": {"prepared_keys": 1, "reused_keys": iterations - 1},
            "hit_only": True,
            "zero_work_checked": True,
            "entry_stop_checked": True,
            "entry_event_checked": True,
            "partial_controls_checked": True,
            "partial_controls": controls,
            "dependency_replacement_checked": True,
        })
    return {
        "schema": 1,
        "profile_id": PROFILE_ID,
        "measure_host": copy.deepcopy(HOST),
        "source_fingerprints": copy.deepcopy(SOURCE_FINGERPRINTS),
        "timing_started": False,
        "strata": strata,
    }


def _set_pair_calls(pair: dict, calls: int, budget: int, iterations: int) -> None:
    for side in ("baseline", "candidate", "interpreter"):
        sample = pair[side]
        sample["calls"] = calls
        sample["guest_instructions"] = calls * budget
        sample["retired"] = calls * budget
        if side != "interpreter":
            sample["cache_delta"]["hits"] = calls * iterations
    pair["candidate_over_baseline_ratio"] = (
        pair["candidate"]["elapsed_ns"] / pair["baseline"]["elapsed_ns"]
    )
    pair["candidate_over_interpreter_ratio"] = (
        pair["candidate"]["elapsed_ns"] / pair["interpreter"]["elapsed_ns"]
    )


class ValidateS331cTests(unittest.TestCase):
    def test_valid_reports_recompute_ratios_and_keep_host_metadata_per_run(self):
        reports = _reports()
        before = copy.deepcopy(reports)
        summary = validate_reports(reports, _preflight(reports))
        self.assertEqual(summary["status"], "passed")
        self.assertEqual(summary["strata"]["a_1"]["retained_pair_count"], 60)
        self.assertAlmostEqual(
            summary["strata"]["a_1"]["candidate_over_baseline_ratio"]["median"],
            250_000_000 / 300_000_000,
        )
        self.assertEqual([x["process_run"] for x in summary["host_runs"]], [1, 2, 3])
        self.assertEqual(reports, before, "validator must not mutate input reports")

    def test_preflight_keeps_its_run_metadata_separate_from_timed_reports(self):
        reports = _reports()
        preflight = _preflight(reports)
        preflight["measure_host"]["execution"]["working_directory"] = "/private/tmp/stage3-s31c-clean"
        summary = validate_reports(reports, preflight)
        self.assertEqual(
            summary["preflight"]["measure_host"]["execution"]["working_directory"],
            "/private/tmp/stage3-s31c-clean",
        )
        self.assertEqual(
            summary["host_runs"][0]["measure_host"]["execution"]["working_directory"],
            "/repo",
        )

    def test_rejects_duration_below_sample_floor(self):
        reports = _reports()
        reports[0]["strata"][0]["retained_pairs"][0]["baseline"]["elapsed_ns"] = 199_999_999
        with self.assertRaisesRegex(ValidationError, "below the 200000000 ns floor"):
            validate_reports(reports)

    def test_rejects_inconsistent_work_count(self):
        reports = _reports()
        reports[0]["strata"][0]["retained_pairs"][0]["candidate"]["guest_instructions"] += 1
        with self.assertRaisesRegex(ValidationError, "guest_instructions: inconsistent values"):
            validate_reports(reports)

    def test_rejects_saved_ratio_that_does_not_match_raw_samples(self):
        reports = _reports()
        reports[0]["strata"][0]["retained_pairs"][0]["candidate_over_baseline_ratio"] = 4.0
        with self.assertRaisesRegex(ValidationError, "does not match raw samples"):
            validate_reports(reports)

    def test_rejects_nonfinite_saved_ratio(self):
        reports = _reports()
        reports[0]["strata"][0]["retained_pairs"][0]["candidate_over_baseline_ratio"] = float("nan")
        with self.assertRaisesRegex(ValidationError, "must be finite and positive"):
            validate_reports(reports)

    def test_rejects_source_fingerprint_mismatch(self):
        reports = _reports()
        reports[2]["source_fingerprints"][0][1] = "f" * 64
        with self.assertRaisesRegex(ValidationError, "source_fingerprints: inconsistent values"):
            validate_reports(reports)

    def test_rejects_cache_miss_during_measured_sample(self):
        reports = _reports()
        reports[1]["strata"][2]["retained_pairs"][0]["candidate"]["cache_delta"]["misses"] = 1
        with self.assertRaisesRegex(ValidationError, "cache_delta.misses: inconsistent values"):
            validate_reports(reports)

    def test_rejects_missing_or_duplicate_stratum(self):
        missing = _reports()
        missing[0]["strata"].pop()
        with self.assertRaisesRegex(ValidationError, "missing strata"):
            validate_reports(missing)

        duplicate = _reports()
        duplicate[0]["strata"].append(copy.deepcopy(duplicate[0]["strata"][0]))
        with self.assertRaisesRegex(ValidationError, "duplicate stratum"):
            validate_reports(duplicate)

    def test_rejects_duplicate_pair_and_undersized_pair_set(self):
        duplicate = _reports()
        second = duplicate[0]["strata"][0]["retained_pairs"][1]
        second["pair"] = 0
        second["order"] = ORDERS[0]
        for side in ("baseline", "candidate", "interpreter"):
            second[side]["pair"] = 0
            second[side]["order"] = ORDERS[0]
        with self.assertRaisesRegex(ValidationError, "duplicate retained pair id"):
            validate_reports(duplicate)

        undersized = _reports()
        undersized[2]["strata"][4]["retained_pairs"].pop()
        with self.assertRaisesRegex(ValidationError, "expected 20 retained pairs; got 19"):
            validate_reports(undersized)

    def test_allows_retained_calls_to_double_after_initial_calibration(self):
        reports = _reports()
        stratum = reports[0]["strata"][0]
        case = _profile_case(stratum["id"])
        discarded = [
            _pair(pair_id, case["seed"], case["guest_work_per_call"], case["iterations"], calls=64)
            for pair_id in range(20)
        ]
        discarded[0]["baseline"]["elapsed_ns"] = 199_000_000
        _set_pair_calls(discarded[0], 64, case["guest_work_per_call"], case["iterations"])
        stratum["discarded_restarts"] = [{
            "attempt": 1,
            "previous_calls": 64,
            "next_calls": 128,
            "discarded_pairs": discarded,
        }]
        stratum["calibrated_calls_per_side"] = 128
        retained = [
            _pair(pair_id + 20, case["seed"], case["guest_work_per_call"], case["iterations"], calls=128)
            for pair_id in range(20)
        ]
        stratum["retained_pairs"] = retained
        summary = validate_reports(reports)
        self.assertEqual(summary["status"], "passed")

    def test_rejects_sparse_retained_pair_ids_even_when_orders_match(self):
        reports = _reports()
        pairs = reports[0]["strata"][0]["retained_pairs"]
        for index, pair in enumerate(pairs):
            pair_id = index * 6
            pair["pair"] = pair_id
            pair["order"] = ORDERS[0]
            for side in ("baseline", "candidate", "interpreter"):
                pair[side]["pair"] = pair_id
                pair[side]["order"] = ORDERS[0]
        with self.assertRaisesRegex(ValidationError, "retained_pairs.ids: inconsistent values"):
            validate_reports(reports)

    def test_rejects_reordered_contiguous_retained_pair_ids(self):
        reports = _reports()
        reports[0]["strata"][0]["retained_pairs"].reverse()
        with self.assertRaisesRegex(ValidationError, "retained_pairs.ids: inconsistent values"):
            validate_reports(reports)

    def test_rejects_identical_but_non_profile_workload_relabeling(self):
        reports = _reports()
        replacement = bytes.fromhex("90")
        for report in reports:
            stratum = next(item for item in report["strata"] if item["id"] == "a_4096")
            stratum.update({
                "iterations": 1,
                "seed": 999,
                "guest_work_per_call": 3,
                "code_hex": replacement.hex(),
                "code_sha256": hashlib.sha256(replacement).hexdigest(),
                "resident_code_bytes": len(replacement),
                "resident_logical_bytes": len(replacement),
            })
            for pair in [stratum["calibration"], *stratum["retained_pairs"]]:
                pair["seed"] = 999
                for side in ("baseline", "candidate", "interpreter"):
                    sample = pair[side]
                    sample["seed"] = 999
                    sample["guest_instructions"] = sample["calls"] * 3
                    sample["retired"] = sample["guest_instructions"]
                    if side != "interpreter":
                        sample["cache_delta"]["hits"] = sample["calls"]
        with self.assertRaisesRegex(ValidationError, "profile_fixture"):
            validate_reports(reports)

    def test_preflight_rejects_missing_observation(self):
        reports = _reports()
        preflight = _preflight(reports)
        preflight["strata"][0]["baseline"] = {}
        with self.assertRaisesRegex(ValidationError, "baseline.state: must be a JSON object"):
            validate_reports(reports, preflight)

    def test_preflight_rejects_matching_wrong_retirement_and_exit(self):
        reports = _reports()
        preflight = _preflight(reports)
        first = preflight["strata"][0]
        for mode in ("baseline", "candidate", "interpreter"):
            first[mode]["retired"] -= 1
            first[mode]["remaining_budget"] += 1
        with self.assertRaisesRegex(ValidationError, "baseline.retired: inconsistent values"):
            validate_reports(reports, preflight)

        preflight = _preflight(reports)
        first = preflight["strata"][0]
        first["baseline"]["exit"] = "OtherExit"
        first["candidate"]["exit"] = "OtherExit"
        with self.assertRaisesRegex(ValidationError, "baseline.exit: inconsistent values"):
            validate_reports(reports, preflight)


if __name__ == "__main__":
    unittest.main()
