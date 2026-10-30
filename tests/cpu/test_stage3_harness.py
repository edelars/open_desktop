#!/usr/bin/env python3
"""Unit tests for Stage 3 harness evidence validation; no child suites run."""

import hashlib
import importlib.util
import os
import signal
import subprocess
import shutil
import sys
import tempfile
import time
import unittest
from unittest import mock
from pathlib import Path

import stage3_harness as harness


def temporary_run_root():
    parent = harness.ROOT / "work" / "stage3-harness"
    parent.mkdir(parents=True, exist_ok=True)
    temporary = tempfile.TemporaryDirectory(dir=parent)
    run_root = Path(temporary.name)
    (run_root / "logs").mkdir()
    return temporary, run_root


def valid_process_step(run_root: Path, name: str = "process_sw_vers"):
    argv = harness.vmhost_process_commands(run_root)[name]
    stdout = "Darwin host evidence\n"
    stderr = ""
    combined = stdout
    log_paths = {
        "log": run_root / "logs" / f"{name}.log",
        "stdout_log": run_root / "logs" / f"{name}.stdout.log",
        "stderr_log": run_root / "logs" / f"{name}.stderr.log",
    }
    log_paths["log"].write_text(combined)
    log_paths["stdout_log"].write_text(stdout)
    log_paths["stderr_log"].write_text(stderr)
    return {
        "name": name,
        "suite": harness.PROCESS_SUITE_NAME,
        "argv": argv,
        "command": harness.shlex.join(argv),
        "status": "passed",
        "failure": None,
        "returncode": 0,
        "expected_returncode": 0,
        "timeout_seconds": harness.PROCESS_STEP_TIMEOUTS[name],
        "duration_seconds": 0.01,
        "source_fingerprint_after": "a" * 64,
        "process_group_cleanup": "empty",
        "log": log_paths["log"].relative_to(harness.ROOT).as_posix(),
        "stdout_log": log_paths["stdout_log"].relative_to(harness.ROOT).as_posix(),
        "stderr_log": log_paths["stderr_log"].relative_to(harness.ROOT).as_posix(),
        "output_tail": combined,
        "output_sha256": harness.sha256_bytes(combined.encode()),
        "stdout_sha256": harness.sha256_bytes(stdout.encode()),
        "stderr_sha256": harness.sha256_bytes(stderr.encode()),
    }


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
    gate_name = "memory_s3_34h_exact_gate"
    gate_output = f"test {harness.MEMORY_EXACT_GATE} ... ok\n"
    gate_log = log_dir / f"{gate_name}.log"
    gate_log.write_text(gate_output)
    steps[gate_name] = {
        "name": gate_name,
        "status": "passed",
        "argv": [
            "cargo", "test", "--offline", "-p", "od-host", "--test", "memory_corpus",
            harness.MEMORY_EXACT_GATE, "--", "--exact", "--test-threads=1",
        ],
        "returncode": 0,
        "failure": None,
        "test_identity": harness.MEMORY_EXACT_GATE,
        "test_outcomes": {harness.MEMORY_EXACT_GATE: "ok"},
        "output_tail": gate_output,
        "output_sha256": harness.sha256_bytes(gate_output.encode()),
        "log": gate_log.relative_to(harness.ROOT).as_posix(),
    }
    steps["memory_corpus"]["test_outcomes"] = {}
    lifecycle = [step for step, suite, _, _ in harness.RUST_TEST_GATES if suite == "local_lifecycle"]
    acceptance = [step for step, suite, _, _ in harness.RUST_TEST_GATES if suite == "local_acceptance"]
    suites = {name: {"status": "not_run"} for name in harness.UNSELECTED_SUITES}
    suites["local_lifecycle"] = {"status": "passed", "tests": lifecycle}
    suites["local_acceptance"] = {"status": "passed", "tests": acceptance}
    suites["stage1_dispatch"] = {"status": "local_only"}
    suites["memory_corpus"] = {
        "status": "passed",
        "unique_inputs": 3600,
        "replays_per_input": 2,
        "test_outcomes": {},
        "s3_34h_exact_gate": steps[gate_name],
    }
    suites[harness.PROCESS_SUITE_NAME] = {"status": "not_run"}
    for name in harness.PROCESS_SUITE_STEPS:
        steps[name] = {"status": "not_run"}
    fingerprint = harness.source_fingerprint()
    return {
        "run_id": run_id,
        "status": "local_only",
        "result_scope": "local_only",
        "suite_selection": "local",
        "source": {"fingerprint_start": fingerprint, "fingerprint_end": fingerprint},
        "steps": steps,
        "suites": suites,
    }


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

    def test_s3_34h_exact_gate_requires_exact_identity_and_argv(self):
        with tempfile.TemporaryDirectory(dir=harness.ROOT) as temporary:
            log_path = Path(temporary) / "gate.log"
            output = f"test {harness.MEMORY_EXACT_GATE} ... ok\n"
            log_path.write_text(output)
            step = {
                "name": "memory_s3_34h_exact_gate",
                "argv": [
                    "cargo", "test", "--offline", "-p", "od-host", "--test", "memory_corpus",
                    harness.MEMORY_EXACT_GATE, "--", "--exact", "--test-threads=1",
                ],
                "returncode": 0,
                "failure": None,
                "test_identity": harness.MEMORY_EXACT_GATE,
                "test_outcomes": {harness.MEMORY_EXACT_GATE: "ok"},
                "output_tail": output,
                "output_sha256": harness.sha256_bytes(output.encode()),
                "log": log_path.relative_to(harness.ROOT).as_posix(),
            }
            harness.validate_memory_exact_gate_evidence(step)
            step["argv"] = ["cargo", "test"]
            with self.assertRaisesRegex(harness.HarnessError, "invocation is missing or forged"):
                harness.validate_memory_exact_gate_evidence(step)

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
        mandatory_steps = (
            "s3_33e_native_cmp_rax_imm8_host",
            "s3_33e_native_cmp_rax_imm8_compiler",
            "s3_33e_native_cmp_rax_imm8_admission",
            "s3_33i_native_xchg_rax_rbx_admission",
            "s3_33i_native_xchg_rax_rbx_emitter",
            "s3_33i_native_xchg_rax_rbx_compiler",
            "s3_33i_native_xchg_rax_rbx_host",
            "s3_33g_native_not_rax_emitter",
            "s3_33j_native_lea_rbx_disp8_admission",
            "s3_33j_native_lea_rbx_disp8_emitter",
            "s3_33j_native_lea_rbx_disp8_compiler",
            "s3_33j_native_lea_rbx_disp8_host",
            "s3_33k_native_mov_rax_imm32_admission",
            "s3_33k_native_mov_rax_imm32_compiler",
            "s3_33k_native_mov_rax_imm32_host",
            "s3_33l_native_lea_rbx_rcx_admission",
            "s3_33l_native_lea_rbx_rcx_compiler",
            "s3_33l_native_lea_rbx_rcx_host",
            "s3_33m_native_lea_rax_rip_disp32_admission",
            "s3_33m_native_lea_rax_rip_disp32_compiler",
            "s3_33m_native_lea_rax_rip_disp32_host",
        )
        for missing_step in (harness.RUST_TEST_GATES[-1][0], *mandatory_steps):
            manifest = {
                "gates": harness.rust_test_gate_manifest(),
                "sources": [
                    {"path": name, "sha256": harness.sha256_file(harness.ROOT / name)}
                    for name in harness.LOCAL_RUST_TEST_SOURCES
                ],
            }
            manifest["gates"] = [
                gate for gate in manifest["gates"] if gate["step"] != missing_step
            ]
            with self.subTest(missing_step=missing_step):
                with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
                    harness.validate_rust_test_manifest(manifest)

    def test_s3_33_native_gates_are_pinned_once_in_manifest_steps_and_summary(self):
        expected_steps = [
            "s3_33a_native_inc_rax_host",
            "s3_33a_native_inc_rax_compiler",
            "s3_33b_native_dec_rax_host",
            "s3_33b_native_dec_rax_compiler",
            "s3_33c_native_neg_rax_host",
            "s3_33c_native_neg_rax_compiler",
            "s3_33d_native_add_rax_imm8_host",
            "s3_33d_native_add_rax_imm8_compiler",
            "s3_33d_native_add_rax_imm8_admission",
            "s3_33e_native_cmp_rax_imm8_host",
            "s3_33e_native_cmp_rax_imm8_compiler",
            "s3_33e_native_cmp_rax_imm8_admission",
            "s3_33f_native_sub_rax_imm8_host",
            "s3_33f_native_sub_rax_imm8_compiler",
            "s3_33f_native_sub_rax_imm8_admission",
            "s3_33g_native_not_rax_admission",
            "s3_33g_native_not_rax_emitter",
            "s3_33g_native_not_rax_compiler",
            "s3_33g_native_not_rax_host",
            "s3_33h_native_lea_rax_rbx_admission",
            "s3_33h_native_lea_rax_rbx_compiler",
            "s3_33h_native_lea_rax_rbx_host",
            "s3_33i_native_xchg_rax_rbx_admission",
            "s3_33i_native_xchg_rax_rbx_emitter",
            "s3_33i_native_xchg_rax_rbx_compiler",
            "s3_33i_native_xchg_rax_rbx_host",
            "s3_33j_native_lea_rbx_disp8_admission",
            "s3_33j_native_lea_rbx_disp8_emitter",
            "s3_33j_native_lea_rbx_disp8_compiler",
            "s3_33j_native_lea_rbx_disp8_host",
            "s3_33k_native_mov_rax_imm32_admission",
            "s3_33k_native_mov_rax_imm32_compiler",
            "s3_33k_native_mov_rax_imm32_host",
            "s3_33l_native_lea_rbx_rcx_admission",
            "s3_33l_native_lea_rbx_rcx_compiler",
            "s3_33l_native_lea_rbx_rcx_host",
            "s3_33m_native_lea_rax_rip_disp32_admission",
            "s3_33m_native_lea_rax_rip_disp32_compiler",
            "s3_33m_native_lea_rax_rip_disp32_host",
        ]
        gates = [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_33")]
        manifest_gates = [
            gate for gate in harness.rust_test_gate_manifest()
            if gate["step"].startswith("s3_33")
        ]
        self.assertEqual([gate[0] for gate in gates], expected_steps)
        self.assertEqual([gate["step"] for gate in manifest_gates], expected_steps)
        self.assertEqual([name for name in harness.LOCAL_STEPS if name.startswith("s3_33")], expected_steps)
        self.assertEqual(len({gate[3] for gate in gates}), len(expected_steps))
        expected_cmp_gates = {
            "s3_33e_native_cmp_rax_imm8_host": (
                "od-host", "tests::native_cmp_rax_imm8_matches_interpreter_and_preserves_gprs",
            ),
            "s3_33e_native_cmp_rax_imm8_compiler": (
                "od-jit", "block_compile::tests::native_cmp_rax_imm8_compiles_eager_and_candidate_branch_consumers",
            ),
            "s3_33e_native_cmp_rax_imm8_admission": (
                "od-jit", "block::tests::cmp_rax_imm8_admission_covers_signed_byte_range_and_rejects_neighbors",
            ),
        }
        expected_xchg_gates = {
            "s3_33i_native_xchg_rax_rbx_admission": (
                "od-jit", "block::tests::xchg_rax_rbx_admission_is_exact_and_separate_from_other_profiles",
            ),
            "s3_33i_native_xchg_rax_rbx_emitter": (
                "od-jit", "block_emitter::tests::dual_write_runtime_shape_is_limited_to_atomic_rax_rbx_exchange",
            ),
            "s3_33i_native_xchg_rax_rbx_compiler": (
                "od-jit", "block_compile::tests::native_xchg_rax_rbx_compiles_with_atomic_pending_statepoints",
            ),
            "s3_33i_native_xchg_rax_rbx_host": (
                "od-host", "tests::native_xchg_rax_rbx_matches_interpreter_and_preserves_flags",
            ),
        }
        expected_not_gates = {
            "s3_33g_native_not_rax_emitter": (
                "od-jit", "block_emitter::tests::native_not_rax_preserving_subtract_emits_no_helper_for_register_homes",
            ),
        }
        expected_lea_disp8_gates = {
            "s3_33j_native_lea_rbx_disp8_admission": (
                "od-jit", "block::tests::lea_rax_rbx_disp8_admission_covers_all_signed_bytes_and_rejects_neighbors",
            ),
            "s3_33j_native_lea_rbx_disp8_emitter": (
                "od-jit", "block_emitter::tests::preserving_integer_support_is_limited_to_w64_add_or_subtract_with_result",
            ),
            "s3_33j_native_lea_rbx_disp8_compiler": (
                "od-jit", "block_compile::tests::native_lea_rax_rbx_disp8_compiles_eager_and_candidate_without_memory_helper",
            ),
            "s3_33j_native_lea_rbx_disp8_host": (
                "od-host", "tests::native_lea_rax_rbx_disp8_matches_interpreter_without_data_access",
            ),
        }
        expected_mov_imm32_gates = {
            "s3_33k_native_mov_rax_imm32_admission": (
                "od-jit", "block::tests::mov_rax_imm32_admission_sign_extends_and_rejects_neighbors",
            ),
            "s3_33k_native_mov_rax_imm32_compiler": (
                "od-jit", "block_compile::tests::native_mov_rax_imm32_compiles_eager_and_candidate_without_flag_helpers",
            ),
            "s3_33k_native_mov_rax_imm32_host": (
                "od-host", "tests::native_mov_rax_imm32_matches_interpreter_full_state_and_preserves_flags",
            ),
        }
        expected_lea_rcx_gates = {
            "s3_33l_native_lea_rbx_rcx_admission": (
                "od-jit", "block::tests::lea_rax_rbx_rcx_admission_is_exact_and_separate_from_other_lea_profiles",
            ),
            "s3_33l_native_lea_rbx_rcx_compiler": (
                "od-jit", "block_compile::tests::native_lea_rax_rbx_rcx_compiles_eager_and_candidate_without_memory_helper",
            ),
            "s3_33l_native_lea_rbx_rcx_host": (
                "od-host", "tests::native_lea_rax_rbx_rcx_matches_interpreter_and_preserves_flags_without_data_access",
            ),
        }
        expected_lea_rip_gates = {
            "s3_33m_native_lea_rax_rip_disp32_admission": (
                "od-jit", "block::tests::lea_rax_rip_disp32_admission_is_exact_and_uses_signed_decoded_disp",
            ),
            "s3_33m_native_lea_rax_rip_disp32_compiler": (
                "od-jit", "block_compile::tests::native_lea_rax_rip_disp32_compiles_eager_and_candidate_from_guest_next_rip",
            ),
            "s3_33m_native_lea_rax_rip_disp32_host": (
                "od-host", "tests::native_lea_rax_rip_disp32_matches_interpreter_without_data_access",
            ),
        }
        gate_by_name = {gate[0]: gate for gate in gates}
        manifest_by_name = {gate["step"]: gate for gate in manifest_gates}
        for step, (package, identity) in {
            **expected_cmp_gates,
            **expected_not_gates,
            **expected_xchg_gates,
            **expected_lea_disp8_gates,
            **expected_mov_imm32_gates,
            **expected_lea_rcx_gates,
            **expected_lea_rip_gates,
        }.items():
            self.assertEqual(gate_by_name[step][2:], (package, identity))
            self.assertEqual(manifest_by_name[step]["package"], package)
            self.assertEqual(manifest_by_name[step]["identity"], identity)
            self.assertEqual(harness.rust_test_argv(package, identity), [
                "cargo", "test", "--offline", "-p", package, "--lib", identity,
                "--", "--exact", "--test-threads=1",
            ])
        self.assertIn("crates/od-jit/src/block.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-jit/src/block_emitter.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-jit/src/block_compile.rs", harness.LOCAL_RUST_TEST_SOURCES)

        with tempfile.TemporaryDirectory(dir=harness.ROOT) as temporary:
            log_dir = Path(temporary)
            with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                summary = passing_final_summary(harness.new_run_id(), log_dir)
            acceptance_steps = summary["suites"]["local_acceptance"]["tests"]
            self.assertEqual(
                [name for name in acceptance_steps if name.startswith("s3_33")],
                expected_steps,
            )
            summary_identities = [
                summary["steps"][name]["test_identity"] for name in expected_steps
            ]
            self.assertEqual(len(set(summary_identities)), len(expected_steps))

    def test_rust_gate_summary_rejects_an_omitted_mandatory_gate(self):
        with tempfile.TemporaryDirectory(dir=harness.ROOT) as temporary:
            mandatory_steps = (
                "s3_33e_native_cmp_rax_imm8_host",
                "s3_33e_native_cmp_rax_imm8_compiler",
                "s3_33e_native_cmp_rax_imm8_admission",
                "s3_33i_native_xchg_rax_rbx_admission",
                "s3_33i_native_xchg_rax_rbx_emitter",
                "s3_33i_native_xchg_rax_rbx_compiler",
                "s3_33i_native_xchg_rax_rbx_host",
                "s3_33g_native_not_rax_emitter",
                "s3_33j_native_lea_rbx_disp8_admission",
                "s3_33j_native_lea_rbx_disp8_emitter",
                "s3_33j_native_lea_rbx_disp8_compiler",
                "s3_33j_native_lea_rbx_disp8_host",
                "s3_33k_native_mov_rax_imm32_admission",
                "s3_33k_native_mov_rax_imm32_compiler",
                "s3_33k_native_mov_rax_imm32_host",
                "s3_33l_native_lea_rbx_rcx_admission",
                "s3_33l_native_lea_rbx_rcx_compiler",
                "s3_33l_native_lea_rbx_rcx_host",
                "s3_33m_native_lea_rax_rip_disp32_admission",
                "s3_33m_native_lea_rax_rip_disp32_compiler",
                "s3_33m_native_lea_rax_rip_disp32_host",
            )
            for missing_step in (harness.ADDITIONAL_TEST_GATES[0][0], *mandatory_steps):
                steps = passing_rust_gate_steps(Path(temporary))
                del steps[missing_step]
                with self.subTest(missing_step=missing_step):
                    with self.assertRaisesRegex(harness.HarnessError, missing_step):
                        harness.validate_required_rust_test_evidence(steps)

    def test_rust_gate_summary_rejects_forged_passed_evidence(self):
        with tempfile.TemporaryDirectory(dir=harness.ROOT) as temporary:
            steps = passing_rust_gate_steps(Path(temporary))
            step_name, _, package, _ = harness.ADDITIONAL_TEST_GATES[0]
            steps[step_name]["output_tail"] = "test tests::unrelated_test ... ok\n"
            with self.assertRaisesRegex(harness.HarnessError, step_name):
                harness.validate_required_rust_test_evidence(steps)

            steps = passing_rust_gate_steps(Path(temporary))
            steps[step_name]["argv"] = harness.rust_test_argv("od-host", "tests::unrelated_test")
            with self.assertRaisesRegex(harness.HarnessError, step_name):
                harness.validate_required_rust_test_evidence(steps)

            steps = passing_rust_gate_steps(Path(temporary))
            steps[step_name]["package"] = "od-host" if package == "od-jit" else "od-jit"
            with self.assertRaisesRegex(harness.HarnessError, step_name):
                harness.validate_required_rust_test_evidence(steps)

            steps = passing_rust_gate_steps(Path(temporary))
            steps[step_name]["test_identity"] = "tests::unrelated_test"
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
            manifest_path = Path(external) / "input_manifest.json"
            manifest_raw = harness.write_json_create(
                manifest_path, harness.input_manifest()
            )
            summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
            expected_log = logs_dir / f"{step_name}.log"
            expected_log.unlink()
            expected_log.symlink_to(external_log)
            try:
                with self.assertRaisesRegex(harness.HarnessError, "log path differs"):
                    harness.validate_final_summary(summary, manifest_path)

                expected_log.unlink()
                expected_log.write_text(output)
                summary["steps"][step_name]["log"] = external_log.relative_to(
                    harness.ROOT
                ).as_posix()
                with self.assertRaisesRegex(harness.HarnessError, "log path differs"):
                    harness.validate_final_summary(summary, manifest_path)
            finally:
                expected_log.unlink(missing_ok=True)
                shutil.rmtree(run_dir)

    def test_vmhost_process_step_rejects_missing_step(self):
        temporary, run_root = temporary_run_root()
        try:
            with self.assertRaisesRegex(harness.HarnessError, "missing, forged, or failed"):
                harness.validate_process_step_evidence(
                    {}, "process_sw_vers", run_root, "a" * 64
                )
        finally:
            temporary.cleanup()

    def test_vmhost_process_step_rejects_malformed_log_evidence(self):
        temporary, run_root = temporary_run_root()
        try:
            step = valid_process_step(run_root)
            step["stdout_sha256"] = "0" * 64
            with self.assertRaisesRegex(harness.HarnessError, "log digest or stream"):
                harness.validate_process_step_evidence(
                    step, "process_sw_vers", run_root, "a" * 64
                )
        finally:
            temporary.cleanup()

    def test_vmhost_process_step_rejects_timeout(self):
        temporary, run_root = temporary_run_root()
        try:
            step = valid_process_step(run_root)
            step.update(status="failed", failure="timeout", returncode=-15)
            with self.assertRaisesRegex(harness.HarnessError, "missing, forged, or failed"):
                harness.validate_process_step_evidence(
                    step, "process_sw_vers", run_root, "a" * 64
                )
        finally:
            temporary.cleanup()

    def test_vmhost_process_step_rejects_forged_passed(self):
        temporary, run_root = temporary_run_root()
        try:
            step = valid_process_step(run_root)
            step["returncode"] = 1
            with self.assertRaisesRegex(harness.HarnessError, "missing, forged, or failed"):
                harness.validate_process_step_evidence(
                    step, "process_sw_vers", run_root, "a" * 64
                )
        finally:
            temporary.cleanup()

    def test_vmhost_process_timeout_kills_child_group(self):
        temporary, run_root = temporary_run_root()
        try:
            child = (
                "import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); "
                "print('child-before-cleanup', flush=True); time.sleep(30)"
            )
            parent = (
                "import subprocess,sys,time; print('parent-before-cleanup', flush=True); "
                "subprocess.Popen([sys.executable, '-c', sys.argv[1]]); "
                "time.sleep(30)"
            )
            with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                step = harness.run_process_step(
                    "process_group_timeout",
                    [sys.executable, "-c", parent, child],
                    run_root,
                    0.2,
                    "a" * 64,
                )
            self.assertEqual(step["failure"], "timeout")
            self.assertEqual(step["status"], "failed")
            stdout_evidence = step["timeout_diagnostics"]["stdout"]
            diagnostic_path = harness.ROOT / stdout_evidence["path"]
            self.assertIn("parent-before-cleanup", diagnostic_path.read_text())
            self.assertIn("child-before-cleanup", diagnostic_path.read_text())
            self.assertFalse(harness.process_group_exists(step["process_group_id"]))
        finally:
            temporary.cleanup()

    def test_vmhost_process_child_log_binding_matches_command(self):
        temporary, run_root = temporary_run_root()
        try:
            for step_name in ("process_control_race", "process_isolation"):
                argv = harness.vmhost_process_commands(run_root)[step_name]
                diagnostics_arg = argv.index("--diagnostics-dir") + 1
                expected_dir = run_root / "logs" / f"{step_name}_children"
                self.assertEqual(Path(argv[diagnostics_arg]), expected_dir)
                expected_dir.mkdir()
                (expected_dir / "child.stdout.log").write_text("stdout\n")
                (expected_dir / "child.stderr.log").write_text("stderr\n")
                self.assertEqual(len(harness.child_log_evidence(run_root, step_name)), 2)
                shutil.rmtree(expected_dir)
        finally:
            temporary.cleanup()

    def test_vmhost_codesign_commands_use_resolved_tool_path(self):
        temporary, run_root = temporary_run_root()
        try:
            resolved_tool = "/toolchain/selected/codesign"
            commands = harness.vmhost_process_commands(run_root, resolved_tool)
            signing_steps = {
                name for name in harness.PROCESS_SUITE_STEPS
                if name.startswith("process_sign_")
                or name.startswith("process_verify_")
                or "_signature" in name
                or "_entitlements" in name
                or name == "process_bundle_verify"
            }
            for name in signing_steps:
                with self.subTest(step=name):
                    self.assertEqual(commands[name][0], resolved_tool)
            self.assertEqual(commands["process_app_bundle"][-1], resolved_tool)
            manifest_commands = harness.vmhost_process_command_manifest()
            manifest_bundle = next(
                command["argv"] for command in manifest_commands
                if command["step"] == "process_app_bundle"
            )
            self.assertEqual(manifest_bundle[-1], "${CODESIGN_PATH}")
        finally:
            temporary.cleanup()

    def test_vmhost_process_manifest_covers_every_gate_once(self):
        temporary, run_root = temporary_run_root()
        try:
            commands = harness.vmhost_process_commands(run_root, "/toolchain/codesign")
            manifest_commands = harness.vmhost_process_command_manifest()
            manifest_names = [command["step"] for command in manifest_commands]
            self.assertEqual(len(harness.PROCESS_SUITE_STEPS), len(set(harness.PROCESS_SUITE_STEPS)))
            self.assertEqual(set(commands), set(harness.PROCESS_SUITE_STEPS))
            self.assertEqual(set(harness.PROCESS_STEP_TIMEOUTS), set(harness.PROCESS_SUITE_STEPS))
            self.assertEqual(len(manifest_names), len(set(manifest_names)))
            self.assertEqual(set(manifest_names), set(harness.PROCESS_SUITE_STEPS))
        finally:
            temporary.cleanup()

    def test_generated_fault_smoke_cleans_child_group_on_success_and_failure(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            smoke_path = harness.ROOT / "tests/system/vmhost_generated_fault_smoke.py"
            spec = importlib.util.spec_from_file_location("vmhost_generated_fault_smoke", smoke_path)
            self.assertIsNotNone(spec)
            smoke = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(smoke)
            for scenario, launcher_returncode, launcher_stderr in (
                ("success", 1, "VMHost terminated by SIGSEGV\\n"),
                ("failure", 0, "unexpected launcher success\\n"),
            ):
                with self.subTest(scenario=scenario):
                    diagnostics = root / f"{scenario}-diagnostics"
                    diagnostics.mkdir()
                    launcher = root / f"{scenario}-launcher"
                    helper = root / f"{scenario}-helper"
                    launcher.touch()
                    helper.touch()
                    group_alive = [True]
                    monotonic_tick = [0.0]
                    observed_signals = []
                    process_group_id = 987654321

                    class FakeProcess:
                        pid = process_group_id
                        returncode = launcher_returncode

                        def __init__(self, _argv, **kwargs):
                            kwargs["stderr"].write(launcher_stderr)

                        def communicate(self, timeout):
                            return None, None

                        def wait(self):
                            return self.returncode

                    def fake_killpg(pid, sig):
                        self.assertEqual(pid, process_group_id)
                        observed_signals.append(sig)
                        if sig == 0:
                            if group_alive[0]:
                                return
                            raise ProcessLookupError
                        if sig == signal.SIGKILL:
                            group_alive[0] = False

                    def fake_monotonic():
                        monotonic_tick[0] += 0.5
                        return monotonic_tick[0]

                    with (
                        mock.patch.object(smoke.sys, "argv", [
                            str(smoke_path), str(launcher), str(helper),
                            "--diagnostics-dir", str(diagnostics),
                        ]),
                        mock.patch.object(smoke.subprocess, "Popen", FakeProcess),
                        mock.patch.object(smoke.os, "killpg", side_effect=fake_killpg),
                        mock.patch.object(smoke.time, "monotonic", side_effect=fake_monotonic),
                        mock.patch.object(smoke.time, "sleep"),
                    ):
                        if scenario == "success":
                            self.assertEqual(smoke.main(), 0)
                        else:
                            with self.assertRaisesRegex(RuntimeError, "did not report"):
                                smoke.main()

                    self.assertIn(signal.SIGTERM, observed_signals)
                    self.assertIn(signal.SIGKILL, observed_signals)
                    self.assertFalse(group_alive[0])
                    self.assertIn(
                        "VMHost terminated by SIGSEGV" if scenario == "success" else "unexpected",
                        (diagnostics / "launcher.stderr.log").read_text(),
                    )

    def test_app_bundle_smoke_preserves_and_executes_five_argument_bundle(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            launcher = root / "launcher"
            launcher.write_text("#!/bin/sh\nprintf '%s\\n' \"$0\" > \"$EXECUTED_PATH\"\n")
            launcher.chmod(0o755)
            vmhost = root / "vmhost"
            vmhost.write_text("stub helper\n")
            entitlements = root / "entitlements.plist"
            entitlements.write_text("<plist/>\n")
            fake_codesign = root / "codesign"
            fake_codesign.write_text("#!/bin/sh\nexit 0\n")
            fake_codesign.chmod(0o755)
            fake_bin = root / "bin"
            fake_bin.mkdir()
            fake_plutil = fake_bin / "plutil"
            fake_plutil.write_text("#!/bin/sh\nexit 0\n")
            fake_plutil.chmod(0o755)
            preserved = root / "preserved.app"
            executed_path = root / "executed-path.txt"
            script = harness.ROOT / "tests/system/vmhost_app_bundle_smoke.sh"
            environment = os.environ.copy()
            environment["EXECUTED_PATH"] = str(executed_path)
            environment["PATH"] = f"{fake_bin}:{environment['PATH']}"
            result = subprocess.run(
                [
                    "sh", str(script), str(launcher), str(vmhost), str(entitlements),
                    str(preserved), str(fake_codesign),
                ],
                cwd=harness.ROOT,
                env=environment,
                capture_output=True,
                text=True,
                timeout=60,
                check=False,
            )
            expected_executable = preserved / "Contents/MacOS/OpenDesktopLauncher"
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue(preserved.is_dir())
            self.assertEqual(Path(executed_path.read_text().strip()), expected_executable)
            self.assertIn(f"vmhost_app_bundle_path={preserved}", result.stdout)
            self.assertIn("vmhost_app_bundle_status=passed", result.stdout)

if __name__ == "__main__":
    unittest.main(verbosity=2)
