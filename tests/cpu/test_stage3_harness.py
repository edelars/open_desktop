#!/usr/bin/env python3
"""Unit tests for Stage 3 harness evidence validation; no child suites run."""

import hashlib
import shutil
import tempfile
import unittest
from pathlib import Path

import stage3_harness as harness


def passing_rust_gate_steps(log_dir: Path):
    steps = {}
    for step, suite, package, identity in harness.RUST_TEST_GATES:
        output = f"test {identity} ... ok\n"
        log_path = log_dir / f"{step}.log"
        log_path.write_text(output)
        steps[step] = {
            "name": step,
            "gate_suite": suite,
            "package": package,
            "test_identity": identity,
            "argv": harness.rust_test_argv(package, identity),
            "status": "passed",
            "failure": None,
            "returncode": 0,
            "output_tail": output,
            "output_sha256": harness.sha256_bytes(output.encode("utf-8")),
            "log": log_path.relative_to(harness.ROOT).as_posix(),
            "test_outcomes": {identity: "ok"},
        }
    return steps


def passing_final_summary(run_id: str, log_dir: Path):
    steps = passing_rust_gate_steps(log_dir)
    for name in ("harness_self_tests", "stage1_dispatch", "memory_corpus"):
        steps[name] = {"status": "passed"}
    lifecycle = [step for step, suite, _, _ in harness.RUST_TEST_GATES if suite == "local_lifecycle"]
    acceptance = [step for step, suite, _, _ in harness.RUST_TEST_GATES if suite == "local_acceptance"]
    suites = {name: {"status": "not_run"} for name in harness.UNSELECTED_SUITES}
    suites["local_lifecycle"] = {"status": "passed", "tests": lifecycle}
    suites["local_acceptance"] = {"status": "passed", "tests": acceptance}
    return {"run_id": run_id, "steps": steps, "suites": suites}


def replace_step_log_output(step, output: str):
    log_path = harness.ROOT / step["log"]
    log_path.write_text(output)
    step["output_tail"] = output[-4000:]
    step["output_sha256"] = harness.sha256_bytes(output.encode("utf-8"))


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

    def test_rust_test_manifest_rejects_a_missing_mandatory_gate(self):
        manifest = {
            "gates": harness.rust_test_gate_manifest(),
            "sources": [
                {"path": name, "sha256": harness.sha256_file(harness.ROOT / name)}
                for name in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        manifest["gates"].pop()
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(manifest)

    def test_rust_gate_summary_rejects_an_omitted_mandatory_gate(self):
        with tempfile.TemporaryDirectory(dir=harness.ROOT) as temporary:
            steps = passing_rust_gate_steps(Path(temporary))
            missing_step = harness.ADDITIONAL_TEST_GATES[0][0]
            del steps[missing_step]
            with self.assertRaisesRegex(harness.HarnessError, missing_step):
                harness.validate_required_rust_test_evidence(steps)

    def test_rust_gate_summary_rejects_forged_passed_evidence(self):
        with tempfile.TemporaryDirectory(dir=harness.ROOT) as temporary:
            steps = passing_rust_gate_steps(Path(temporary))
            step_name, _, _, _ = harness.ADDITIONAL_TEST_GATES[0]
            steps[step_name]["output_tail"] = "test tests::unrelated_test ... ok\n"
            with self.assertRaisesRegex(harness.HarnessError, step_name):
                harness.validate_required_rust_test_evidence(steps)

            steps = passing_rust_gate_steps(Path(temporary))
            steps[step_name]["argv"] = harness.rust_test_argv("od-host", "tests::unrelated_test")
            with self.assertRaisesRegex(harness.HarnessError, step_name):
                harness.validate_required_rust_test_evidence(steps)

    def test_rust_gate_rejects_zero_match_ignored_and_failed_runs(self):
        step_name, _, _, identity = harness.ADDITIONAL_TEST_GATES[0]
        with tempfile.TemporaryDirectory(dir=harness.ROOT) as temporary:
            for output in (
                "test result: ok. 0 passed; 0 failed; 0 ignored\n",
                f"test {identity} ... ignored\n",
                f"test {identity} ... ok\ntest {identity} ... ok\n",
            ):
                steps = passing_rust_gate_steps(Path(temporary))
                replace_step_log_output(steps[step_name], output)
                with self.assertRaisesRegex(harness.HarnessError, step_name):
                    harness.validate_required_rust_test_evidence(steps)

            steps = passing_rust_gate_steps(Path(temporary))
            steps[step_name]["status"] = "failed"
            steps[step_name]["failure"] = "timeout"
            steps[step_name]["returncode"] = None
            with self.assertRaisesRegex(harness.HarnessError, step_name):
                harness.validate_required_rust_test_evidence(steps)

    def test_final_summary_rejects_missing_invalid_and_traversal_run_id(self):
        for run_id in (
            None,
            "",
            "invalid-run-id",
            "../outside",
            "stage3-20261030-010101-abcdef0123/../../outside",
        ):
            with self.subTest(run_id=run_id):
                with self.assertRaisesRegex(harness.HarnessError, "run id is missing or invalid"):
                    harness.validate_final_summary({"run_id": run_id}, Path("missing-input.json"))

    def test_final_summary_rejects_run_directory_and_logs_symlinks(self):
        harness_root = harness.ROOT / "work" / "stage3-harness"
        harness_root.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=harness.ROOT) as external:
            external_path = Path(external)

            run_id = harness.new_run_id()
            run_link = harness_root / run_id
            run_link.symlink_to(external_path, target_is_directory=True)
            try:
                with self.assertRaisesRegex(harness.HarnessError, "run directory escaped"):
                    harness.validate_run_id(run_id)
            finally:
                run_link.unlink()

            canonical_id = harness.new_run_id()
            canonical_dir = harness_root / canonical_id
            canonical_dir.mkdir()
            alias_id = harness.new_run_id()
            alias_link = harness_root / alias_id
            alias_link.symlink_to(canonical_dir, target_is_directory=True)
            try:
                with self.assertRaisesRegex(harness.HarnessError, "run directory escaped"):
                    harness.validate_run_id(alias_id)
            finally:
                alias_link.unlink()
                canonical_dir.rmdir()

            run_id = harness.new_run_id()
            run_dir = harness_root / run_id
            run_dir.mkdir()
            logs_link = run_dir / "logs"
            logs_link.symlink_to(external_path, target_is_directory=True)
            try:
                with self.assertRaisesRegex(harness.HarnessError, "run logs escaped"):
                    harness.validate_final_summary({"run_id": run_id}, Path("missing-input.json"))
            finally:
                logs_link.unlink()
                run_dir.rmdir()

    def test_final_summary_rejects_step_log_symlink_and_external_log_path(self):
        harness_root = harness.ROOT / "work" / "stage3-harness"
        harness_root.mkdir(parents=True, exist_ok=True)
        run_id = harness.new_run_id()
        run_dir = harness_root / run_id
        logs_dir = run_dir / "logs"
        logs_dir.mkdir(parents=True)
        with tempfile.TemporaryDirectory(dir=harness.ROOT) as external:
            external_log = Path(external) / "result.log"
            step_name, _, _, identity = harness.RUST_TEST_GATES[0]
            output = f"test {identity} ... ok\n"
            external_log.write_text(output)
            summary = passing_final_summary(run_id, logs_dir)
            expected_log = logs_dir / f"{step_name}.log"
            expected_log.unlink()
            expected_log.symlink_to(external_log)
            try:
                with self.assertRaisesRegex(harness.HarnessError, "log path differs"):
                    harness.validate_final_summary(summary, Path("unused-input-manifest.json"))

                expected_log.unlink()
                expected_log.write_text(output)
                summary["steps"][step_name]["log"] = external_log.relative_to(
                    harness.ROOT
                ).as_posix()
                with self.assertRaisesRegex(harness.HarnessError, "log path differs"):
                    harness.validate_final_summary(summary, Path("unused-input-manifest.json"))
            finally:
                expected_log.unlink(missing_ok=True)
                shutil.rmtree(run_dir)

if __name__ == "__main__":
    unittest.main(verbosity=2)
