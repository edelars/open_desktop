#!/usr/bin/env python3
"""Unit tests for Stage 3 harness evidence validation; no child suites run."""

import hashlib
import tempfile
import unittest
from pathlib import Path

import stage3_harness as harness


class Stage3HarnessValidationTests(unittest.TestCase):
    def test_source_fingerprint_mismatch_is_rejected(self):
        with self.assertRaisesRegex(harness.HarnessError, "source fingerprint changed"):
            harness.validate_fingerprint("a" * 64, "b" * 64)

    def test_input_manifest_digest_mismatch_is_rejected(self):
        raw = b'{"schema_version":1}\n'
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "input_manifest.json"
            path.write_bytes(raw)
            wrong = hashlib.sha256(b"different manifest").hexdigest()
            with self.assertRaisesRegex(harness.HarnessError, "input manifest digest mismatch"):
                harness.validate_input_manifest_file(path, wrong)

    def test_partial_run_cannot_be_reported_as_local_only(self):
        statuses = {name: "passed" for name in harness.LOCAL_STEPS}
        statuses["memory_corpus"] = "not_run"
        with self.assertRaisesRegex(harness.HarnessError, "memory_corpus"):
            harness.validate_required_steps(statuses)

    def test_unselected_suites_must_remain_not_run(self):
        suites = {name: {"status": "not_run"} for name in harness.UNSELECTED_SUITES}
        harness.validate_unselected_suites(suites)
        suites["mac_mini"]["status"] = "passed"
        with self.assertRaisesRegex(harness.HarnessError, "mac_mini"):
            harness.validate_unselected_suites(suites)

    def test_required_memory_test_must_be_present_and_non_ignored(self):
        required = {"fixed_seed_memory_corpus_matches_interpreter_twice_per_input"}
        with self.assertRaisesRegex(harness.HarnessError, "missing, ignored, or failed"):
            harness.validate_test_outcomes(
                "test fixed_seed_memory_corpus_matches_interpreter_twice_per_input ... ignored\n",
                required,
                {"saved_case"},
            )

    def test_allowed_environment_replay_tests_do_not_hide_unexpected_ignored_tests(self):
        output = (
            "test replay_saved_case_from_environment ... ignored, set OD_JIT_REPLAY_CASE\n"
            "test fixed_seed_memory_corpus_matches_interpreter_twice_per_input ... ok\n"
        )
        outcomes = harness.validate_test_outcomes(
            output,
            {"fixed_seed_memory_corpus_matches_interpreter_twice_per_input"},
            harness.MEMORY_ALLOWED_IGNORED_TESTS,
        )
        self.assertEqual(outcomes["replay_saved_case_from_environment"], "ignored")
        with self.assertRaisesRegex(harness.HarnessError, "unexpected ignored tests"):
            harness.validate_test_outcomes(
                output + "test required_fault_profile ... ignored\n",
                {"fixed_seed_memory_corpus_matches_interpreter_twice_per_input"},
                harness.MEMORY_ALLOWED_IGNORED_TESTS,
            )

    def test_self_test_output_validator_rejects_zero_match_and_all_skipped(self):
        count = len(harness.SELF_TEST_NAMES)
        def output(status):
            rows = [
                f"{name} (__main__.Stage3HarnessValidationTests.{name}) ... {status}"
                for name in harness.SELF_TEST_NAMES
            ]
            footer = "OK" if status == "ok" else f"OK (skipped={count})"
            return "\n".join([*rows, f"Ran {count} tests in 0.001s", footer])

        passed = harness.validate_self_test_output(output("ok"))
        self.assertEqual(len(passed), count)
        with self.assertRaisesRegex(harness.HarnessError, "exact required test identities"):
            harness.validate_self_test_output("Ran 0 tests in 0.000s\nOK\n")
        with self.assertRaisesRegex(harness.HarnessError, "skipped or failed"):
            harness.validate_self_test_output(output("skipped, not run"))

        wrong_identity = output("ok").replace(
            "__main__.Stage3HarnessValidationTests.test_source_fingerprint_mismatch_is_rejected",
            "__main__.OtherTests.test_source_fingerprint_mismatch_is_rejected",
        )
        with self.assertRaisesRegex(harness.HarnessError, "exact required test identities"):
            harness.validate_self_test_output(wrong_identity)

    def test_stage1_child_fingerprint_and_both_builds_are_pinned(self):
        expected = "a" * 64
        build = {
            name: {"revision": f"sha256:{expected}"}
            for name in ("local_interpreter", "local_jit_dispatch")
        }
        harness.validate_stage1_fingerprint_and_builds(expected, expected, build)
        for observed in ("0" * 64, expected):
            altered = {key: value.copy() for key, value in build.items()}
            if observed == expected:
                altered["local_interpreter"]["revision"] = f"sha256:{'0' * 64}"
            with self.assertRaises(harness.HarnessError):
                harness.validate_stage1_fingerprint_and_builds(observed, expected, altered)
        for name in ("local_interpreter", "local_jit_dispatch"):
            altered = {key: value.copy() for key, value in build.items()}
            altered[name]["revision"] = f"sha256:{'0' * 64}"
            with self.assertRaises(harness.HarnessError):
                harness.validate_stage1_fingerprint_and_builds(expected, expected, altered)

    def test_manual_frozen_input_and_result_bind_to_parent_manifest(self):
        expected_digest = "a" * 64
        parent = [{"case_id": "manual-case", "sha256": expected_digest}]
        child_inputs = [{
            "case_id": "manual-case", "seed": None, "fixture_sha256": expected_digest,
            "fixture_path": "work/stage1-oracle/run/fixtures/manual-case.json",
        }]
        results = [
            {
                "case_id": "manual-case", "fixture_sha256": expected_digest,
                "fixture_path": child_inputs[0]["fixture_path"], "repeat": repeat,
            }
            for repeat in (1, 2)
        ]
        harness.validate_stage1_manual_bindings(child_inputs, results, parent)
        changed_input = [dict(child_inputs[0], fixture_sha256="b" * 64)]
        with self.assertRaisesRegex(harness.HarnessError, "frozen manual input digest"):
            harness.validate_stage1_manual_bindings(changed_input, results, parent)
        changed_result = [dict(results[0], fixture_sha256="b" * 64), results[1]]
        with self.assertRaisesRegex(harness.HarnessError, "manual result is not bound"):
            harness.validate_stage1_manual_bindings(child_inputs, changed_result, parent)
        changed_path = [dict(results[0], fixture_path="work/other.json"), results[1]]
        with self.assertRaisesRegex(harness.HarnessError, "manual result is not bound"):
            harness.validate_stage1_manual_bindings(child_inputs, changed_path, parent)

if __name__ == "__main__":
    unittest.main(verbosity=2)
