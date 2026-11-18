#!/usr/bin/env python3
"""Unit tests for Stage 3 harness evidence validation; no child suites run."""

import copy
import hashlib
import importlib.util
import json
import os
import socket
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

# Keep inventory pins independent of the runtime registries.
EXPECTED_SELF_TESTS = 152
EXPECTED_RUST_GATES = 326
EXPECTED_LOCAL_STEPS = 344

_result_smoke_spec = importlib.util.spec_from_file_location(
    "vmhost_result_state_smoke",
    harness.ROOT / "tests/system/vmhost_result_state_smoke.py",
)
if _result_smoke_spec is None or _result_smoke_spec.loader is None:
    raise RuntimeError("cannot load VMHost RESULT state validator")
result_state_smoke = importlib.util.module_from_spec(_result_smoke_spec)
_result_smoke_spec.loader.exec_module(result_state_smoke)


def temporary_run_root():
    parent = harness.ROOT / "work" / "stage3-harness"
    parent.mkdir(parents=True, exist_ok=True)
    temporary = tempfile.TemporaryDirectory(dir=parent)
    run_root = Path(temporary.name)
    (run_root / "logs").mkdir()
    return temporary, run_root


def temporary_fixture_directory():
    parent = harness.ROOT / "work" / "stage3-harness" / "test-fixtures"
    parent.mkdir(parents=True, exist_ok=True)
    return tempfile.TemporaryDirectory(dir=parent)


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


def write_valid_cache_trace(path: Path):
    def make_input(index: int, rax: int, flags: int):
        return {
            "version": 1,
            "profile": "NativeRegisterV1",
            "input_digest": f"fnv1a64:{index:016x}",
            "code": [0x48, 0x83, 0xC0, 1],
            "guest_base_rip": 0x1000,
            "initial_gpr": [rax, *([0] * 15)],
            "initial_rflags": flags,
        }

    def make_step(index: int, input_value: dict, counters: dict):
        return {
            "sequence_index": index,
            "input": input_value,
            "bindings": {},
            "interpreter": {},
            "uncached": {},
            "cached": {},
            "cache": counters,
        }

    generated = [
        make_step(
            index,
            make_input(index, 0, 0x202),
            {"hits": 0, "misses": 0, "successful_compiles": 0, "successful_publications": 0},
        )
        for index in range(1200)
    ]
    retained = [
        make_step(
            0,
            make_input(5000, 1, 0x202),
            {"hits": 0, "misses": 1, "successful_compiles": 1, "successful_publications": 1},
        ),
        make_step(
            1,
            make_input(5001, 2, 0x203),
            {"hits": 1, "misses": 1, "successful_compiles": 1, "successful_publications": 1},
        ),
    ]
    trace = {
        "version": 1,
        "profile": "NativeRegisterV1",
        "source_inputs_digest": "fnv1a64:" + "a" * 16,
        "segments": [
            {"kind": "generated_corpus", "strategy": "eager", "steps": generated},
            {"kind": "generated_corpus", "strategy": "demand_candidate", "steps": generated},
            {"kind": "retained_hit", "strategy": "eager", "steps": retained},
            {"kind": "retained_hit", "strategy": "demand_candidate", "steps": retained},
        ],
    }
    payload = {
        "version": trace["version"],
        "profile": trace["profile"],
        "source_inputs_digest": trace["source_inputs_digest"],
        "segments": trace["segments"],
    }
    raw = json.dumps(payload, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    value = 0xCBF29CE484222325
    for byte in raw:
        value = ((value ^ byte) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    trace["trace_digest"] = f"fnv1a64:{value:016x}"
    path.write_text(json.dumps(trace, separators=(",", ":"), ensure_ascii=False))
    observations = {
        "version": 1,
        "profile": "NativeRegisterV1",
        "inputs_digest": trace["source_inputs_digest"],
        "cases": [{} for _ in range(1200)],
        "envelope_digest": "",
    }
    observations["envelope_digest"] = harness.native_register_v1_observations_digest(observations)
    (path.parent / "native-register-v1-observations.json").write_text(
        json.dumps(observations, separators=(",", ":"), ensure_ascii=False)
    )
    return trace


def passing_final_summary(run_id: str, log_dir: Path):
    steps = passing_rust_gate_steps(log_dir)
    run_root = harness.ROOT / "work" / "stage3-harness" / run_id
    artifact_dir = run_root / "artifacts" / "native-register-v1"
    artifact_dir.mkdir(parents=True, exist_ok=True)
    for name, _, _, _ in harness.RUST_TEST_GATES:
        if name.startswith(("s3_34n_native_register_v1_", "s3_34o_native_register_v1_", "s3_34p_cached_native_register_v1_")):
            steps[name]["artifact_directory"] = artifact_dir.relative_to(harness.ROOT).as_posix()
    corpus_summary = {
        "profile": "NativeRegisterV1",
        "version": 1,
        "unique_inputs": 1200,
        "seeds": [0x5333346E00000001, 0x5333346E00000002, 0x5333346E00000003],
        "cases_per_seed": 400,
        "repeats_per_backend": 2,
        "full_partial_preserve_xchg_branch_anchors": 3,
        "rip_relative_lea_after_variable_predecessor_cases": 1197,
        "anchor_branch_taken": 1,
        "anchor_branch_fallthrough": 2,
        "code_end_exits": 400,
        "budget_exits": 400,
        "stop_address_exits": 200,
        "stop_requested_exits": 200,
        "fallback_instructions_retired": 100,
        "bounded_loop_inputs": 147,
        "zero_budget_inputs": 1,
        "rip_predecessor_lengths": [3, 4, 7],
        "form_counts": [1] * 13,
        "canonical_inputs_digest": "fnv1a64:" + "a" * 16,
    }
    saved_case = {
        "input": {
            "profile": "NativeRegisterV1",
            "version": 1,
            "input_digest": "fnv1a64:" + "2" * 16,
        },
        "backend_runs": {},
    }
    (artifact_dir / "native-register-v1-summary.json").write_text(json.dumps(corpus_summary))
    (artifact_dir / "native-register-v1-saved-case.json").write_text(json.dumps(saved_case))
    write_valid_cache_trace(artifact_dir / "native-register-v1-cache-trace.json")
    harness.attach_native_register_v1_evidence(
        steps["s3_34n_native_register_v1_corpus"],
        run_root,
        "s3_34n_native_register_v1_corpus",
    )
    for name in (
        "s3_34p_cached_native_register_v1_corpus",
        "s3_34p_cached_native_register_v1_replay",
    ):
        harness.attach_native_register_v1_evidence(steps[name], run_root, name)
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
        "unique_inputs": harness.MEMORY_CORPUS_UNIQUE_INPUTS,
        "replays_per_input": 2,
        "test_outcomes": {},
        "s3_34h_exact_gate": steps[gate_name],
    }
    for name in harness.LOCAL_VMHOST_RESULT_STEPS:
        steps[name] = {"status": "passed"}
    suites[harness.LOCAL_VMHOST_RESULT_SUITE] = {
        "status": "passed",
        "steps": list(harness.LOCAL_VMHOST_RESULT_STEPS),
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
    def test_run_scoped_artifacts_reuse_real_directory_and_reject_symlink(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            artifacts = root / "artifacts"
            existing_evidence = artifacts / "native-register-v1" / "sentinel.json"
            existing_evidence.parent.mkdir(parents=True)
            existing_evidence.write_text('{"preserved":true}\n')

            harness.ensure_real_directory(artifacts, "test artifacts")

            self.assertEqual(existing_evidence.read_text(), '{"preserved":true}\n')

            external = root / "external-artifacts"
            external.mkdir()
            linked_artifacts = root / "linked-artifacts"
            linked_artifacts.symlink_to(external, target_is_directory=True)
            with self.assertRaisesRegex(harness.HarnessError, "real directory"):
                harness.ensure_real_directory(linked_artifacts, "test artifacts")

            missing_target = root / "missing-artifacts-target"
            dangling_artifacts = root / "dangling-artifacts"
            dangling_artifacts.symlink_to(missing_target, target_is_directory=True)
            with self.assertRaisesRegex(harness.HarnessError, "real directory"):
                harness.ensure_real_directory(dangling_artifacts, "test artifacts")
            self.assertFalse(missing_target.exists())

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
        with temporary_fixture_directory() as temporary:
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
        self.assertEqual(count, EXPECTED_SELF_TESTS)
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
            "s3_34n_native_register_v1_corpus",
            "s3_34n_native_register_v1_replay",
            "s3_34n_native_register_v1_selfcheck",
            "s3_34o_native_register_v1_reducer",
            "s3_34o_native_register_v1_reducer_selfcheck",
            "s3_34p_cached_native_register_v1_corpus",
            "s3_34p_cached_native_register_v1_replay",
            "s3_34p_cached_native_register_v1_failure_replay",
            "s3_38e_session_same_code_reuses_publication",
            "s3_38e_session_clear_releases_publication",
            "s3_38e_session_rejects_second_mapping_owner",
            "s3_38e_session_invalid_entry_no_automatic_retry",
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

    def test_s3_30p3_allocator_helper_clobber_gate_is_pinned_and_fails_closed(self):
        p3_step = harness.S3_30P3_GATE_STEP
        p3_identity = harness.S3_30P3_GATE_IDENTITY
        expected_gate = (p3_step, "local_acceptance", "od-host", p3_identity)
        gates = [gate for gate in harness.RUST_TEST_GATES if gate[0] == p3_step]
        self.assertEqual(gates, [expected_gate])
        manifest_gates = [
            gate for gate in harness.rust_test_gate_manifest()
            if gate["step"] == p3_step
        ]
        self.assertEqual(manifest_gates, [{
            "step": p3_step,
            "suite": "local_acceptance",
            "package": "od-host",
            "identity": p3_identity,
        }])
        self.assertEqual(harness.LOCAL_STEPS.count(p3_step), 1)
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.rust_test_gate_manifest()), EXPECTED_RUST_GATES)
        self.assertEqual(harness.rust_test_argv("od-host", p3_identity), [
            "cargo", "test", "--offline", "-p", "od-host", "--lib", p3_identity,
            "--", "--exact", "--test-threads=1",
        ])

        p4_step = "demand_flags_branch_materialization"
        p4_identity = (
            "tests::demand_flags_candidate_materializes_before_jz_and_jnz_after_preserving_mov"
        )
        p4 = [gate for gate in harness.RUST_TEST_GATES if gate[0] == p4_step]
        self.assertEqual(p4, [(p4_step, "local_acceptance", "od-host", p4_identity)])
        self.assertEqual(harness.LOCAL_STEPS.count(p4_step), 1)

        manifest = {
            "gates": harness.rust_test_gate_manifest(),
            "sources": [
                {"path": name, "sha256": harness.sha256_file(harness.ROOT / name)}
                for name in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        manifest["gates"] = [gate for gate in manifest["gates"] if gate["step"] != p3_step]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(manifest)

        run_id = harness.new_run_id()
        run_root = harness.ROOT / "work" / "stage3-harness" / run_id
        try:
            with temporary_fixture_directory() as temporary:
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    summary = passing_final_summary(run_id, Path(temporary))
            self.assertEqual(summary["steps"][p3_step]["test_identity"], p3_identity)
            self.assertEqual(
                summary["suites"]["local_acceptance"]["tests"].count(p3_step), 1
            )
            self.assertEqual(summary["suites"]["local_acceptance"]["tests"].count(p4_step), 1)
        finally:
            shutil.rmtree(run_root, ignore_errors=True)

        temporary, run_root = temporary_run_root()
        try:
            def valid_steps():
                return passing_rust_gate_steps(run_root / "logs")

            steps = valid_steps()
            del steps[p3_step]
            with self.assertRaisesRegex(harness.HarnessError, p3_step):
                harness.validate_required_rust_test_evidence(steps, run_root)

            for output in (
                "test result: ok. 0 passed; 0 failed; 0 ignored\n",
                f"test {p3_identity} ... ignored\n",
            ):
                steps = valid_steps()
                replace_step_log_output(steps[p3_step], output)
                with self.assertRaisesRegex(harness.HarnessError, p3_step):
                    harness.validate_required_rust_test_evidence(steps, run_root)

            steps = valid_steps()
            steps[p3_step]["test_identity"] = "tests::unrelated_test"
            with self.assertRaisesRegex(harness.HarnessError, p3_step):
                harness.validate_required_rust_test_evidence(steps, run_root)

            steps = valid_steps()
            steps[p3_step]["argv"] = harness.rust_test_argv("od-host", "tests::unrelated_test")
            with self.assertRaisesRegex(harness.HarnessError, p3_step):
                harness.validate_required_rust_test_evidence(steps, run_root)

            steps = valid_steps()
            steps[p3_step]["log"] = (run_root / "logs" / "wrong-binding.log").relative_to(
                harness.ROOT
            ).as_posix()
            with self.assertRaisesRegex(harness.HarnessError, "log path differs"):
                harness.validate_required_rust_test_evidence(steps, run_root)

            steps = valid_steps()
            replace_step_log_output(steps[p3_step], "test tests::unrelated_test ... ok\n")
            self.assertEqual(steps[p3_step]["status"], "passed")
            self.assertEqual(steps[p3_step]["returncode"], 0)
            with self.assertRaisesRegex(harness.HarnessError, p3_step):
                harness.validate_required_rust_test_evidence(steps, run_root)
        finally:
            temporary.cleanup()

    def test_s3_38d_result_state_validator_rejects_corruption(self):
        smoke = result_state_smoke
        request_id = 0x380D0001
        budget = 4
        payload = smoke.expected_result(budget)
        valid = smoke.decode_result(
            smoke.RESULT,
            request_id,
            payload,
            expected_request_id=request_id,
            initial_gprs=smoke.INITIAL_GPRS,
            initial_rflags=smoke.INITIAL_RFLAGS,
            budget=budget,
            expected_reason=1,
        )
        self.assertEqual(valid["payload_length"], 64)
        self.assertEqual(valid["instructions_executed"], budget)
        self.assertEqual(valid["rip"], 0)
        self.assertEqual(valid["reserved"], 0)

        def mutate_u64(offset):
            changed = bytearray(payload)
            value = int.from_bytes(changed[offset:offset + 8], "little")
            changed[offset:offset + 8] = (value ^ 1).to_bytes(8, "little")
            return bytes(changed)

        def mutate_u32(offset):
            changed = bytearray(payload)
            value = int.from_bytes(changed[offset:offset + 4], "little")
            changed[offset:offset + 4] = (value ^ 1).to_bytes(4, "little")
            return bytes(changed)

        for label, changed in (
            ("rax", mutate_u64(0)),
            ("rcx", mutate_u64(8)),
            ("rdx", mutate_u64(16)),
            ("rbx", mutate_u64(24)),
            ("rip", mutate_u64(32)),
            ("rflags", mutate_u64(40)),
            ("reason", mutate_u32(48)),
            ("reserved", mutate_u32(52)),
            ("retired", mutate_u64(56)),
            ("short payload", payload[:-1]),
            ("long payload", payload + b"\0"),
        ):
            with self.subTest(corruption=label), self.assertRaises(ValueError):
                smoke.decode_result(
                    smoke.RESULT,
                    request_id,
                    changed,
                    expected_request_id=request_id,
                    initial_gprs=smoke.INITIAL_GPRS,
                    initial_rflags=smoke.INITIAL_RFLAGS,
                    budget=budget,
                    expected_reason=1,
                )

        for label, kind, observed_request_id in (
            ("kind", smoke.STOP_ACK, request_id),
            ("request id", smoke.RESULT, request_id + 1),
        ):
            with self.subTest(corruption=label), self.assertRaises(ValueError):
                smoke.decode_result(
                    kind,
                    observed_request_id,
                    payload,
                    expected_request_id=request_id,
                    initial_gprs=smoke.INITIAL_GPRS,
                    initial_rflags=smoke.INITIAL_RFLAGS,
                    budget=budget,
                    expected_reason=1,
                )

        stop_payload = smoke.expected_result(10, retired=2, reason=2)
        over_budget = bytearray(stop_payload)
        over_budget[56:64] = (11).to_bytes(8, "little")
        over_budget[32:40] = (10).to_bytes(8, "little")
        with self.assertRaisesRegex(ValueError, "beyond budget"):
            smoke.decode_result(
                smoke.RESULT,
                request_id,
                bytes(over_budget),
                expected_request_id=request_id,
                initial_gprs=smoke.INITIAL_GPRS,
                initial_rflags=smoke.INITIAL_RFLAGS,
                budget=10,
                expected_reason=2,
            )

        run_root = harness.ROOT / "work" / "stage3-harness" / "stage3-20261101-000000-0000000000"
        relative_logs = (run_root / "logs").relative_to(harness.ROOT)
        self.assertEqual(
            harness.local_vmhost_result_child_paths(run_root, [{
                "path": (relative_logs / "local_s38d_result_state_children/raw-budget-0.stdout.log").as_posix(),
            }]),
            ["local_s38d_result_state_children/raw-budget-0.stdout.log"],
        )
        with self.assertRaisesRegex(harness.HarnessError, "escaped its run logs"):
            harness.local_vmhost_result_child_paths(run_root, [{
                "path": "work/stage3-harness/other-run/logs/local_s38d_result_state_children/raw-budget-0.stdout.log",
            }])

        suites = {name: {"status": "not_run"} for name in harness.ALL_SUITES}
        suites[harness.LOCAL_VMHOST_RESULT_SUITE] = {"status": "passed"}
        steps = {name: {"status": "not_run"} for name in harness.ALL_STEPS}
        harness.validate_vmhost_result_state_selection(suites, steps)
        for changed_suite, changed_step in (
            ("stage2", None),
            (harness.PROCESS_SUITE_NAME, None),
            (None, harness.PROCESS_SUITE_STEPS[0]),
        ):
            changed_suites = {name: value.copy() for name, value in suites.items()}
            changed_steps = {name: value.copy() for name, value in steps.items()}
            if changed_suite is not None:
                changed_suites[changed_suite] = {"status": "passed"}
            if changed_step is not None:
                changed_steps[changed_step] = {"status": "passed"}
            with self.subTest(changed_suite=changed_suite, changed_step=changed_step):
                with self.assertRaises(harness.HarnessError):
                    harness.validate_vmhost_result_state_selection(changed_suites, changed_steps)
        missing_selected_suite = {name: value.copy() for name, value in suites.items()}
        missing_selected_suite[harness.LOCAL_VMHOST_RESULT_SUITE] = {"status": "not_run"}
        with self.assertRaisesRegex(harness.HarnessError, "selected S3-38d suite"):
            harness.validate_vmhost_result_state_selection(missing_selected_suite, steps)

        class FakeSocket:
            def __init__(self, descriptor):
                self.descriptor = descriptor

            def fileno(self):
                return self.descriptor

            def close(self):
                self.descriptor = -1

            def settimeout(self, _timeout):
                pass

            def sendall(self, _payload):
                pass

        parent, child = FakeSocket(101), FakeSocket(102)

        class FakeProcess:
            pid = 987654321

            def __init__(self):
                self.wait_calls = 0

            def poll(self):
                return None

            def wait(self):
                self.wait_calls += 1
                return -9

        process = FakeProcess()
        with tempfile.TemporaryDirectory() as temporary:
            with (
                mock.patch.object(smoke.socket, "socketpair", return_value=(parent, child)),
                mock.patch.object(smoke.subprocess, "Popen", return_value=process) as popen,
                mock.patch.object(smoke, "read_frame", side_effect=socket.timeout()),
                mock.patch.object(smoke, "terminate_group") as terminate,
                self.assertRaisesRegex(
                    RuntimeError,
                    r"raw-budget-0: timed out after 30s waiting READY after HELLO",
                ),
            ):
                smoke.start_vmhost(Path("fake-vmhost"), Path(temporary), "raw-budget-0")
            popen.assert_called_once()
            self.assertTrue(popen.call_args.kwargs["start_new_session"])
            terminate.assert_called_once_with(process.pid)
            self.assertEqual(process.wait_calls, 1)
            self.assertEqual(parent.fileno(), -1)
            self.assertEqual(child.fileno(), -1)

    def test_s3_32a_candidate_gates_are_unique_and_fail_closed(self):
        expected = [
            (
                "s3_32a_candidate_exact_lowering", "local_acceptance", "od-jit",
                "lowering::tests::s3_32a_candidate_reuses_one_preinstruction_rax_read_only_for_exact_opcode",
            ),
            (
                "s3_32a_candidate_verified_compile_pipeline", "local_acceptance", "od-jit",
                "block_compile::tests::s3_32a_exact_candidate_uses_the_normal_verified_compile_pipeline",
            ),
            (
                "s3_32a_candidate_interpreter_edges_and_resume", "local_acceptance", "od-host",
                "tests::s3_32a_candidate_matches_interpreter_edges_and_budget_resume",
            ),
        ]
        for gate in expected:
            step, suite, package, identity = gate
            self.assertEqual(harness.RUST_TEST_GATES.count(gate), 1)
            self.assertEqual(harness.ADDITIONAL_TEST_GATES.count(gate), 1)
            self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
            self.assertEqual(
                [item for item in harness.rust_test_gate_manifest() if item["step"] == step],
                [{"step": step, "suite": suite, "package": package, "identity": identity}],
            )
            self.assertEqual(harness.rust_test_argv(package, identity), [
                "cargo", "test", "--offline", "-p", package, "--lib", identity,
                "--", "--exact", "--test-threads=1",
            ])
        manifest = harness.rust_test_gate_manifest()
        identities = [gate["identity"] for gate in manifest]
        steps_in_manifest = [gate["step"] for gate in manifest]
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len(set(steps_in_manifest)), EXPECTED_RUST_GATES)
        for _, _, _, identity in expected:
            self.assertEqual(identities.count(identity), 1)
        self.assertEqual(sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308)
        self.assertEqual(sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            [gate["step"] for gate in manifest if gate["step"].startswith("s3_32a_")],
            [gate[0] for gate in expected],
        )
        self.assertIn("crates/od-jit/src/lowering.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0] == harness.S3_30P3_GATE_STEP],
            [(harness.S3_30P3_GATE_STEP, "local_acceptance", "od-host", harness.S3_30P3_GATE_IDENTITY)],
        )
        p4_step = "demand_flags_branch_materialization"
        self.assertEqual(harness.LOCAL_STEPS.count(p4_step), 1)
        self.assertEqual(len([gate for gate in harness.RUST_TEST_GATES if gate[0] == p4_step]), 1)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, package, identity in expected:
                for changed in (
                    {"package": "od-host" if package == "od-jit" else "od-jit"},
                    {"test_identity": "tests::unrelated_test"},
                    {"argv": harness.rust_test_argv(package, "tests::unrelated_test")},
                    {"argv": [*harness.rust_test_argv(package, identity)[:-1], "--test-threads=2"]},
                    {"output_sha256": "f" * 64},
                ):
                    steps = passing_rust_gate_steps(log_dir)
                    steps[step].update(changed)
                    with self.subTest(step=step, changed=changed), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(steps, run_root)

                for output in (
                    "test result: ok. 0 passed; 0 failed; 0 ignored\n",
                    f"test {identity} ... ignored\n",
                    f"test {identity} ... ok\ntest {identity} ... ok\n",
                    "test tests::unrelated_test ... ok\n",
                ):
                    steps = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(steps[step], output)
                    with self.subTest(step=step, output=output), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(steps, run_root)

                steps = passing_rust_gate_steps(log_dir)
                del steps[step]
                with self.subTest(step=step, mutation="missing"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(steps, run_root)

                steps = passing_rust_gate_steps(log_dir)
                steps[step]["log"] = (temporary_dir / f"{step}-wrong.log").relative_to(
                    harness.ROOT
                ).as_posix()
                with self.subTest(step=step, mutation="wrong log binding"), self.assertRaisesRegex(
                    harness.HarnessError, f"log path differs: {step}"
                ):
                    harness.validate_required_rust_test_evidence(steps, run_root)

                outside_log = temporary_dir / f"{step}-outside.log"
                outside_log.write_text(f"test {identity} ... ok\n")
                steps = passing_rust_gate_steps(log_dir)
                expected_log = log_dir / f"{step}.log"
                expected_log.unlink()
                expected_log.symlink_to(outside_log)
                with self.subTest(step=step, mutation="symlink log"), self.assertRaisesRegex(
                    harness.HarnessError, f"log path differs: {step}"
                ):
                    harness.validate_required_rust_test_evidence(steps, run_root)
                expected_log.unlink()
                expected_log.write_text(f"test {identity} ... ok\n")

                bad_manifest = {
                    "gates": harness.rust_test_gate_manifest(),
                    "sources": [
                        {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                        for source in harness.LOCAL_RUST_TEST_SOURCES
                    ],
                }
                harness.validate_rust_test_manifest(bad_manifest)
                index = next(i for i, item in enumerate(bad_manifest["gates"]) if item["step"] == step)
                original = bad_manifest["gates"][index]
                for label, replacement in (("missing", None), ("duplicate", original)):
                    changed_manifest = json.loads(json.dumps(bad_manifest))
                    if replacement is None:
                        del changed_manifest["gates"][index]
                    else:
                        changed_manifest["gates"].append(replacement)
                    with self.subTest(step=step, manifest=label), self.assertRaisesRegex(
                        harness.HarnessError, "exact required Rust test gates"
                    ):
                        harness.validate_rust_test_manifest(changed_manifest)

            bad_source_manifest = json.loads(json.dumps(bad_manifest))
            lowering_source = next(
                source for source in bad_source_manifest["sources"]
                if source["path"] == "crates/od-jit/src/lowering.rs"
            )
            lowering_source["sha256"] = "0" * 64
            with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
                harness.validate_rust_test_manifest(bad_source_manifest)

        run_id = harness.new_run_id()
        run_root = harness.ROOT / "work" / "stage3-harness" / run_id
        try:
            with temporary_fixture_directory() as temporary:
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    summary = passing_final_summary(run_id, Path(temporary))
            self.assertEqual(summary["status"], "local_only")
            self.assertEqual(summary["suites"]["local_acceptance"]["status"], "passed")
            self.assertTrue(all(
                summary["suites"][suite]["status"] == "not_run"
                for suite in harness.UNSELECTED_SUITES
            ))
            self.assertTrue(all(
                summary["steps"][step]["status"] == "not_run"
                for step in harness.PROCESS_SUITE_STEPS
            ))
            for step, _, _, identity in expected:
                self.assertEqual(summary["steps"][step]["test_identity"], identity)
                self.assertEqual(summary["suites"]["local_acceptance"]["tests"].count(step), 1)
            self.assertEqual(
                [step for step in summary["suites"]["local_acceptance"]["tests"] if step.startswith("s3_32a_")],
                [gate[0] for gate in expected],
            )
            self.assertEqual(summary["suites"]["local_acceptance"]["tests"].count(harness.S3_30P3_GATE_STEP), 1)
            self.assertEqual(summary["suites"]["local_acceptance"]["tests"].count(p4_step), 1)
        finally:
            shutil.rmtree(run_root, ignore_errors=True)

    def test_s3_37e_memory_cache_gate_is_pinned_once_and_fail_closed(self):
        expected = (
            "s3_37e_memory_cache_lifecycle_republishes",
            "local_acceptance",
            "od-host",
            "capacity_one_memory_cache_republishes_after_fault_and_backend_error",
        )
        step, suite, package, identity = expected
        self.assertEqual([gate for gate in harness.RUST_TEST_GATES if gate[0] == step], [expected])
        self.assertEqual([gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0] == step], [expected])
        self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(
            [gate for gate in manifest if gate["step"] == step],
            [{"step": step, "suite": suite, "package": package, "identity": identity}],
        )
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308)
        self.assertEqual(sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(harness.rust_test_argv(package, identity), [
            "cargo", "test", "--offline", "-p", package, "--test", "memory_corpus",
            identity, "--", "--exact", "--test-threads=1",
        ])
        self.assertIn("crates/od-host/tests/memory_corpus.rs", harness.LOCAL_RUST_TEST_SOURCES)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for changed in (
                {"package": "od-jit"},
                {"test_identity": "tests::unrelated_test"},
                {"argv": harness.rust_test_argv(package, "tests::unrelated_test")},
                {"argv": [*harness.rust_test_argv(package, identity)[:-1], "--test-threads=2"]},
                {"returncode": 1},
            ):
                steps = passing_rust_gate_steps(log_dir)
                steps[step].update(changed)
                with self.subTest(changed=changed), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(steps, run_root)

            for output in (
                "test result: ok. 0 passed; 0 failed; 0 ignored\n",
                f"test {identity} ... ignored\n",
                f"test {identity} ... ok\ntest {identity} ... ok\n",
                "test tests::unrelated_test ... ok\n",
            ):
                steps = passing_rust_gate_steps(log_dir)
                replace_step_log_output(steps[step], output)
                with self.subTest(output=output), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(steps, run_root)

            steps = passing_rust_gate_steps(log_dir)
            del steps[step]
            with self.assertRaisesRegex(harness.HarnessError, step):
                harness.validate_required_rust_test_evidence(steps, run_root)

            good_manifest = {
                "gates": harness.rust_test_gate_manifest(),
                "sources": [
                    {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                    for source in harness.LOCAL_RUST_TEST_SOURCES
                ],
            }
            harness.validate_rust_test_manifest(good_manifest)
            gate_index = next(i for i, gate in enumerate(good_manifest["gates"]) if gate["step"] == step)
            mutations = (
                ("missing", lambda gates: gates.pop(gate_index)),
                ("duplicate", lambda gates: gates.append(gates[gate_index])),
                ("identity", lambda gates: gates[gate_index].update(identity="tests::unrelated_test")),
                ("package", lambda gates: gates[gate_index].update(package="od-jit")),
            )
            for label, mutate in mutations:
                altered = json.loads(json.dumps(good_manifest))
                mutate(altered["gates"])
                with self.subTest(manifest=label), self.assertRaisesRegex(
                    harness.HarnessError, "exact required Rust test gates"
                ):
                    harness.validate_rust_test_manifest(altered)

            summary_run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / summary_run_id
            try:
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    summary = passing_final_summary(summary_run_id, log_dir)
                self.assertEqual(summary["steps"][step]["test_identity"], identity)
                self.assertEqual(summary["suites"]["local_acceptance"]["tests"].count(step), 1)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)

    def test_s3_23_memory_pair_invalidation_gate_is_pinned_once_and_fail_closed(self):
        expected = (
            "s3_23_memory_pair_cache_invalidation_republishes",
            "local_acceptance",
            "od-host",
            "tests::memory_pair_cache_invalidation_drops_mapping_before_republish",
        )
        step, suite, package, identity = expected
        self.assertEqual(harness.SELF_TEST_NAMES.count(
            "test_s3_23_memory_pair_invalidation_gate_is_pinned_once_and_fail_closed"
        ), 1)
        self.assertEqual([gate for gate in harness.RUST_TEST_GATES if gate[0] == step], [expected])
        self.assertEqual([gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0] == step], [expected])
        self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308)
        self.assertEqual(sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18)
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(
            [gate for gate in manifest if gate["step"] == step],
            [{"step": step, "suite": suite, "package": package, "identity": identity}],
        )
        self.assertEqual(harness.rust_test_argv(package, identity), [
            "cargo", "test", "--offline", "-p", package, "--lib", identity,
            "--", "--exact", "--test-threads=1",
        ])

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for changed in (
                {"package": "od-jit"},
                {"test_identity": "tests::unrelated_test"},
                {"argv": harness.rust_test_argv(package, "tests::unrelated_test")},
                {"returncode": 1},
            ):
                steps = passing_rust_gate_steps(log_dir)
                steps[step].update(changed)
                with self.subTest(changed=changed), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(steps, run_root)

            missing = passing_rust_gate_steps(log_dir)
            del missing[step]
            with self.assertRaisesRegex(harness.HarnessError, step):
                harness.validate_required_rust_test_evidence(missing, run_root)

    def test_s3_18g_dependency_generation_gate_is_pinned_once_and_fail_closed(self):
        expected = (
            "s3_18g_load_load_dependency_generation_republishes",
            "local_acceptance",
            "od-host",
            "tests::s3_18g_load_load_dependency_generation_republishes",
        )
        step, suite, package, identity = expected
        self.assertEqual([gate for gate in harness.RUST_TEST_GATES if gate[0] == step], [expected])
        self.assertEqual([gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0] == step], [expected])
        self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(
            [gate for gate in manifest if gate["step"] == step],
            [{"step": step, "suite": suite, "package": package, "identity": identity}],
        )
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308)
        self.assertEqual(sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            harness.rust_test_argv(package, identity),
            ["cargo", "test", "--offline", "-p", package, "--lib", identity, "--", "--exact", "--test-threads=1"],
        )

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for changed in (
                {"package": "od-jit"},
                {"test_identity": "tests::unrelated_test"},
                {"argv": ["cargo", "test"]},
                {"returncode": 1},
            ):
                steps = passing_rust_gate_steps(log_dir)
                steps[step].update(changed)
                with self.subTest(changed=changed), self.assertRaisesRegex(harness.HarnessError, step):
                    harness.validate_required_rust_test_evidence(steps, run_root)
            zero_match = passing_rust_gate_steps(log_dir)
            replace_step_log_output(
                zero_match[step],
                "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
            )
            with self.assertRaisesRegex(harness.HarnessError, step):
                harness.validate_required_rust_test_evidence(zero_match, run_root)
            missing_evidence = passing_rust_gate_steps(log_dir)
            del missing_evidence[step]
            with self.assertRaisesRegex(harness.HarnessError, step):
                harness.validate_required_rust_test_evidence(missing_evidence, run_root)

            source_manifest = {
                "gates": manifest,
                "sources": [
                    {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                    for source in harness.LOCAL_RUST_TEST_SOURCES
                ],
            }
            harness.validate_rust_test_manifest(source_manifest)
            for mutate in (
                lambda gates: gates.pop(next(i for i, gate in enumerate(gates) if gate["step"] == step)),
                lambda gates: gates[next(i for i, gate in enumerate(gates) if gate["step"] == step)].update(identity="tests::unrelated_test"),
            ):
                altered = json.loads(json.dumps(source_manifest))
                mutate(altered["gates"])
                with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
                    harness.validate_rust_test_manifest(altered)

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
            "s3_33n_native_register_v1_admission",
            "s3_33n_native_register_v1_compiler",
            "s3_33n_native_register_v1_host",
        ]
        gates = [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_33")]
        manifest_gates = [
            gate for gate in harness.rust_test_gate_manifest()
            if gate["step"].startswith("s3_33")
        ]
        self.assertEqual([gate[0] for gate in gates], expected_steps)
        self.assertEqual([gate["step"] for gate in manifest_gates], expected_steps)
        self.assertEqual([name for name in harness.LOCAL_STEPS if name.startswith("s3_33")], expected_steps)
        self.assertEqual(len(expected_steps), 42)
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
        expected_register_v1_gates = {
            "s3_33n_native_register_v1_admission": (
                "od-jit", "block::tests::native_register_v1_admission_is_exact_and_combines_all_committed_forms",
            ),
            "s3_33n_native_register_v1_compiler": (
                "od-jit", "block_compile::tests::native_register_v1_mixed_compiler_preserves_statepoints_and_boundaries",
            ),
            "s3_33n_native_register_v1_host": (
                "od-host", "tests::native_register_v1_mixed_matches_interpreter_and_bounded_ordered_pair_corpus",
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
            **expected_register_v1_gates,
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
        run_id = harness.new_run_id()
        run_root = harness.ROOT / "work" / "stage3-harness" / run_id
        try:
            with temporary_fixture_directory() as temporary:
                log_dir = Path(temporary)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    summary = passing_final_summary(run_id, log_dir)
                acceptance_steps = summary["suites"]["local_acceptance"]["tests"]
                self.assertEqual(
                    [name for name in acceptance_steps if name.startswith("s3_33")],
                    expected_steps,
                )
                summary_identities = [
                    summary["steps"][name]["test_identity"] for name in expected_steps
                ]
                self.assertEqual(len(set(summary_identities)), len(expected_steps))
        finally:
            shutil.rmtree(run_root, ignore_errors=True)

    def test_s3_34n_native_register_corpus_gates_are_pinned_once(self):
        expected = [
            (
                "s3_34n_native_register_v1_corpus", "od-host",
                "tests::native_register_v1_corpus::generated_corpus_runs_twice_across_all_backends",
            ),
            (
                "s3_34n_native_register_v1_replay", "od-host",
                "tests::native_register_v1_corpus::saved_case_round_trip_and_replay",
            ),
            (
                "s3_34n_native_register_v1_selfcheck", "od-host",
                "tests::native_register_v1_corpus::comparator_and_envelope_fail_closed",
            ),
        ]
        gates = [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34n_")]
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(sum(name.startswith("s3_33") for name, _, _, _ in harness.RUST_TEST_GATES), 42)
        self.assertEqual([(name, package, identity) for name, _, package, identity in gates], expected)
        self.assertIn("crates/od-host/src/tests/native_register_v1_corpus.rs", harness.LOCAL_RUST_TEST_SOURCES)
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        native_identities = {
            gate["identity"] for gate in manifest if gate["step"].startswith("s3_33")
        }
        self.assertEqual(len(native_identities), 42)
        with temporary_fixture_directory() as temporary:
            run_dir = Path(temporary)
            wrong_path = {
                "artifact_directory": "work/other-run/artifacts/native-register-v1"
            }
            with self.assertRaisesRegex(harness.HarnessError, "artifact directory"):
                harness.attach_native_register_v1_evidence(
                    wrong_path, run_dir, "s3_34n_native_register_v1_corpus"
                )
            missing = {
                "artifact_directory": (
                    run_dir / "artifacts" / "native-register-v1"
                ).relative_to(harness.ROOT).as_posix()
            }
            with self.assertRaisesRegex(harness.HarnessError, "evidence is missing"):
                harness.attach_native_register_v1_evidence(
                    missing, run_dir, "s3_34n_native_register_v1_corpus"
                )
            artifact_rel = (
                run_dir / "artifacts" / "native-register-v1"
            ).relative_to(harness.ROOT).as_posix()
            artifact_dir = run_dir / "artifacts" / "native-register-v1"
            steps = {
                name: {"status": "passed", "artifact_directory": artifact_rel}
                for name, _, _, _ in harness.RUST_TEST_GATES
                if name.startswith(("s3_34n_native_register_v1_", "s3_34o_native_register_v1_", "s3_34p_cached_native_register_v1_"))
            }
            with self.assertRaisesRegex(harness.HarnessError, "summary evidence is missing"):
                harness.validate_native_register_v1_artifacts(steps, run_dir)

            external_dir = run_dir / "external"
            external_dir.mkdir()
            artifacts_link = run_dir / "artifacts"
            artifacts_link.symlink_to(external_dir, target_is_directory=True)
            with self.assertRaisesRegex(harness.HarnessError, "artifact directory"):
                harness.attach_native_register_v1_evidence(
                    steps["s3_34n_native_register_v1_corpus"],
                    run_dir,
                    "s3_34n_native_register_v1_corpus",
                )
            artifacts_link.unlink()
            artifact_dir.mkdir(parents=True)
            outside_file = run_dir / "external-case.json"
            outside_file.write_text("{}")
            (artifact_dir / "native-register-v1-summary.json").symlink_to(outside_file)
            with self.assertRaisesRegex(harness.HarnessError, "artifact file cannot be a symlink"):
                harness.attach_native_register_v1_evidence(
                    steps["s3_34n_native_register_v1_corpus"],
                    run_dir,
                    "s3_34n_native_register_v1_corpus",
                )
            harness_run = run_dir / "harness-run"
            log_dir = harness_run / "logs"
            log_dir.mkdir(parents=True)
            replay_step, _, package, identity = next(
                gate for gate in harness.RUST_TEST_GATES
                if gate[0] == "s3_34n_native_register_v1_replay"
            )
            argv = harness.rust_test_argv(package, identity)
            fake_process = type("Process", (), {
                "stdout": f"test {identity} ... ok\n", "stderr": "", "returncode": 0
            })()
            with (
                mock.patch.object(harness.subprocess, "run", return_value=fake_process) as mocked_run,
                mock.patch.object(harness, "source_fingerprint", return_value="a" * 64),
                mock.patch.dict(os.environ, {"OD_JIT_NATIVE_REGISTER_V1_REPLAY_CASE": "/external/forged.json"}),
            ):
                step = harness.run_step(replay_step, argv, harness_run, 30, "a" * 64)
            self.assertEqual(step["status"], "passed")
            expected_replay = str(
                harness_run / "artifacts" / "native-register-v1" / "native-register-v1-saved-case.json"
            )
            self.assertEqual(
                mocked_run.call_args.kwargs["env"]["OD_JIT_NATIVE_REGISTER_V1_REPLAY_CASE"],
                expected_replay,
            )
            failure_step, _, failure_package, failure_identity = next(
                gate for gate in harness.RUST_TEST_GATES
                if gate[0] == "s3_34p_cached_native_register_v1_failure_replay"
            )
            failure_argv = harness.rust_test_argv(failure_package, failure_identity)
            with (
                mock.patch.object(harness.subprocess, "run", return_value=fake_process) as failure_process,
                mock.patch.object(harness, "source_fingerprint", return_value="a" * 64),
                mock.patch.dict(
                    os.environ,
                    {"OD_JIT_NATIVE_REGISTER_V1_FAILURE_TRACE": "/external/old-failure.json"},
                ),
            ):
                failure_result = harness.run_step(failure_step, failure_argv, harness_run, 30, "a" * 64)
            self.assertEqual(failure_result["status"], "passed")
            self.assertEqual(
                failure_result["artifact_directory"],
                (harness_run / "artifacts" / "native-register-v1").relative_to(harness.ROOT).as_posix(),
            )
            self.assertEqual(
                failure_process.call_args.kwargs["env"]["OD_JIT_NATIVE_REGISTER_V1_CORPUS_DIR"],
                str(harness_run / "artifacts" / "native-register-v1"),
            )
            self.assertNotIn(
                "OD_JIT_NATIVE_REGISTER_V1_FAILURE_TRACE",
                failure_process.call_args.kwargs["env"],
            )

        run_id = harness.new_run_id()
        harness_run = harness.ROOT / "work" / "stage3-harness" / run_id
        (harness_run / "logs").mkdir(parents=True)
        try:
            with temporary_fixture_directory() as temporary:
                summary = passing_final_summary(run_id, harness_run / "logs")
                manifest_path = Path(temporary) / "input_manifest.json"
                manifest_raw = harness.write_json_create(manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                summary["steps"]["s3_34n_native_register_v1_corpus"].pop("corpus_evidence")
                with mock.patch.object(harness, "validate_local_vmhost_result_state"):
                    with self.assertRaisesRegex(harness.HarnessError, "summary evidence is missing"):
                        harness.validate_final_summary(summary, manifest_path)
        finally:
            shutil.rmtree(harness_run, ignore_errors=True)

    def test_s3_34p_cached_native_register_trace_gates_are_pinned_once(self):
        expected = [
            (
                "s3_34p_cached_native_register_v1_corpus", "od-host",
                "tests::native_register_v1_corpus::cached_corpus_matches_existing_observations_and_saves_versioned_trace",
            ),
            (
                "s3_34p_cached_native_register_v1_replay", "od-host",
                "tests::native_register_v1_corpus::cached_trace_replays_from_empty_cache_and_rejects_corruption",
            ),
            (
                "s3_34p_cached_native_register_v1_failure_replay", "od-host",
                "tests::native_register_v1_corpus::cached_failure_trace_replays_saved_prefix_and_failure",
            ),
        ]
        gates = [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34p_")]
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.rust_test_gate_manifest()), EXPECTED_RUST_GATES)
        self.assertEqual(sum(name.startswith("s3_33") for name, _, _, _ in harness.RUST_TEST_GATES), 42)
        self.assertEqual([(name, package, identity) for name, _, package, identity in gates], expected)
        self.assertIn("crates/od-host/src/tests/native_register_v1_corpus.rs", harness.LOCAL_RUST_TEST_SOURCES)

        def counter_snapshot(hits, misses, compiles, publications):
            return {
                "hits": hits,
                "misses": misses,
                "successful_compiles": compiles,
                "successful_publications": publications,
            }

        def trace_step(index, input_value, counters):
            return {
                "sequence_index": index,
                "input": input_value,
                "bindings": {},
                "interpreter": {},
                "uncached": {},
                "cached": {},
                "cache": counters,
            }

        def input_value(index, rax=0, rflags=0x202):
            return {
                "version": 1,
                "profile": "NativeRegisterV1",
                "input_digest": f"fnv1a64:{index:016x}",
                "code": [0x48, 0x83, 0xC0, 1],
                "guest_base_rip": 0x1000,
                "initial_gpr": [rax, *([0] * 15)],
                "initial_rflags": rflags,
            }

        generated = [
            trace_step(index, input_value(index), counter_snapshot(0, 0, 0, 0))
            for index in range(1200)
        ]
        retained = [
            trace_step(0, input_value(5000, 1, 0x202), counter_snapshot(0, 1, 1, 1)),
            trace_step(1, input_value(5001, 2, 0x203), counter_snapshot(1, 1, 1, 1)),
        ]
        trace = {
            "version": 1,
            "profile": "NativeRegisterV1",
            "source_inputs_digest": "fnv1a64:" + "a" * 16,
            "segments": [
                {"kind": "generated_corpus", "strategy": "eager", "steps": generated},
                {"kind": "generated_corpus", "strategy": "demand_candidate", "steps": generated},
                {"kind": "retained_hit", "strategy": "eager", "steps": retained},
                {"kind": "retained_hit", "strategy": "demand_candidate", "steps": retained},
            ],
        }
        payload = {
            "version": trace["version"],
            "profile": trace["profile"],
            "source_inputs_digest": trace["source_inputs_digest"],
            "segments": trace["segments"],
        }
        raw = json.dumps(payload, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
        value = 0xCBF29CE484222325
        for byte in raw:
            value = ((value ^ byte) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
        trace["trace_digest"] = f"fnv1a64:{value:016x}"

        with temporary_fixture_directory() as temporary:
            run_dir = Path(temporary)
            artifact_dir = run_dir / "artifacts" / "native-register-v1"
            artifact_dir.mkdir(parents=True)
            trace_path = artifact_dir / "native-register-v1-cache-trace.json"
            trace_path.write_text(json.dumps(trace, separators=(",", ":"), ensure_ascii=False))
            observations = {
                "version": 1,
                "profile": "NativeRegisterV1",
                "inputs_digest": trace["source_inputs_digest"],
                "cases": [{} for _ in range(1200)],
                "envelope_digest": "",
            }
            observations["envelope_digest"] = harness.native_register_v1_observations_digest(observations)
            (artifact_dir / "native-register-v1-observations.json").write_text(
                json.dumps(observations, separators=(",", ":"), ensure_ascii=False)
            )
            expected_dir = artifact_dir.relative_to(harness.ROOT).as_posix()
            steps = {
                name: {"status": "passed", "artifact_directory": expected_dir}
                for name, _, _ in expected
            }
            for name in steps:
                harness.attach_native_register_v1_cache_trace_evidence(steps[name], run_dir)
            harness.validate_native_register_v1_cache_trace_artifacts(steps, run_dir)

            observations_path = artifact_dir / "native-register-v1-observations.json"
            original_observations = observations_path.read_bytes()
            tampered_observations = json.loads(original_observations)
            tampered_observations["cases"][0]["tampered"] = True
            tampered_observations["envelope_digest"] = harness.native_register_v1_observations_digest(
                tampered_observations
            )
            observations_path.write_text(json.dumps(tampered_observations, separators=(",", ":")))
            with self.assertRaisesRegex(harness.HarnessError, "evidence is missing or forged"):
                harness.validate_native_register_v1_cache_trace_artifacts(steps, run_dir)
            observations_path.write_bytes(original_observations)

            bad_trace = dict(trace)
            bad_trace["trace_digest"] = "fnv1a64:" + "0" * 16
            trace_path.write_text(json.dumps(bad_trace, separators=(",", ":"), ensure_ascii=False))
            with self.assertRaisesRegex(harness.HarnessError, "digest"):
                harness.native_register_v1_cache_trace_evidence(run_dir)

            outside = run_dir / "outside.json"
            outside.write_text("{}")
            trace_path.unlink()
            trace_path.symlink_to(outside)
            with self.assertRaisesRegex(harness.HarnessError, "symlink"):
                harness.native_register_v1_cache_trace_evidence(run_dir)

    def test_s3_38b_native_register_cache_gates_are_pinned_once(self):
        expected = {
            "s3_38b_native_register_v1_cache_profile_invalidation": (
                "od-jit", "dispatch::tests::native_cached_profile_isolated_and_dependency_context_changes_miss",
            ),
            "s3_38b_native_register_v1_cache_guards_capacity": (
                "od-jit", "dispatch::tests::native_cached_guards_and_failures_preserve_publication_and_entry_counts",
            ),
            "s3_38b_native_register_v1_host_reuse_lifetime_parity": (
                "od-host", "tests::cached_native_register_v1_reuses_eager_block_with_full_state_parity",
            ),
            "s3_38c_native_register_v1_strategy_key_isolation": (
                "od-jit", "dispatch::tests::native_cached_demand_candidate_strategy_key_isolated",
            ),
            "s3_38c_native_register_v1_candidate_lifecycle_guards": (
                "od-jit", "dispatch::tests::native_cached_demand_candidate_guards_and_lifetime",
            ),
            "s3_38c_native_register_v1_candidate_cold_hit_parity": (
                "od-host", "tests::cached_native_register_v1_demand_candidate_cold_hit_full_state_parity",
            ),
        }
        gate_by_name = {gate[0]: gate for gate in harness.RUST_TEST_GATES}
        manifest_by_name = {
            gate["step"]: gate for gate in harness.rust_test_gate_manifest()
        }
        for step, (package, identity) in expected.items():
            self.assertEqual(gate_by_name[step][2:], (package, identity))
            self.assertEqual(manifest_by_name[step]["package"], package)
            self.assertEqual(manifest_by_name[step]["identity"], identity)
            self.assertEqual(harness.rust_test_argv(package, identity), [
                "cargo", "test", "--offline", "-p", package, "--lib", identity,
                "--", "--exact", "--test-threads=1",
            ])
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest_by_name), EXPECTED_RUST_GATES)

    def test_s3_38e_session_lifecycle_gates_are_pinned_once_and_fail_closed(self):
        expected = (
            (
                "s3_38e_session_same_code_reuses_publication", "local_lifecycle", "od-host",
                "session::tests::same_code_reuses_publication_with_fresh_state_budget_and_stop",
            ),
            (
                "s3_38e_session_clear_releases_publication", "local_lifecycle", "od-host",
                "session::tests::clear_releases_publication_and_mapping_lease",
            ),
            (
                "s3_38e_session_rejects_second_mapping_owner", "local_lifecycle", "od-host",
                "session::tests::retained_session_rejects_a_second_executable_memory_owner",
            ),
            (
                "s3_38e_session_invalid_entry_no_automatic_retry", "local_lifecycle", "od-host",
                "session::tests::invalid_entry_is_reported_without_an_automatic_retry",
            ),
        )
        self.assertIn(
            "test_s3_38e_session_lifecycle_gates_are_pinned_once_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(len(harness.SELF_TEST_NAMES), EXPECTED_SELF_TESTS)
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_38e_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_38e_")],
            list(expected),
        )

        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        self.assertEqual(harness.LOCAL_RUST_TEST_SOURCES.count("crates/od-host/src/session.rs"), 1)

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", "od-host", "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_gate_manifest = json.loads(json.dumps(source_manifest))
            missing_gate_manifest["gates"] = [
                gate for gate in missing_gate_manifest["gates"] if gate["step"] != step
            ]
            with self.subTest(missing_manifest_gate=step), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_gate_manifest)

        missing_source_manifest = json.loads(json.dumps(source_manifest))
        missing_source_manifest["sources"] = [
            source for source in missing_source_manifest["sources"]
            if source["path"] != "crates/od-host/src/session.rs"
        ]
        with self.assertRaisesRegex(harness.HarnessError, "required Rust test sources"):
            harness.validate_rust_test_manifest(missing_source_manifest)

        run_id = harness.new_run_id()
        run_root = harness.ROOT / "work" / "stage3-harness" / run_id
        try:
            with temporary_fixture_directory() as temporary:
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    summary = passing_final_summary(run_id, Path(temporary))
            lifecycle_steps = summary["suites"]["local_lifecycle"]["tests"]
            for step, _, _, identity in expected:
                self.assertEqual(summary["steps"][step]["test_identity"], identity)
                self.assertEqual(lifecycle_steps.count(step), 1)
        finally:
            shutil.rmtree(run_root, ignore_errors=True)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, _ in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(missing_evidence=step), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

            step, _, package, identity = expected[0]
            for changed in (
                {"test_identity": "session::tests::unrelated_test"},
                {"argv": harness.rust_test_argv(package, "session::tests::unrelated_test")},
            ):
                changed_steps = passing_rust_gate_steps(log_dir)
                changed_steps[step].update(changed)
                with self.subTest(changed=changed), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(changed_steps, run_root)

            for output in (
                "test result: ok. 0 passed; 0 failed; 0 ignored\n",
                f"test {identity} ... ignored\n",
                f"test {identity} ... FAILED\n",
            ):
                changed_steps = passing_rust_gate_steps(log_dir)
                replace_step_log_output(changed_steps[step], output)
                with self.subTest(output=output), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(changed_steps, run_root)

    def test_s3_34q_retained_hit_reducer_gate_is_pinned_once(self):
        expected = (
            "s3_34q_retained_hit_state_reducer_selfcheck",
            "local_acceptance",
            "od-host",
            "tests::native_register_v1_corpus::retained_hit_failure_reducer_and_selfcheck",
        )
        gates = [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34q_")]
        self.assertEqual(gates, [expected])
        self.assertIn(expected[0], harness.LOCAL_STEPS)

    def test_s3_34r_cross_thread_idle_handle_gate_is_pinned_once_and_fail_closed(self):
        expected = (
            "s3_34r_abi_idle_handle_cross_thread_mapping_release",
            "local_acceptance",
            "od-host",
            "tests::c_abi_idle_handle_runs_sequentially_on_host_threads_and_releases_mapping",
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0] == expected[0]],
            [expected],
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0] == expected[0]],
            [expected],
        )
        self.assertEqual(harness.LOCAL_STEPS.count(expected[0]), 1)
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(sum(gate["step"] == expected[0] for gate in manifest), 1)
        self.assertEqual(
            [gate for gate in manifest if gate["step"] == expected[0]],
            [{"step": expected[0], "suite": expected[1], "package": expected[2], "identity": expected[3]}],
        )
        exact_argv = [
            "cargo", "test", "--offline", "-p", "od-host", "--lib", expected[3],
            "--", "--exact", "--test-threads=1",
        ]
        self.assertEqual(harness.rust_test_argv(expected[2], expected[3]), exact_argv)

        with temporary_fixture_directory() as temporary:
            log_dir = Path(temporary)

            steps = passing_rust_gate_steps(log_dir)
            del steps[expected[0]]
            with self.assertRaisesRegex(harness.HarnessError, expected[0]):
                harness.validate_required_rust_test_evidence(steps)

            for changed in (
                {"package": "od-jit"},
                {"test_identity": "tests::unrelated_test"},
                {"argv": harness.rust_test_argv("od-host", "tests::unrelated_test")},
                {"argv": [*exact_argv[:-1], "--test-threads=2"]},
            ):
                steps = passing_rust_gate_steps(log_dir)
                steps[expected[0]].update(changed)
                with self.subTest(changed=changed), self.assertRaisesRegex(
                    harness.HarnessError, expected[0]
                ):
                    harness.validate_required_rust_test_evidence(steps)

            for output in (
                "test result: ok. 0 passed; 0 failed; 0 ignored\n",
                f"test {expected[3]} ... ignored\n",
                f"test {expected[3]} ... ok\ntest {expected[3]} ... ok\n",
                "test tests::unrelated_test ... ok\n",
            ):
                steps = passing_rust_gate_steps(log_dir)
                replace_step_log_output(steps[expected[0]], output)
                with self.subTest(output=output), self.assertRaisesRegex(
                    harness.HarnessError, expected[0]
                ):
                    harness.validate_required_rust_test_evidence(steps)

            gate_manifest = {
                "gates": [gate for gate in manifest if gate["step"] != expected[0]],
                "sources": [
                    {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                    for source in harness.LOCAL_RUST_TEST_SOURCES
                ],
            }
            with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
                harness.validate_rust_test_manifest(gate_manifest)

            run_id = harness.new_run_id()
            run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[0])
                manifest_path = Path(temporary) / "input_manifest.json"
                manifest_raw = harness.write_json_create(manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, manifest_path)
            finally:
                shutil.rmtree(run_root, ignore_errors=True)

    def test_s3_34s_memory_event_gates_are_pinned_once_and_fail_closed(self):
        expected = [
            (
                "s3_34s_memory_event_bare_nop_bridge_budget_effects", "local_acceptance", "od-host",
                "tests::bounded_memory_dispatch_opt_in_bare_nop_bridge_preserves_budget_and_effects",
            ),
            (
                "s3_34s_memory_event_after_publication_stop_wins", "local_acceptance", "od-jit",
                "memory_dispatch::tests::event_after_publication_skips_entry_and_stop_wins",
            ),
            (
                "s3_34s_memory_event_malformed_pending_accounting", "local_acceptance", "od-jit",
                "memory_dispatch::tests::malformed_event_pending_executor_accounting_is_rejected",
            ),
            (
                "s3_34s_memory_event_yield_before_and_after_committed_artifact", "local_acceptance", "od-jit",
                "memory_dispatch::tests::controlled_event_yields_before_work_and_after_one_committed_artifact",
            ),
            (
                "s3_34s_memory_event_terminal_exit_stop_not_masked", "local_acceptance", "od-jit",
                "memory_dispatch::tests::completed_artifact_terminal_exit_and_stop_are_not_masked_by_event",
            ),
            (
                "s3_34s_memory_event_stop_boundary_budget_priority", "local_acceptance", "od-jit",
                "memory_dispatch::tests::controlled_event_respects_stop_boundary_and_budget_priority",
            ),
            (
                "s3_34s_memory_event_nop_fallback_resume_without_replay", "local_acceptance", "od-jit",
                "memory_dispatch::tests::nop_fallback_event_yields_before_nop_and_resumes_without_replay",
            ),
        ]
        observed = [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34s_")]
        self.assertEqual(observed, expected)
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34s_")],
            expected,
        )
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertIn("crates/od-jit/src/memory_dispatch.rs", harness.LOCAL_RUST_TEST_SOURCES)
        for step, suite, package, identity in expected:
            self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
            exact_manifest_entry = {
                "step": step,
                "suite": suite,
                "package": package,
                "identity": identity,
            }
            self.assertEqual([gate for gate in manifest if gate["step"] == step], [exact_manifest_entry])
            self.assertEqual(harness.rust_test_argv(package, identity), [
                "cargo", "test", "--offline", "-p", package, "--lib", identity,
                "--", "--exact", "--test-threads=1",
            ])

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, _ in expected:
                changed_steps = passing_rust_gate_steps(log_dir)
                changed_steps[step]["log"] = (temporary_dir / f"{step}.log").relative_to(harness.ROOT).as_posix()
                with self.subTest(step=step, mutation="wrong per-run log"), self.assertRaisesRegex(
                    harness.HarnessError, f"log path differs: {step}"
                ):
                    harness.validate_required_rust_test_evidence(changed_steps, run_root)

            for expected_gate in expected:
                step, _, package, identity = expected_gate
                mutations = (
                    {"package": "od-host" if package == "od-jit" else "od-jit"},
                    {"test_identity": "tests::unrelated_test"},
                    {"argv": harness.rust_test_argv(package, "tests::unrelated_test")},
                    {"argv": [*harness.rust_test_argv(package, identity)[:-1], "--test-threads=2"]},
                    {"returncode": 1},
                )
                for changed in mutations:
                    steps = passing_rust_gate_steps(log_dir)
                    steps[step].update(changed)
                    with self.subTest(step=step, changed=changed), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(steps, run_root)

                for output in (
                    "test result: ok. 0 passed; 0 failed; 0 ignored\n",
                    f"test {identity} ... ignored\n",
                    f"test {identity} ... ok\ntest {identity} ... ok\n",
                    "test tests::unrelated_test ... ok\n",
                ):
                    steps = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(steps[step], output)
                    with self.subTest(step=step, output=output), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(steps, run_root)

                steps = passing_rust_gate_steps(log_dir)
                del steps[step]
                with self.subTest(step=step, mutation="missing"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(steps, run_root)

                gate_manifest = {
                    "gates": harness.rust_test_gate_manifest(),
                    "sources": [
                        {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                        for source in harness.LOCAL_RUST_TEST_SOURCES
                    ],
                }
                entry_index = next(i for i, gate in enumerate(gate_manifest["gates"]) if gate["step"] == step)
                original_entry = gate_manifest["gates"][entry_index]
                manifest_mutations = (
                    ("missing", None),
                    ("duplicate", original_entry),
                    ("wrong package", {**original_entry, "package": "od-jit" if package == "od-host" else "od-host"}),
                    ("wrong identity", {**original_entry, "identity": "tests::unrelated_test"}),
                )
                for label, replacement in manifest_mutations:
                    changed_manifest = json.loads(json.dumps(gate_manifest))
                    if replacement is None:
                        del changed_manifest["gates"][entry_index]
                    elif label == "duplicate":
                        changed_manifest["gates"].append(replacement)
                    else:
                        changed_manifest["gates"][entry_index] = replacement
                    with self.subTest(step=step, manifest=label), self.assertRaisesRegex(
                        harness.HarnessError, "exact required Rust test gates"
                    ):
                        harness.validate_rust_test_manifest(changed_manifest)

            run_id = harness.new_run_id()
            run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, run_root / "logs")
                for step, _, _, _ in expected:
                    summary["suites"]["local_acceptance"]["tests"].remove(step)
                manifest_path = log_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, manifest_path)
            finally:
                shutil.rmtree(run_root, ignore_errors=True)

    def test_s3_34t_register_event_gates_are_pinned_once_and_fail_closed(self):
        expected = [
            (
                "s3_34t_register_event_cache_miss_hit_request", "local_acceptance", "od-jit",
                "dispatch::tests::event_pending_after_native_cache_miss_and_hit_preserves_request_until_cleared",
            ),
            (
                "s3_34t_register_event_publication_prevents_entry", "local_acceptance", "od-jit",
                "dispatch::tests::event_set_during_publication_prevents_native_entry",
            ),
            (
                "s3_34t_register_event_multi_cache_capacity", "local_acceptance", "od-jit",
                "dispatch::tests::event_pending_on_multi_cache_hit_and_miss_obeys_capacity",
            ),
            (
                "s3_34t_register_event_jcc_resume_without_replay", "local_acceptance", "od-jit",
                "dispatch::tests::event_after_jcc_yields_before_nop_and_resumes_without_replaying_prefix",
            ),
            (
                "s3_34t_register_event_control_priority", "local_acceptance", "od-jit",
                "dispatch::tests::event_request_obeys_stop_code_end_budget_and_error_priority",
            ),
        ]
        observed = [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34t_")]
        self.assertEqual(observed, expected)
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34t_")],
            expected,
        )
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertIn("crates/od-jit/src/dispatch.rs", harness.LOCAL_RUST_TEST_SOURCES)
        for step, suite, package, identity in expected:
            self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
            self.assertEqual(
                [gate for gate in manifest if gate["step"] == step],
                [{"step": step, "suite": suite, "package": package, "identity": identity}],
            )
            self.assertEqual(harness.rust_test_argv(package, identity), [
                "cargo", "test", "--offline", "-p", "od-jit", "--lib", identity,
                "--", "--exact", "--test-threads=1",
            ])

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, package, identity in expected:
                exact_argv = harness.rust_test_argv(package, identity)

                steps = passing_rust_gate_steps(log_dir)
                steps[step]["log"] = (temporary_dir / f"{step}-wrong.log").relative_to(
                    harness.ROOT
                ).as_posix()
                with self.subTest(step=step, mutation="wrong per-run log"), self.assertRaisesRegex(
                    harness.HarnessError, f"log path differs: {step}"
                ):
                    harness.validate_required_rust_test_evidence(steps, run_root)

                for changed in (
                    {"package": "od-host"},
                    {"test_identity": "dispatch::tests::unrelated_test"},
                    {"argv": harness.rust_test_argv("od-jit", "dispatch::tests::unrelated_test")},
                    {"argv": [*exact_argv[:-1], "--test-threads=2"]},
                    {"returncode": 1},
                    {"test_outcomes": {identity: "FAILED"}},
                ):
                    steps = passing_rust_gate_steps(log_dir)
                    steps[step].update(changed)
                    with self.subTest(step=step, changed=changed), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(steps, run_root)

                for output in (
                    "test result: ok. 0 passed; 0 failed; 0 ignored\n",
                    f"test {identity} ... ignored\n",
                    f"test {identity} ... ok\ntest {identity} ... ok\n",
                    "test dispatch::tests::unrelated_test ... ok\n",
                ):
                    steps = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(steps[step], output)
                    with self.subTest(step=step, output=output), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(steps, run_root)

                steps = passing_rust_gate_steps(log_dir)
                del steps[step]
                with self.subTest(step=step, mutation="missing"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(steps, run_root)

                gate_manifest = {
                    "gates": harness.rust_test_gate_manifest(),
                    "sources": [
                        {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                        for source in harness.LOCAL_RUST_TEST_SOURCES
                    ],
                }
                gate_index = next(
                    index for index, gate in enumerate(gate_manifest["gates"])
                    if gate["step"] == step
                )
                original_gate = gate_manifest["gates"][gate_index]
                for label, replacement in (
                    ("missing", None),
                    ("duplicate", original_gate),
                    ("wrong package", {**original_gate, "package": "od-host"}),
                    ("wrong identity", {**original_gate, "identity": "dispatch::tests::unrelated_test"}),
                ):
                    changed_manifest = json.loads(json.dumps(gate_manifest))
                    if replacement is None:
                        del changed_manifest["gates"][gate_index]
                    elif label == "duplicate":
                        changed_manifest["gates"].append(replacement)
                    else:
                        changed_manifest["gates"][gate_index] = replacement
                    with self.subTest(step=step, manifest=label), self.assertRaisesRegex(
                        harness.HarnessError, "exact required Rust test gates"
                    ):
                        harness.validate_rust_test_manifest(changed_manifest)

                bad_source_manifest = json.loads(json.dumps(gate_manifest))
                source_entry = next(
                    item for item in bad_source_manifest["sources"]
                    if item["path"] == "crates/od-jit/src/dispatch.rs"
                )
                source_entry["sha256"] = "0" * 64
                with self.subTest(step=step, manifest="wrong source pin"), self.assertRaisesRegex(
                    harness.HarnessError, "source pin differs"
                ):
                    harness.validate_rust_test_manifest(bad_source_manifest)

    def test_s3_34u_memory_load_and_pc_map_gates_are_pinned_once_and_fail_closed(self):
        expected = [
            (
                "s3_34u_ir_dword_load_admission_provenance", "local_acceptance", "od-jit",
                "memory_load_v2::tests::ir_dword_load_admits_only_exact_8b03_and_preserves_its_provenance",
            ),
            (
                "s3_34u_ir_dword_load_zero_extend_four_bytes", "local_acceptance", "od-host",
                "tests::s3_26c_ir_dword_load_zero_extends_and_reads_exactly_four_bytes",
            ),
            (
                "s3_34u_single_site_pc_map_checked_ranges", "local_acceptance", "od-jit",
                "memory_load_v2::tests::ir_single_site_pc_maps_attribute_all_checked_ranges_and_boundaries",
            ),
            (
                "s3_34u_single_site_pc_map_publication_lifetime", "local_acceptance", "od-host",
                "published_single_site_pc_maps_match_artifacts_outlive_them_and_republish",
            ),
        ]
        observed = [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34u_")]
        self.assertEqual(observed, expected)
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34u_")],
            expected,
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        for step, suite, package, identity in expected:
            self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
            self.assertEqual(
                [gate for gate in manifest if gate["step"] == step],
                [{"step": step, "suite": suite, "package": package, "identity": identity}],
            )

        self.assertEqual(harness.rust_test_argv("od-jit", expected[0][3]), [
            "cargo", "test", "--offline", "-p", "od-jit", "--lib", expected[0][3],
            "--", "--exact", "--test-threads=1",
        ])
        self.assertEqual(harness.rust_test_argv("od-host", expected[1][3]), [
            "cargo", "test", "--offline", "-p", "od-host", "--lib", expected[1][3],
            "--", "--exact", "--test-threads=1",
        ])

        self.assertEqual(harness.rust_test_argv("od-jit", expected[2][3]), [
            "cargo", "test", "--offline", "-p", "od-jit", "--lib", expected[2][3],
            "--", "--exact", "--test-threads=1",
        ])
        self.assertEqual(harness.rust_test_argv("od-host", expected[3][3]), [
            "cargo", "test", "--offline", "-p", "od-host", "--test", "memory_corpus",
            expected[3][3], "--", "--exact", "--test-threads=1",
        ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            gate for gate in missing_manifest["gates"] if gate["step"] != expected[3][0]
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(steps, run_root)
            for step, _, _, _ in expected:
                changed = passing_rust_gate_steps(log_dir)
                del changed[step]
                with self.subTest(missing_gate=step), self.assertRaisesRegex(
                    harness.HarnessError, f"required Rust test gate is missing or not passed: {step}"
                ):
                    harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_26f_store_store_gates_are_pinned_once_and_fail_closed(self):
        host_identity = "tests::ir_two_store_pair_preserves_callbacks_faults_and_continuation"
        jit_gate = (
            "s3_26f_store_store_source_sites_and_pc_map", "local_acceptance", "od-jit",
            "memory_load_v2::tests::ir_store_then_store_binds_two_exact_ordered_sites",
        )
        dispatch_gate = (
            "s3_26f_store_store_uncached_dispatch_continuation", "local_acceptance", "od-jit",
            "memory_dispatch::tests::uncached_dispatch_selects_store_store_pair_and_resumes_at_second_site",
        )
        host_gate = ("ir_two_store_pair_preserves_callbacks_faults_and_continuation", "local_lifecycle", "od-host", host_identity)
        corpus_gate = (
            "s3_26f_store_store_corpus_jit_interpreter", "local_acceptance", "od-host",
            "fixed_seed_s3_26f_store_store_corpus_matches_interpreter_twice_per_input",
        )
        testkit_gate = (
            "s3_26f_store_store_fixed_seed_generator", "local_acceptance", "od-testkit",
            "memory_corpus::tests::s3_26f_store_store_inputs_are_deterministic_unique_and_use_two_addresses",
        )
        s3_26f_gates = (jit_gate, dispatch_gate, corpus_gate, testkit_gate)
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_26f_")],
            list(s3_26f_gates),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if "store_pair_preserves" in gate[3]],
            [host_gate],
        )
        self.assertEqual(harness.LIFECYCLE_TESTS.count(host_identity.removeprefix("tests::")), 1)
        self.assertEqual(
            harness.SELF_TEST_NAMES.count(
                "test_s3_26f_store_store_gates_are_pinned_once_and_fail_closed"
            ),
            1,
        )
        self.assertIn(corpus_gate[3], harness.MEMORY_REQUIRED_TESTS)
        self.assertEqual(len(harness.MEMORY_REQUIRED_TESTS), 14)
        self.assertEqual(harness.MEMORY_CORPUS_UNIQUE_INPUTS, 4800)
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-jit/src/lowering.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-jit/src/block_emitter.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-jit/src/memory_dispatch.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertEqual(harness.LOCAL_RUST_TEST_SOURCES.count("crates/od-host/tests/memory_corpus.rs"), 1)
        self.assertEqual(harness.LOCAL_RUST_TEST_SOURCES.count("crates/od-testkit/src/memory_corpus.rs"), 1)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        all_gates = (*s3_26f_gates, host_gate)
        for step, suite, package, identity in all_gates:
            self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
            self.assertEqual(
                [gate for gate in manifest if gate["step"] == step],
                [{"step": step, "suite": suite, "package": package, "identity": identity}],
            )
        self.assertEqual(
            harness.rust_test_argv("od-host", corpus_gate[3]),
            [
                "cargo", "test", "--offline", "-p", "od-host", "--test", "memory_corpus",
                corpus_gate[3], "--", "--exact", "--test-threads=1",
            ],
        )
        self.assertEqual(
            harness.rust_test_argv("od-testkit", testkit_gate[3]),
            [
                "cargo", "test", "--offline", "-p", "od-testkit", "--lib", testkit_gate[3],
                "--", "--exact", "--test-threads=1",
            ],
        )

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for path in ("crates/od-host/tests/memory_corpus.rs", "crates/od-testkit/src/memory_corpus.rs"):
            changed = json.loads(json.dumps(source_manifest))
            changed["sources"] = [source for source in changed["sources"] if source["path"] != path]
            with self.subTest(missing_source=path), self.assertRaisesRegex(
                harness.HarnessError, "required Rust test sources"
            ):
                harness.validate_rust_test_manifest(changed)
        for step in (gate[0] for gate in all_gates):
            for label, mutate in (
                ("missing", lambda gates: [gate for gate in gates if gate["step"] != step]),
                ("duplicate", lambda gates: [*gates, next(gate for gate in gates if gate["step"] == step)]),
            ):
                changed = json.loads(json.dumps(source_manifest))
                changed["gates"] = mutate(changed["gates"])
                with self.subTest(gate=step, manifest=label), self.assertRaisesRegex(
                    harness.HarnessError, "exact required Rust test gates"
                ):
                    harness.validate_rust_test_manifest(changed)

    def test_s3_16o_store_store_not_rcx_mixed_corpus_gate_is_pinned_once_and_fail_closed(self):
        step = "s3_16o_store_store_not_rcx_store_store_corpus"
        suite = "local_acceptance"
        package = "od-host"
        identity = "tests::s3_16o_s3_34_store_store_not_rcx_store_store_fixed_seed_boundaries"
        expected = (step, suite, package, identity)
        self.assertEqual(harness.ADDITIONAL_TEST_GATES.count(expected), 1)
        self.assertEqual(harness.RUST_TEST_GATES.count(expected), 1)
        self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
        self.assertEqual(
            harness.SELF_TEST_NAMES.count(
                "test_s3_16o_store_store_not_rcx_mixed_corpus_gate_is_pinned_once_and_fail_closed"
            ),
            1,
        )
        self.assertIn(
            "__main__.Stage3HarnessValidationTests.test_s3_16o_store_store_not_rcx_mixed_corpus_gate_is_pinned_once_and_fail_closed",
            harness.SELF_TEST_IDENTITIES,
        )
        self.assertEqual(len(harness.SELF_TEST_NAMES), EXPECTED_SELF_TESTS)
        self.assertEqual(len(harness.SELF_TEST_IDENTITIES), EXPECTED_SELF_TESTS)

        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(
            [gate for gate in manifest if gate["step"] == step],
            [{"step": step, "suite": suite, "package": package, "identity": identity}],
        )
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        command = [
            "cargo", "test", "--offline", "-p", package, "--lib", identity,
            "--", "--exact", "--test-threads=1",
        ]
        self.assertEqual(harness.rust_test_argv(package, identity), command)

        source_path = "crates/od-host/src/lib.rs"
        self.assertEqual(harness.LOCAL_RUST_TEST_SOURCES.count(source_path), 1)
        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            source for source in bad_source_pin["sources"] if source["path"] == source_path
        )
        source_pin["sha256"] = "0" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        for label, mutate in (
            ("missing", lambda gates: [gate for gate in gates if gate["step"] != step]),
            ("duplicate", lambda gates: [*gates, next(gate for gate in gates if gate["step"] == step)]),
            ("identity", lambda gates: [
                {**gate, "identity": "tests::unrelated_test"} if gate["step"] == step else gate
                for gate in gates
            ]),
            ("package", lambda gates: [
                {**gate, "package": "od-jit"} if gate["step"] == step else gate
                for gate in gates
            ]),
        ):
            changed = json.loads(json.dumps(source_manifest))
            changed["gates"] = mutate(changed["gates"])
            with self.subTest(manifest=label), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(changed)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for changed in (
                {"name": "different_step"},
                {"gate_suite": "local_lifecycle"},
                {"package": "od-jit"},
                {"test_identity": "tests::unrelated_test"},
                {"argv": ["cargo", "test"]},
                {"returncode": 1},
            ):
                steps = passing_rust_gate_steps(log_dir)
                steps[step].update(changed)
                with self.subTest(evidence=changed), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(steps, run_root)
            missing = passing_rust_gate_steps(log_dir)
            del missing[step]
            with self.assertRaisesRegex(harness.HarnessError, step):
                harness.validate_required_rust_test_evidence(missing, run_root)

    def test_s3_34v_dword_store_gates_are_pinned_once_and_fail_closed(self):
        expected = [
            (
                "s3_34v_ir_dword_store_admission_provenance", "local_acceptance", "od-jit",
                "memory_load_v2::tests::ir_dword_store_admits_only_exact_8903_and_preserves_its_provenance",
            ),
            (
                "s3_34v_ir_dword_store_commits_one_four_byte_write", "local_acceptance", "od-host",
                "tests::s3_26e_ir_dword_store_matches_interpreter_and_commits_one_four_byte_write",
            ),
        ]
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34v_")],
            expected,
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34v_")],
            expected,
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)
        for step, suite, package, identity in expected:
            self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
            self.assertEqual(
                [gate for gate in manifest if gate["step"] == step],
                [{"step": step, "suite": suite, "package": package, "identity": identity}],
            )
            self.assertEqual(harness.rust_test_argv(package, identity), [
                "cargo", "test", "--offline", "-p", package, "--lib", identity,
                "--", "--exact", "--test-threads=1",
            ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            for label, mutate in (
                ("missing", lambda gates, target: gates.__delitem__(target)),
                ("duplicate", lambda gates, target: gates.append(gates[target])),
            ):
                changed = json.loads(json.dumps(source_manifest))
                target = next(
                    index for index, gate in enumerate(changed["gates"])
                    if gate["step"] == step
                )
                mutate(changed["gates"], target)
                with self.subTest(step=step, manifest=label), self.assertRaisesRegex(
                    harness.HarnessError, "exact required Rust test gates"
                ):
                    harness.validate_rust_test_manifest(changed)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError, f"required Rust test gate is missing or not passed: {step}"
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                failed = passing_rust_gate_steps(log_dir)
                replace_step_log_output(failed[step], f"test {identity} ... FAILED\n")
                failed[step].update({"status": "failed", "returncode": 101, "failure": "test failed"})
                with self.subTest(step=step, evidence="failed"), self.assertRaisesRegex(
                    harness.HarnessError, f"required Rust test gate is missing or not passed: {step}"
                ):
                    harness.validate_required_rust_test_evidence(failed, run_root)

    def test_s3_34w_dword_pc_map_gates_are_pinned_once_and_fail_closed(self):
        expected = [
            (
                "s3_34w_ir_dword_load_pc_map_exact_8b03", "local_acceptance", "od-jit",
                "memory_load_v2::tests::ir_dword_load_pc_map_has_exact_8b03_single_site_contract",
            ),
            (
                "s3_34w_ir_dword_store_pc_map_exact_8903", "local_acceptance", "od-jit",
                "memory_load_v2::tests::ir_dword_store_pc_map_has_exact_8903_single_site_contract",
            ),
            (
                "s3_34w_published_dword_pc_maps_lifetime", "local_acceptance", "od-host",
                "published_dword_pc_maps_match_artifacts_survive_drop_and_republish_at_new_rips",
            ),
        ]
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34w_")],
            expected,
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34w_")],
            expected,
        )
        manifest = harness.rust_test_gate_manifest()
        for step, suite, package, identity in expected:
            self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
            self.assertEqual(
                [gate for gate in manifest if gate["step"] == step],
                [{"step": step, "suite": suite, "package": package, "identity": identity}],
            )

        self.assertEqual(harness.rust_test_argv("od-jit", expected[0][3]), [
            "cargo", "test", "--offline", "-p", "od-jit", "--lib", expected[0][3],
            "--", "--exact", "--test-threads=1",
        ])
        self.assertEqual(harness.rust_test_argv("od-jit", expected[1][3]), [
            "cargo", "test", "--offline", "-p", "od-jit", "--lib", expected[1][3],
            "--", "--exact", "--test-threads=1",
        ])
        self.assertEqual(harness.rust_test_argv("od-host", expected[2][3]), [
            "cargo", "test", "--offline", "-p", "od-host", "--test", "memory_corpus",
            expected[2][3], "--", "--exact", "--test-threads=1",
        ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            gate for gate in missing_manifest["gates"] if gate["step"] != expected[0][0]
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, package, identity in expected:
                with self.subTest(step=step, evidence="missing"):
                    missing = passing_rust_gate_steps(log_dir)
                    del missing[step]
                    with self.assertRaisesRegex(
                        harness.HarnessError,
                        f"required Rust test gate is missing or not passed: {step}",
                    ):
                        harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_cargo_target = (
                    ["--test", "memory_corpus"] if package == "od-jit" else ["--lib"]
                )
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, *wrong_cargo_target, identity,
                    "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError, f"required Rust test gate invocation is not exact: {step}"
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34x_dword_alias_load_gates_are_pinned_once_and_fail_closed(self):
        expected = [
            (
                "s3_34x_ir_dword_alias_load_admission_provenance", "local_acceptance", "od-jit",
                "memory_load_v2::tests::ir_dword_alias_load_admits_only_exact_8b00_and_preserves_its_provenance",
            ),
            (
                "s3_34x_ir_dword_alias_load_pc_map_exact_8b00", "local_acceptance", "od-jit",
                "memory_load_v2::tests::ir_dword_alias_load_pc_map_has_exact_8b00_single_site_contract",
            ),
            (
                "s3_34x_host_dword_alias_load_old_full_rax_atomic_eax", "local_acceptance", "od-host",
                "tests::s3_dword_alias_load_reads_old_full_rax_and_commits_eax_atomically",
            ),
        ]
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34x_")],
            expected,
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34x_")],
            expected,
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for step, suite, package, identity in expected:
            self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
            self.assertEqual(
                [gate for gate in manifest if gate["step"] == step],
                [{"step": step, "suite": suite, "package": package, "identity": identity}],
            )
            self.assertEqual(harness.rust_test_argv(package, identity), [
                "cargo", "test", "--offline", "-p", package, "--lib", identity,
                "--", "--exact", "--test-threads=1",
            ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest = json.loads(json.dumps(source_manifest))
            missing_manifest["gates"] = [
                gate for gate in missing_manifest["gates"] if gate["step"] != step
            ]
            with self.subTest(step=step, manifest="missing"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError, f"required Rust test gate is missing or not passed: {step}"
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "memory_corpus",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError, f"required Rust test gate invocation is not exact: {step}"
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34y_dword_alias_store_gates_are_pinned_once_and_fail_closed(self):
        expected = [
            (
                "s3_34y_ir_dword_alias_store_admission_provenance", "local_acceptance", "od-jit",
                "memory_load_v2::tests::ir_dword_alias_store_admits_only_exact_8900_and_preserves_its_provenance",
            ),
            (
                "s3_34y_ir_dword_alias_store_pc_map_exact_8900", "local_acceptance", "od-jit",
                "memory_load_v2::tests::ir_dword_alias_store_pc_map_has_exact_8900_single_site_contract",
            ),
            (
                "s3_34y_host_dword_alias_store_old_rax_address_and_payload", "local_acceptance", "od-host",
                "tests::s3_20cq_ir_dword_alias_store_uses_old_rax_for_address_and_payload",
            ),
        ]
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34y_")],
            expected,
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34y_")],
            expected,
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for step, suite, package, identity in expected:
            self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
            self.assertEqual(
                [gate for gate in manifest if gate["step"] == step],
                [{"step": step, "suite": suite, "package": package, "identity": identity}],
            )
            self.assertEqual(harness.rust_test_argv(package, identity), [
                "cargo", "test", "--offline", "-p", package, "--lib", identity,
                "--", "--exact", "--test-threads=1",
            ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest = json.loads(json.dumps(source_manifest))
            missing_manifest["gates"] = [
                gate for gate in missing_manifest["gates"] if gate["step"] != step
            ]
            with self.subTest(step=step, manifest="missing"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError, f"required Rust test gate is missing or not passed: {step}"
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "memory_corpus",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError, f"required Rust test gate invocation is not exact: {step}"
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34z_rbx_disp8_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = [
            (
                "s3_34z_ir_dword_rbx_disp8_load_site_binding", "local_acceptance", "od-jit",
                "memory_load_v2::tests::ir_dword_rbx_disp8_load_binds_all_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34z_host_disp8_dword_load_zero_extend_four_bytes", "local_acceptance", "od-host",
                "tests::s3_20cr_disp8_dword_load_zero_extends_and_reads_exactly_four_bytes",
            ),
        ]
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34z_")],
            expected,
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34z_")],
            expected,
        )
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.rust_test_gate_manifest()), EXPECTED_RUST_GATES)
        self.assertEqual(
            len({gate["step"] for gate in harness.rust_test_gate_manifest()}), EXPECTED_RUST_GATES
        )
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for step, suite, package, identity in expected:
            self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
            self.assertEqual(
                [gate for gate in harness.rust_test_gate_manifest() if gate["step"] == step],
                [{"step": step, "suite": suite, "package": package, "identity": identity}],
            )
            self.assertEqual(harness.rust_test_argv(package, identity), [
                "cargo", "test", "--offline", "-p", package, "--lib", identity,
                "--", "--exact", "--test-threads=1",
            ])

        source_manifest = {
            "gates": harness.rust_test_gate_manifest(),
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest = json.loads(json.dumps(source_manifest))
            missing_manifest["gates"] = [
                gate for gate in missing_manifest["gates"] if gate["step"] != step
            ]
            with self.subTest(step=step, manifest="missing"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest)
        bad_source_pin = json.loads(json.dumps(source_manifest))
        host_source = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )
        host_source["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError, f"required Rust test gate is missing or not passed: {step}"
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "memory_corpus",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError, f"required Rust test gate invocation is not exact: {step}"
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34aa_rbx_disp32_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = [
            (
                "s3_34aa_ir_dword_rbx_disp32_load_source_binding", "local_acceptance", "od-jit",
                "memory_load_v2::tests::ir_dword_rbx_disp32_load_binds_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34aa_ir_dword_rbx_disp32_load_pc_map_six_byte_boundary", "local_acceptance", "od-jit",
                "memory_load_v2::tests::ir_dword_rbx_disp32_load_pc_map_uses_six_byte_guest_boundary",
            ),
            (
                "s3_34aa_host_dword_rbx_disp32_zero_extend_four_bytes", "local_acceptance", "od-host",
                "tests::s3_20cs_disp32_dword_load_zero_extends_and_reads_exactly_four_bytes",
            ),
        ]
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34aa_")],
            expected,
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34aa_")],
            expected,
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        disp8_host = (
            "s3_34z_host_disp8_dword_load_zero_extend_four_bytes", "local_acceptance", "od-host",
            "tests::s3_20cr_disp8_dword_load_zero_extends_and_reads_exactly_four_bytes",
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0] == disp8_host[0]],
            [disp8_host],
        )

        for step, suite, package, identity in expected:
            self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
            self.assertEqual(
                [gate for gate in manifest if gate["step"] == step],
                [{"step": step, "suite": suite, "package": package, "identity": identity}],
            )
            self.assertEqual(harness.rust_test_argv(package, identity), [
                "cargo", "test", "--offline", "-p", package, "--lib", identity,
                "--", "--exact", "--test-threads=1",
            ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest = json.loads(json.dumps(source_manifest))
            missing_manifest["gates"] = [
                gate for gate in missing_manifest["gates"] if gate["step"] != step
            ]
            with self.subTest(step=step, manifest="missing"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        host_source = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )
        host_source["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError, f"required Rust test gate is missing or not passed: {step}"
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "memory_corpus",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError, f"required Rust test gate invocation is not exact: {step}"
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34ab_rbx_disp8_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = [
            (
                "s3_34ab_ir_dword_rbx_disp8_store_source_binding", "local_acceptance", "od-jit",
                "memory_load_v2::tests::ir_dword_rbx_disp8_store_binds_all_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34ab_ir_dword_rbx_disp8_store_pc_map_three_byte_boundary", "local_acceptance", "od-jit",
                "memory_load_v2::tests::ir_dword_rbx_disp8_store_pc_map_uses_three_byte_guest_boundary",
            ),
            (
                "s3_34ab_host_dword_rbx_disp8_store_checked_semantics", "local_acceptance", "od-host",
                "tests::s3_20ct_disp8_dword_store_preserves_checked_semantics",
            ),
        ]
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34ab_")],
            expected,
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34ab_")],
            expected,
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for step, suite, package, identity in expected:
            self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
            self.assertEqual(
                [gate for gate in manifest if gate["step"] == step],
                [{"step": step, "suite": suite, "package": package, "identity": identity}],
            )
            self.assertEqual(harness.rust_test_argv(package, identity), [
                "cargo", "test", "--offline", "-p", package, "--lib", identity,
                "--", "--exact", "--test-threads=1",
            ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest = json.loads(json.dumps(source_manifest))
            missing_manifest["gates"] = [
                gate for gate in missing_manifest["gates"] if gate["step"] != step
            ]
            with self.subTest(step=step, manifest="missing"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        host_source = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )
        host_source["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError, f"required Rust test gate is missing or not passed: {step}"
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "memory_corpus",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError, f"required Rust test gate invocation is not exact: {step}"
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34ac_published_dword_disp8_disp32_load_maps_gate_is_pinned_and_fail_closed(self):
        expected = (
            "s3_34ac_published_dword_disp_load_maps_lifetime", "local_acceptance", "od-host",
            "published_dword_disp8_and_disp32_load_pc_maps_match_artifacts_survive_drop_and_republish",
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34ac_")],
            [expected],
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34ac_")],
            [expected],
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertIn("crates/od-host/tests/memory_corpus.rs", harness.LOCAL_RUST_TEST_SOURCES)

        step, suite, package, identity = expected
        self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
        self.assertEqual(
            [gate for gate in manifest if gate["step"] == step],
            [{"step": step, "suite": suite, "package": package, "identity": identity}],
        )
        self.assertEqual(harness.rust_test_argv(package, identity), [
            "cargo", "test", "--offline", "-p", "od-host", "--test", "memory_corpus",
            identity, "--", "--exact", "--test-threads=1",
        ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            gate for gate in missing_manifest["gates"] if gate["step"] != step
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        pinned_gate = next(
            gate for gate in wrong_identity_manifest["gates"] if gate["step"] == step
        )
        pinned_gate["identity"] = "tests::unrelated_publication_test"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            entry for entry in bad_source_pin["sources"]
            if entry["path"] == "crates/od-host/tests/memory_corpus.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            missing = passing_rust_gate_steps(log_dir)
            del missing[step]
            with self.assertRaisesRegex(
                harness.HarnessError, f"required Rust test gate is missing or not passed: {step}"
            ):
                harness.validate_required_rust_test_evidence(missing, run_root)

            wrong_target = passing_rust_gate_steps(log_dir)
            wrong_target[step]["argv"] = [
                "cargo", "test", "--offline", "-p", "od-host", "--lib", identity,
                "--", "--exact", "--test-threads=1",
            ]
            with self.assertRaisesRegex(
                harness.HarnessError, f"required Rust test gate invocation is not exact: {step}"
            ):
                harness.validate_required_rust_test_evidence(wrong_target, run_root)

            for label, output in (
                ("ignored", f"test {identity} ... ignored\n"),
                ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                ("failed", f"test {identity} ... FAILED\n"),
            ):
                changed = passing_rust_gate_steps(log_dir)
                replace_step_log_output(changed[step], output)
                if label == "failed":
                    changed[step].update(
                        {"status": "failed", "returncode": 101, "failure": "test failed"}
                    )
                with self.subTest(evidence=label), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34ad_rbx_disp32_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = [
            (
                "s3_34ad_ir_dword_rbx_disp32_store_source_binding", "local_acceptance", "od-jit",
                "memory_load_v2::tests::ir_dword_rbx_disp32_store_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34ad_ir_dword_rbx_disp32_store_pc_map_six_byte_boundary",
                "local_acceptance", "od-jit",
                "memory_load_v2::tests::ir_dword_rbx_disp32_store_pc_map_uses_six_byte_guest_boundary",
            ),
            (
                "s3_34ad_host_dword_rbx_disp32_store_checked_semantics", "local_acceptance", "od-host",
                "tests::s3_20cu_disp32_dword_store_preserves_checked_semantics",
            ),
        ]
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34ad_")],
            expected,
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34ad_")],
            expected,
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for step, suite, package, identity in expected:
            self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
            self.assertEqual(
                [gate for gate in manifest if gate["step"] == step],
                [{"step": step, "suite": suite, "package": package, "identity": identity}],
            )
            self.assertEqual(harness.rust_test_argv(package, identity), [
                "cargo", "test", "--offline", "-p", package, "--lib", identity,
                "--", "--exact", "--test-threads=1",
            ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest = json.loads(json.dumps(source_manifest))
            missing_manifest["gates"] = [
                gate for gate in missing_manifest["gates"] if gate["step"] != step
            ]
            with self.subTest(step=step, manifest="missing"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_dword_store_test"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        jit_source = next(
            entry for entry in bad_source_pin["sources"]
            if entry["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        jit_source["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError, f"required Rust test gate is missing or not passed: {step}"
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34ae_disp32_dword_store_boundary_gate_is_pinned_and_fail_closed(self):
        expected = (
            "s3_34ae_host_disp32_dword_store_canonical_end_range_overflow", "local_acceptance",
            "od-host", "tests::s3_27i_disp32_dword_store_checks_canonical_end_and_range_overflow",
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34ae_")],
            [expected],
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34ae_")],
            [expected],
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        step, suite, package, identity = expected
        self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
        self.assertEqual(
            [gate for gate in manifest if gate["step"] == step],
            [{"step": step, "suite": suite, "package": package, "identity": identity}],
        )
        exact_argv = [
            "cargo", "test", "-p", "od-host", "--lib", identity,
            "--offline", "--", "--exact",
        ]
        self.assertEqual(harness.rust_test_argv(package, identity), exact_argv)

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            gate for gate in missing_manifest["gates"] if gate["step"] != step
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_boundary_test"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        host_source = next(
            entry for entry in bad_source_pin["sources"]
            if entry["path"] == "crates/od-host/src/lib.rs"
        )
        host_source["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            missing = passing_rust_gate_steps(log_dir)
            del missing[step]
            with self.assertRaisesRegex(
                harness.HarnessError, f"required Rust test gate is missing or not passed: {step}"
            ):
                harness.validate_required_rust_test_evidence(missing, run_root)

            wrong_target = passing_rust_gate_steps(log_dir)
            wrong_target[step]["argv"] = [
                "cargo", "test", "-p", package, "--test", "memory_corpus", identity,
                "--offline", "--", "--exact",
            ]
            with self.assertRaisesRegex(
                harness.HarnessError, f"required Rust test gate invocation is not exact: {step}"
            ):
                harness.validate_required_rust_test_evidence(wrong_target, run_root)

            for label, output in (
                ("ignored", f"test {identity} ... ignored\n"),
                ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                ("failed", f"test {identity} ... FAILED\n"),
            ):
                changed = passing_rust_gate_steps(log_dir)
                replace_step_log_output(changed[step], output)
                if label == "failed":
                    changed[step].update(
                        {"status": "failed", "returncode": 101, "failure": "test failed"}
                    )
                with self.subTest(evidence=label), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34af_dword_store_publication_gate_is_pinned_and_fail_closed(self):
        expected = (
            "s3_34af_published_dword_disp8_disp32_store_maps_lifetime", "local_acceptance",
            "od-host",
            "published_dword_disp8_and_disp32_store_pc_maps_match_artifacts_survive_drop_and_republish",
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34af_")],
            [expected],
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34af_")],
            [expected],
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertIn("crates/od-host/tests/memory_corpus.rs", harness.LOCAL_RUST_TEST_SOURCES)

        step, suite, package, identity = expected
        self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
        self.assertEqual(
            [gate for gate in manifest if gate["step"] == step],
            [{"step": step, "suite": suite, "package": package, "identity": identity}],
        )
        self.assertEqual(harness.rust_test_argv(package, identity), [
            "cargo", "test", "--offline", "-p", "od-host", "--test", "memory_corpus",
            identity, "--", "--exact", "--test-threads=1",
        ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            gate for gate in missing_manifest["gates"] if gate["step"] != step
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "unrelated_publication_test"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            entry for entry in bad_source_pin["sources"]
            if entry["path"] == "crates/od-host/tests/memory_corpus.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            missing = passing_rust_gate_steps(log_dir)
            del missing[step]
            with self.assertRaisesRegex(
                harness.HarnessError, f"required Rust test gate is missing or not passed: {step}"
            ):
                harness.validate_required_rust_test_evidence(missing, run_root)

            wrong_target = passing_rust_gate_steps(log_dir)
            wrong_target[step]["argv"] = [
                "cargo", "test", "--offline", "-p", "od-host", "--lib", identity,
                "--", "--exact", "--test-threads=1",
            ]
            with self.assertRaisesRegex(
                harness.HarnessError, f"required Rust test gate invocation is not exact: {step}"
            ):
                harness.validate_required_rust_test_evidence(wrong_target, run_root)

            for label, output in (
                ("ignored", f"test {identity} ... ignored\n"),
                ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                ("failed", f"test {identity} ... FAILED\n"),
            ):
                changed = passing_rust_gate_steps(log_dir)
                replace_step_log_output(changed[step], output)
                if label == "failed":
                    changed[step].update(
                        {"status": "failed", "returncode": 101, "failure": "test failed"}
                    )
                with self.subTest(evidence=label), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34ag_rip_relative_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34ag_ir_dword_rip_relative_disp32_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rip_relative_disp32_load_binds_all_source_bytes_to_next_rip",
            ),
            (
                "s3_34ag_ir_dword_rip_relative_disp32_load_pc_map_six_byte_boundary", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rip_relative_disp32_load_pc_map_uses_six_byte_guest_boundary",
            ),
            (
                "s3_34ag_host_rip_relative_disp32_dword_load_guest_next_rip", "local_acceptance",
                "od-host", "tests::s3_20cv_rip_relative_disp32_dword_load_uses_guest_next_rip",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34ag_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34ag_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for gate in expected:
            step, suite, package, identity = gate
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [item for item in manifest if item["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            item for item in missing_manifest["gates"]
            if not item["step"].startswith("s3_34ag_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_rip_load"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34ah_rip_relative_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34ah_ir_dword_rip_relative_disp32_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rip_relative_disp32_store_binds_all_source_bytes_to_next_rip",
            ),
            (
                "s3_34ah_ir_dword_rip_relative_disp32_store_pc_map_six_byte_boundary", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rip_relative_disp32_store_pc_map_uses_six_byte_guest_boundary",
            ),
            (
                "s3_34ah_host_rip_relative_disp32_dword_store_guest_next_rip", "local_acceptance",
                "od-host", "tests::s3_20cw_rip_relative_disp32_dword_store_uses_guest_next_rip",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34ah_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34ah_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for gate in expected:
            step, suite, package, identity = gate
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [item for item in manifest if item["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            item for item in missing_manifest["gates"]
            if not item["step"].startswith("s3_34ah_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_rip_store"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34ai_addr32_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34ai_ir_dword_addr32_load_exact_admission_provenance", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_load_admits_only_67_8b_03_and_preserves_its_provenance",
            ),
            (
                "s3_34ai_ir_dword_addr32_load_pc_map_three_byte_boundary", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_load_pc_map_uses_three_byte_guest_boundary",
            ),
            (
                "s3_34ai_host_addr32_dword_load_zero_extended_ebx", "local_acceptance",
                "od-host", "tests::s3_20cx_addr32_dword_load_uses_zero_extended_ebx",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34ai_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34ai_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for gate in expected:
            step, suite, package, identity = gate
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [item for item in manifest if item["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            item for item in missing_manifest["gates"]
            if not item["step"].startswith("s3_34ai_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_addr32_load"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34aj_addr32_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34aj_ir_dword_addr32_store_exact_admission_provenance", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_store_admits_only_67_89_03_and_preserves_its_provenance",
            ),
            (
                "s3_34aj_host_addr32_dword_store_zero_extended_ebx", "local_acceptance",
                "od-host", "tests::s3_20cy_addr32_dword_store_uses_zero_extended_ebx",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34aj_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34aj_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for gate in expected:
            step, suite, package, identity = gate
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [item for item in manifest if item["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            item for item in missing_manifest["gates"]
            if not item["step"].startswith("s3_34aj_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_addr32_store"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34ak_addr32_disp8_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34ak_ir_dword_addr32_disp8_load_source_binding", "local_acceptance", "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_disp8_load_binds_all_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34ak_host_addr32_disp8_dword_load_wrapped_ebx", "local_acceptance", "od-host",
                "tests::s3_20cz_addr32_disp8_dword_load_uses_wrapped_ebx",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34ak_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34ak_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for gate in expected:
            step, suite, package, identity = gate
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [item for item in manifest if item["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            item for item in missing_manifest["gates"]
            if not item["step"].startswith("s3_34ak_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_addr32_disp8_load"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34al_addr32_disp32_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34al_ir_dword_addr32_disp32_load_source_binding", "local_acceptance", "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_disp32_load_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34al_host_addr32_disp32_dword_load_wrapped_ebx", "local_acceptance", "od-host",
                "tests::s3_20da_addr32_disp32_dword_load_uses_wrapped_ebx",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34al_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34al_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for gate in expected:
            step, suite, package, identity = gate
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [item for item in manifest if item["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            item for item in missing_manifest["gates"]
            if not item["step"].startswith("s3_34al_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_addr32_disp32_load"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34o_reducer_artifact_inventory_rejects_tampering_and_symlinks(self):
        with temporary_fixture_directory() as temporary:
            run_dir = Path(temporary)
            reducer_dir = (
                run_dir
                / "artifacts"
                / "native-register-v1"
                / "native-register-v1-reducer"
            )
            required_files = [
                "reduction-report.json",
                "source-saved-case.json",
                "local-fault-injection-selfcheck/reduction-report.json",
                "local-fault-injection-selfcheck/source-saved-case.json",
                "local-fault-injection-selfcheck/reduced-saved-case.json",
                "local-fault-injection-selfcheck/reduced-input-digest.txt",
                "local-fault-injection-selfcheck/limit/reduction-report.json",
                "local-fault-injection-selfcheck/limit/reduced-saved-case.json",
                "local-fault-injection-selfcheck/limit/reduced-input-digest.txt",
            ]
            for relative in required_files:
                path = reducer_dir / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("captured reducer artifact\n")
            artifact_relative = (
                run_dir / "artifacts" / "native-register-v1"
            ).relative_to(harness.ROOT).as_posix()
            evidence = harness.native_register_v1_reducer_artifact_inventory(run_dir)
            names = (
                "s3_34o_native_register_v1_reducer",
                "s3_34o_native_register_v1_reducer_selfcheck",
            )
            steps = {
                name: {
                    "status": "passed",
                    "artifact_directory": artifact_relative,
                    "reducer_artifacts": evidence,
                }
                for name in names
            }
            harness.validate_native_register_v1_reducer_inventory(steps, run_dir)

            report = reducer_dir / "reduction-report.json"
            report.write_text("tampered backend observations\n")
            with self.assertRaisesRegex(harness.HarnessError, "digest/path binding"):
                harness.validate_native_register_v1_reducer_inventory(steps, run_dir)

            report.write_text("captured reducer artifact\n")
            steps[names[0]]["reducer_artifacts"][0]["path"] = "wrong/path"
            with self.assertRaisesRegex(harness.HarnessError, "digest/path binding"):
                harness.validate_native_register_v1_reducer_inventory(steps, run_dir)

            steps[names[0]]["reducer_artifacts"] = evidence
            missing = reducer_dir / required_files[4]
            missing.unlink()
            with self.assertRaisesRegex(harness.HarnessError, "inventory is incomplete"):
                harness.native_register_v1_reducer_artifact_inventory(run_dir)

            outside = run_dir / "outside-artifact.txt"
            outside.write_text("outside\n")
            missing.symlink_to(outside)
            with self.assertRaisesRegex(harness.HarnessError, "cannot be a symlink"):
                harness.native_register_v1_reducer_artifact_inventory(run_dir)


    def test_rust_gate_summary_rejects_an_omitted_mandatory_gate(self):
        with temporary_fixture_directory() as temporary:
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
                "s3_33n_native_register_v1_admission",
                "s3_33n_native_register_v1_compiler",
                "s3_33n_native_register_v1_host",
                "s3_34n_native_register_v1_corpus",
                "s3_34n_native_register_v1_replay",
                "s3_34n_native_register_v1_selfcheck",
                "s3_34o_native_register_v1_reducer",
                "s3_34o_native_register_v1_reducer_selfcheck",
                "s3_34r_abi_idle_handle_cross_thread_mapping_release",
                "s3_34bf_ir_dword_rbp_disp32_store_source_binding",
                "s3_38e_session_same_code_reuses_publication",
                "s3_38e_session_clear_releases_publication",
                "s3_38e_session_rejects_second_mapping_owner",
                "s3_38e_session_invalid_entry_no_automatic_retry",
            )
            for missing_step in (harness.ADDITIONAL_TEST_GATES[0][0], *mandatory_steps):
                steps = passing_rust_gate_steps(Path(temporary))
                del steps[missing_step]
                with self.subTest(missing_step=missing_step):
                    with self.assertRaisesRegex(harness.HarnessError, missing_step):
                        harness.validate_required_rust_test_evidence(steps)

    def test_rust_gate_summary_rejects_forged_passed_evidence(self):
        with temporary_fixture_directory() as temporary:
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
        with temporary_fixture_directory() as temporary:
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
        with temporary_fixture_directory() as external:
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
        with temporary_fixture_directory() as external:
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
                with mock.patch.object(harness, "validate_local_vmhost_result_state"):
                    with self.assertRaisesRegex(harness.HarnessError, "log path differs"):
                        harness.validate_final_summary(summary, manifest_path)

                expected_log.unlink()
                expected_log.write_text(output)
                summary["steps"][step_name]["log"] = external_log.relative_to(
                    harness.ROOT
                ).as_posix()
                with mock.patch.object(harness, "validate_local_vmhost_result_state"):
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
        with temporary_fixture_directory() as temporary_fixture:
            fixture_path = Path(temporary_fixture)
            self.assertEqual(
                fixture_path.parent,
                harness.ROOT / "work" / "stage3-harness" / "test-fixtures",
            )

        child = (
            "import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); "
            "print('child-before-cleanup', flush=True); time.sleep(30)"
        )
        parent = (
            "import subprocess,sys,time; print('parent-before-cleanup', flush=True); "
            "subprocess.Popen([sys.executable, '-c', sys.argv[1]]); "
            "time.sleep(30)"
        )
        for run_process in (False, True):
            for fingerprint_error in (False, True):
                with self.subTest(run_process=run_process, fingerprint_error=fingerprint_error):
                    temporary, run_root = temporary_run_root()
                    try:
                        command = (
                            [sys.executable, "-c", parent, child]
                            if run_process
                            else [sys.executable, "-c", "import time; time.sleep(30)"]
                        )
                        runner = harness.run_process_step if run_process else harness.run_step
                        fingerprint_patch = (
                            mock.patch.object(
                                harness,
                                "source_fingerprint",
                                side_effect=harness.HarnessError(
                                    "synthetic fingerprint read failure"
                                ),
                            )
                            if fingerprint_error
                            else mock.patch.object(
                                harness, "source_fingerprint", return_value="b" * 64
                            )
                        )
                        with fingerprint_patch:
                            step = runner(
                                "timeout_fingerprint_precedence",
                                command,
                                run_root,
                                0.2,
                                "a" * 64,
                            )
                        self.assertEqual(step["failure"], "timeout")
                        self.assertEqual(step["status"], "failed")
                        if fingerprint_error:
                            self.assertIsNone(step["source_fingerprint_after"])
                        else:
                            self.assertEqual(step["source_fingerprint_after"], "b" * 64)
                        if run_process:
                            stdout_evidence = step["timeout_diagnostics"]["stdout"]
                            diagnostic_path = harness.ROOT / stdout_evidence["path"]
                            self.assertIn("parent-before-cleanup", diagnostic_path.read_text())
                            self.assertIn("child-before-cleanup", diagnostic_path.read_text())
                            self.assertFalse(
                                harness.process_group_exists(step["process_group_id"])
                            )
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
            v1_smoke = commands["process_control_v1_sessions"]
            self.assertEqual(
                v1_smoke,
                [
                    sys.executable,
                    "tests/system/vmhost_control_v1_same_code_smoke.py",
                    str(harness.vmhost_process_paths(run_root)["vmhost"]),
                ],
            )
            self.assertIn(
                "tests/system/vmhost_control_v1_same_code_smoke.py",
                harness.PROCESS_SOURCE_PATHS,
            )
            vmhost_path = str(harness.vmhost_process_paths(run_root)["vmhost"])
            code_mismatch_path = "tests/system/vmhost_control_v1_code_mismatch_smoke.py"
            code_mismatch_step = "process_control_v1_code_mismatch"
            code_mismatch_marker = (
                "VMHost control V1 smoke passed: immutable-code mismatch rejection "
                "preserves binding and RUN ID high-watermark"
            )
            self.assertEqual(
                commands[code_mismatch_step],
                [sys.executable, code_mismatch_path, vmhost_path],
            )
            self.assertIn(code_mismatch_path, harness.PROCESS_SOURCE_PATHS)
            self.assertEqual(harness.PROCESS_STEP_TIMEOUTS[code_mismatch_step], 30)
            self.assertEqual(
                harness.PROCESS_SUCCESS_MARKERS["control_v1_code_mismatch"],
                code_mismatch_marker,
            )
            code_mismatch_manifest = next(
                entry for entry in manifest_commands if entry["step"] == code_mismatch_step
            )
            self.assertEqual(
                code_mismatch_manifest["argv"],
                [sys.executable, code_mismatch_path, "${RUN_ROOT}/artifacts/od-vmhost-smoke"],
            )
            manifest_inputs = harness.input_manifest()["vmhost_process"]["inputs"]
            mismatch_source = {
                "path": code_mismatch_path,
                "sha256": harness.sha256_file(harness.ROOT / code_mismatch_path),
            }
            self.assertIn(
                mismatch_source,
                manifest_inputs,
            )
            for step_name, script_path, marker_key, marker in (
                (
                    "process_control_v1_frames",
                    "tests/system/vmhost_control_v1_frames_smoke.py",
                    "control_v1_frames",
                    "VMHost control V1 smoke passed: fragmented/coalesced frames, bounded EOF teardown, oversized length",
                ),
                (
                    "process_control_v1_stop_eof",
                    "tests/system/vmhost_control_v1_stop_eof_smoke.py",
                    "control_v1_stop_eof",
                    "VMHost control V1 smoke passed: STOP isolation and bounded active-run EOF teardown",
                ),
            ):
                with self.subTest(step=step_name):
                    self.assertEqual(commands[step_name], [sys.executable, script_path, vmhost_path])
                    self.assertIn(script_path, harness.PROCESS_SOURCE_PATHS)
                    self.assertEqual(harness.PROCESS_STEP_TIMEOUTS[step_name], 30)
                    self.assertEqual(harness.PROCESS_SUCCESS_MARKERS[marker_key], marker)
                    command = next(
                        entry for entry in manifest_commands if entry["step"] == step_name
                    )
                    self.assertEqual(
                        command["argv"], [sys.executable, script_path, "${RUN_ROOT}/artifacts/od-vmhost-smoke"]
                    )
            self.assertIn("apps/vmhost/Sources/ODHostABI/include/VMHostABI.h", harness.PROCESS_SOURCE_PATHS)
            self.assertIn("include/open_desktop/session_abi_v1.h", harness.PROCESS_SOURCE_PATHS)
            self.assertIn("apps/macos/Sources/Shared/ControlFrame.swift", harness.PROCESS_SOURCE_PATHS)
            self.assertIn(
                "apps/vmhost/Sources/ODHostABI/include/VMHostABI.h",
                commands["process_compile_vmhost"],
            )
            self.assertEqual(
                harness.input_manifest()["vmhost_process"]["success_markers"],
                harness.PROCESS_SUCCESS_MARKERS,
            )
            self.assertEqual(
                harness.PROCESS_SUCCESS_MARKERS["control_v1_sessions"],
                "VMHost control V1 smoke passed: same-code sequential RUNs with fresh state and budgets",
            )
        finally:
            temporary.cleanup()

    def test_vmhost_process_run_stop_id_gate_is_pinned_once_and_fails_closed(self):
        step_name = "process_control_v1_run_stop_id"
        script_path = "tests/system/vmhost_control_v1_run_stop_id_smoke.py"
        marker = "VMHost control V1 smoke passed: active STOP and RUN-ID high-watermark"
        self_test_name = "test_vmhost_process_run_stop_id_gate_is_pinned_once_and_fails_closed"
        self.assertEqual(harness.SELF_TEST_NAMES.count(self_test_name), 1)
        self.assertIn(
            f"__main__.Stage3HarnessValidationTests.{self_test_name}",
            harness.SELF_TEST_IDENTITIES,
        )
        temporary, run_root = temporary_run_root()
        try:
            commands = harness.vmhost_process_commands(run_root)
            vmhost_path = str(harness.vmhost_process_paths(run_root)["vmhost"])
            self.assertEqual(harness.PROCESS_SUITE_STEPS.count(step_name), 1)
            self.assertEqual(
                commands[step_name], [sys.executable, script_path, vmhost_path]
            )
            self.assertEqual(harness.PROCESS_STEP_TIMEOUTS[step_name], 30)
            self.assertEqual(harness.PROCESS_SUCCESS_MARKERS["control_v1_run_stop_id"], marker)
            self.assertEqual(
                harness.PROCESS_SOURCE_PATHS.count(script_path), 1
            )
            manifest = harness.input_manifest()["vmhost_process"]
            source_record = {
                "path": script_path,
                "sha256": harness.sha256_file(harness.ROOT / script_path),
            }
            self.assertIn(source_record, manifest["inputs"])
            command = next(item for item in manifest["build_profile"]["commands"]
                           if item["step"] == step_name)
            self.assertEqual(
                command["argv"],
                [sys.executable, script_path, "${RUN_ROOT}/artifacts/od-vmhost-smoke"],
            )

            for corruption in ("command", "source", "marker"):
                with self.subTest(corruption=corruption), tempfile.TemporaryDirectory() as directory:
                    corrupted = copy.deepcopy(harness.input_manifest())
                    process = corrupted["vmhost_process"]
                    if corruption == "command":
                        entry = next(item for item in process["build_profile"]["commands"]
                                     if item["step"] == step_name)
                        entry["argv"][1] = "tests/system/vmhost_control_v1_same_code_smoke.py"
                    elif corruption == "source":
                        entry = next(item for item in process["inputs"]
                                     if item["path"] == script_path)
                        entry["sha256"] = "0" * 64
                    else:
                        process["success_markers"]["control_v1_run_stop_id"] += " forged"
                    manifest_path = Path(directory) / "input_manifest.json"
                    raw = harness.canonical_json(corrupted)
                    manifest_path.write_bytes(raw)
                    with self.assertRaises(harness.HarnessError):
                        harness.validate_input_manifest_file(
                            manifest_path, harness.sha256_bytes(raw)
                        )
        finally:
            temporary.cleanup()

    def test_vmhost_process_v1_isolation_gate_is_pinned_once_and_fails_closed(self):
        step_name = "process_control_v1_isolation"
        script_path = "tests/system/vmhost_control_v1_isolation_smoke.py"
        marker = (
            "VMHost control V1 isolation smoke passed: SIGSEGV stayed within helper A; "
            "helper B completed exact STOP, fresh RUN, and idle EOF"
        )
        self_test_name = "test_vmhost_process_v1_isolation_gate_is_pinned_once_and_fails_closed"
        self.assertEqual(harness.SELF_TEST_NAMES.count(self_test_name), 1)
        self.assertIn(
            f"__main__.Stage3HarnessValidationTests.{self_test_name}",
            harness.SELF_TEST_IDENTITIES,
        )
        temporary, run_root = temporary_run_root()
        try:
            commands = harness.vmhost_process_commands(run_root)
            vmhost_path = str(harness.vmhost_process_paths(run_root)["vmhost"])
            self.assertEqual(harness.PROCESS_SUITE_STEPS.count(step_name), 1)
            self.assertEqual(commands[step_name], [sys.executable, script_path, vmhost_path])
            self.assertEqual(harness.PROCESS_STEP_TIMEOUTS[step_name], 30)
            self.assertEqual(harness.PROCESS_SUCCESS_MARKERS["control_v1_isolation"], marker)
            self.assertEqual(harness.PROCESS_SOURCE_PATHS.count(script_path), 1)
            manifest = harness.input_manifest()["vmhost_process"]
            source_record = {
                "path": script_path,
                "sha256": harness.sha256_file(harness.ROOT / script_path),
            }
            self.assertIn(source_record, manifest["inputs"])
            command = next(
                item for item in manifest["build_profile"]["commands"]
                if item["step"] == step_name
            )
            self.assertEqual(
                command["argv"],
                [sys.executable, script_path, "${RUN_ROOT}/artifacts/od-vmhost-smoke"],
            )

            for corruption in ("command", "source", "marker"):
                with self.subTest(corruption=corruption), tempfile.TemporaryDirectory() as directory:
                    corrupted = copy.deepcopy(harness.input_manifest())
                    process = corrupted["vmhost_process"]
                    if corruption == "command":
                        entry = next(
                            item for item in process["build_profile"]["commands"]
                            if item["step"] == step_name
                        )
                        entry["argv"][1] = "tests/system/vmhost_control_v1_same_code_smoke.py"
                    elif corruption == "source":
                        entry = next(
                            item for item in process["inputs"] if item["path"] == script_path
                        )
                        entry["sha256"] = "0" * 64
                    else:
                        process["success_markers"]["control_v1_isolation"] += " forged"
                    manifest_path = Path(directory) / "input_manifest.json"
                    raw = harness.canonical_json(corrupted)
                    manifest_path.write_bytes(raw)
                    with self.assertRaises(harness.HarnessError):
                        harness.validate_input_manifest_file(
                            manifest_path, harness.sha256_bytes(raw)
                        )
        finally:
            temporary.cleanup()

    def test_vmhost_process_run_id_exhaustion_gate_is_pinned_once_and_fails_closed(self):
        step_name = "process_control_v1_run_id_exhaustion"
        script_path = "tests/system/vmhost_control_v1_run_id_exhaustion_smoke.py"
        marker = (
            "VMHost control V1 smoke passed: RUN-ID exhaustion does not wrap and "
            "fresh connection accepts ID 1"
        )
        self_test_name = "test_vmhost_process_run_id_exhaustion_gate_is_pinned_once_and_fails_closed"
        self.assertEqual(harness.SELF_TEST_NAMES.count(self_test_name), 1)
        self.assertIn(
            f"__main__.Stage3HarnessValidationTests.{self_test_name}",
            harness.SELF_TEST_IDENTITIES,
        )
        temporary, run_root = temporary_run_root()
        try:
            commands = harness.vmhost_process_commands(run_root)
            vmhost_path = str(harness.vmhost_process_paths(run_root)["vmhost"])
            self.assertEqual(harness.PROCESS_SUITE_STEPS.count(step_name), 1)
            self.assertEqual(
                commands[step_name], [sys.executable, script_path, vmhost_path]
            )
            self.assertEqual(harness.PROCESS_STEP_TIMEOUTS[step_name], 30)
            self.assertEqual(harness.PROCESS_SUCCESS_MARKERS["control_v1_run_id_exhaustion"], marker)
            self.assertEqual(harness.PROCESS_SOURCE_PATHS.count(script_path), 1)
            manifest = harness.input_manifest()["vmhost_process"]
            source_record = {
                "path": script_path,
                "sha256": harness.sha256_file(harness.ROOT / script_path),
            }
            self.assertIn(source_record, manifest["inputs"])
            command = next(item for item in manifest["build_profile"]["commands"]
                           if item["step"] == step_name)
            self.assertEqual(
                command["argv"],
                [sys.executable, script_path, "${RUN_ROOT}/artifacts/od-vmhost-smoke"],
            )

            for corruption in ("command", "source", "marker"):
                with self.subTest(corruption=corruption), tempfile.TemporaryDirectory() as directory:
                    corrupted = copy.deepcopy(harness.input_manifest())
                    process = corrupted["vmhost_process"]
                    if corruption == "command":
                        entry = next(item for item in process["build_profile"]["commands"]
                                     if item["step"] == step_name)
                        entry["argv"][1] = "tests/system/vmhost_control_v1_same_code_smoke.py"
                    elif corruption == "source":
                        entry = next(item for item in process["inputs"]
                                     if item["path"] == script_path)
                        entry["sha256"] = "0" * 64
                    else:
                        process["success_markers"]["control_v1_run_id_exhaustion"] += " forged"
                    manifest_path = Path(directory) / "input_manifest.json"
                    raw = harness.canonical_json(corrupted)
                    manifest_path.write_bytes(raw)
                    with self.assertRaises(harness.HarnessError):
                        harness.validate_input_manifest_file(
                            manifest_path, harness.sha256_bytes(raw)
                        )
        finally:
            temporary.cleanup()

    def test_vmhost_process_manifest_self_check_rejects_code_mismatch_drift(self):
        process_script = "tests/system/vmhost_control_v1_code_mismatch_smoke.py"
        for corruption in ("command", "source", "marker"):
            with self.subTest(corruption=corruption), tempfile.TemporaryDirectory() as temporary:
                manifest = copy.deepcopy(harness.input_manifest())
                process = manifest["vmhost_process"]
                if corruption == "command":
                    command = next(
                        item for item in process["build_profile"]["commands"]
                        if item["step"] == "process_control_v1_code_mismatch"
                    )
                    command["argv"][1] = "tests/system/vmhost_control_v1_same_code_smoke.py"
                elif corruption == "source":
                    record = next(item for item in process["inputs"] if item["path"] == process_script)
                    record["sha256"] = "0" * 64
                else:
                    process["success_markers"]["control_v1_code_mismatch"] += " forged"
                manifest_path = Path(temporary) / "input_manifest.json"
                raw = harness.canonical_json(manifest)
                manifest_path.write_bytes(raw)
                with self.assertRaisesRegex(
                    harness.HarnessError,
                    "VMHost process (command manifest|source inputs|success markers) differ(?:s)?",
                ):
                    harness.validate_input_manifest_file(
                        manifest_path, harness.sha256_bytes(raw)
                    )

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


    def test_s3_34am_addr32_disp8_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34am_ir_dword_addr32_disp8_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_disp8_store_binds_all_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34am_host_addr32_disp8_dword_store_wrapped_ebx", "local_acceptance",
                "od-host", "tests::s3_20db_addr32_disp8_dword_store_uses_wrapped_ebx",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34am_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34am_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            gate for gate in missing_manifest["gates"]
            if not gate["step"].startswith("s3_34am_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_addr32_disp8_store"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)



    def test_s3_34an_addr32_disp32_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34an_ir_dword_addr32_disp32_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_disp32_store_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34an_host_addr32_disp32_dword_store_wrapped_ebx", "local_acceptance",
                "od-host", "tests::s3_20dc_addr32_disp32_dword_store_uses_wrapped_ebx",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34an_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34an_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            gate for gate in missing_manifest["gates"]
            if not gate["step"].startswith("s3_34an_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_addr32_disp32_store"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)



    def test_s3_34ao_rbx_rcx_sib_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34ao_ir_dword_rbx_rcx_sib_load_exact_admission", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_rbx_rcx_sib_load_admits_only_exact_8b040b",
            ),
            (
                "s3_34ao_host_rbx_rcx_sib_dword_load_full_width_base_index", "local_acceptance",
                "od-host", "tests::s3_20dd_rbx_rcx_sib_dword_load_uses_full_width_base_and_index",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34ao_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34ao_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            gate for gate in missing_manifest["gates"]
            if not gate["step"].startswith("s3_34ao_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_rbx_rcx_sib_load"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)



    def test_s3_34ap_rbx_rcx_sib_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34ap_ir_dword_rbx_rcx_sib_store_exact_admission", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_rbx_rcx_sib_store_admits_only_exact_89040b",
            ),
            (
                "s3_34ap_host_rbx_rcx_sib_dword_store_full_width_base_index", "local_acceptance",
                "od-host", "tests::s3_20de_rbx_rcx_sib_dword_store_uses_full_width_base_and_index",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34ap_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34ap_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            gate for gate in missing_manifest["gates"]
            if not gate["step"].startswith("s3_34ap_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_rbx_rcx_sib_store"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)



    def test_s3_34aq_rbx_rcx_sib_scaled_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34aq_ir_dword_rbx_rcx_sib_scaled_load_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_rbx_rcx_sib_dword_load_binds_exact_scales_2_4_8",
            ),
            (
                "s3_34aq_host_rbx_rcx_sib_scaled_dword_load_full_width_base_index", "local_acceptance",
                "od-host", "tests::s3_20df_rbx_rcx_sib_scaled_dword_load_uses_full_width_base_and_index",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34aq_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34aq_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            gate for gate in missing_manifest["gates"]
            if not gate["step"].startswith("s3_34aq_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_scaled_rbx_rcx_load"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)



    def test_s3_34ar_rbx_rcx_sib_scaled_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34ar_ir_dword_rbx_rcx_sib_scaled_store_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_rbx_rcx_sib_store_binds_exact_scales_2_4_8",
            ),
            (
                "s3_34ar_host_rbx_rcx_scaled_sib_dword_store_full_width_base_index", "local_acceptance",
                "od-host", "tests::s3_20dg_rbx_rcx_scaled_sib_dword_store_uses_full_width_base_and_index",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34ar_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34ar_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            gate for gate in missing_manifest["gates"]
            if not gate["step"].startswith("s3_34ar_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_scaled_rbx_rcx_store"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)



    def test_s3_34as_rbx_rcx_scaled_disp8_sib_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34as_ir_dword_rbx_rcx_sib_disp8_scaled_load_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_rbx_rcx_sib_disp8_load_binds_all_scales_and_signed_bytes",
            ),
            (
                "s3_34as_host_rbx_rcx_scaled_disp8_sib_dword_load_signed_displacement", "local_acceptance",
                "od-host", "tests::s3_20dh_rbx_rcx_scaled_disp8_sib_dword_load_uses_signed_displacement",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34as_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34as_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            gate for gate in missing_manifest["gates"]
            if not gate["step"].startswith("s3_34as_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_scaled_disp8_sib_load"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)



    def test_s3_34at_rbx_rcx_scaled_disp8_sib_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34at_ir_dword_rbx_rcx_sib_disp8_scaled_store_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_rbx_rcx_sib_disp8_store_binds_all_scales_and_signed_bytes",
            ),
            (
                "s3_34at_host_rbx_rcx_scaled_disp8_sib_dword_store_checked_semantics", "local_acceptance",
                "od-host", "tests::s3_20di_rbx_rcx_scaled_disp8_sib_dword_store_preserves_checked_semantics",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34at_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34at_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            gate for gate in missing_manifest["gates"]
            if not gate["step"].startswith("s3_34at_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_scaled_disp8_sib_store"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)



    def test_s3_34au_rbx_rcx_scaled_disp32_sib_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34au_ir_dword_rbx_rcx_sib_disp32_scaled_load_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_rbx_rcx_sib_disp32_load_binds_all_scales_and_source_bytes",
            ),
            (
                "s3_34au_host_rbx_rcx_scaled_disp32_sib_dword_load_checked_semantics", "local_acceptance",
                "od-host", "tests::s3_20dj_rbx_rcx_scaled_disp32_sib_dword_load_preserves_checked_semantics",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34au_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34au_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            gate for gate in missing_manifest["gates"]
            if not gate["step"].startswith("s3_34au_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_scaled_disp32_sib_load"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)



    def test_s3_34av_rbx_rcx_scaled_disp32_sib_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34av_ir_dword_rbx_rcx_sib_disp32_scaled_store_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_rbx_rcx_sib_disp32_store_binds_all_scales_and_source_bytes",
            ),
            (
                "s3_34av_host_rbx_rcx_scaled_disp32_sib_dword_store_checked_semantics", "local_acceptance",
                "od-host", "tests::s3_20dk_rbx_rcx_scaled_disp32_sib_dword_store_preserves_checked_semantics",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34av_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34av_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            gate for gate in missing_manifest["gates"]
            if not gate["step"].startswith("s3_34av_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_scaled_disp32_sib_store"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)



    def test_s3_34aw_addr32_scaled_sib_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34aw_ir_dword_addr32_rbx_rcx_sib_scaled_load_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_addr32_rbx_rcx_sib_load_binds_exact_scales_and_prefix",
            ),
            (
                "s3_34aw_host_addr32_scaled_sib_dword_load_wraps_zero_extends", "local_acceptance",
                "od-host", "tests::s3_20dl_addr32_scaled_sib_dword_load_wraps_and_zero_extends",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34aw_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34aw_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            gate for gate in missing_manifest["gates"]
            if not gate["step"].startswith("s3_34aw_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_addr32_scaled_sib_load"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34ax_addr32_scaled_sib_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34ax_ir_dword_addr32_rbx_rcx_sib_scaled_store_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_addr32_rbx_rcx_sib_store_binds_exact_scales_and_prefix",
            ),
            (
                "s3_34ax_host_addr32_scaled_sib_dword_store_wraps_preserves_state", "local_acceptance",
                "od-host", "tests::s3_20dm_addr32_scaled_sib_dword_store_wraps_and_preserves_state",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34ax_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34ax_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            gate for gate in missing_manifest["gates"]
            if not gate["step"].startswith("s3_34ax_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_addr32_scaled_sib_store"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)


    def test_s3_34ay_addr32_scaled_disp8_sib_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34ay_ir_dword_addr32_rbx_rcx_sib_disp8_load_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_addr32_rbx_rcx_sib_disp8_load_binds_all_scales_and_signed_bytes",
            ),
            (
                "s3_34ay_host_addr32_scaled_disp8_sib_dword_load_signed_displacement", "local_acceptance",
                "od-host", "tests::s3_20dn_addr32_scaled_disp8_sib_dword_load_uses_signed_displacement",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34ay_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34ay_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            gate for gate in missing_manifest["gates"]
            if not gate["step"].startswith("s3_34ay_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_addr32_scaled_sib_disp8_load"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)


    def test_s3_34az_addr32_scaled_disp8_sib_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34az_ir_dword_addr32_rbx_rcx_sib_disp8_store_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_addr32_rbx_rcx_sib_disp8_store_binds_all_scales_and_signed_bytes",
            ),
            (
                "s3_34az_host_addr32_scaled_disp8_sib_dword_store_wraps_preserves_state", "local_acceptance",
                "od-host", "tests::s3_20do_addr32_scaled_disp8_sib_dword_store_wraps_and_preserves_state",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34az_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34az_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            gate for gate in missing_manifest["gates"]
            if not gate["step"].startswith("s3_34az_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_addr32_scaled_sib_disp8_store"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)


    def test_s3_34ba_addr32_scaled_disp32_sib_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34ba_ir_dword_addr32_rbx_rcx_sib_disp32_load_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_addr32_rbx_rcx_sib_disp32_load_binds_scales_and_all_displacement_bytes",
            ),
            (
                "s3_34ba_host_addr32_scaled_disp32_sib_dword_load_wraps_preserves_state", "local_acceptance",
                "od-host", "tests::s3_20dp_addr32_scaled_disp32_sib_dword_load_wraps_and_preserves_state",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34ba_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34ba_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            gate for gate in missing_manifest["gates"]
            if not gate["step"].startswith("s3_34ba_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_addr32_scaled_sib_disp32_load"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "test result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34bb_addr32_scaled_disp32_sib_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bb_ir_dword_addr32_rbx_rcx_sib_disp32_store_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_addr32_rbx_rcx_sib_disp32_store_binds_scales_and_all_displacement_bytes",
            ),
            (
                "s3_34bb_host_addr32_scaled_disp32_sib_dword_store_wraps_preserves_state", "local_acceptance",
                "od-host", "tests::s3_20dq_addr32_scaled_disp32_sib_dword_store_wraps_and_preserves_state",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bb_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bb_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        self.assertIn("crates/od-jit/src/memory_load_v2.rs", harness.LOCAL_RUST_TEST_SOURCES)
        self.assertIn("crates/od-host/src/lib.rs", harness.LOCAL_RUST_TEST_SOURCES)

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest = json.loads(json.dumps(source_manifest))
        missing_manifest["gates"] = [
            gate for gate in missing_manifest["gates"]
            if not gate["step"].startswith("s3_34bb_")
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest)

        wrong_identity_manifest = json.loads(json.dumps(source_manifest))
        wrong_identity_manifest["gates"][-1]["identity"] = "tests::unrelated_addr32_scaled_sib_disp32_store"
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(wrong_identity_manifest)

        bad_source_pin = json.loads(json.dumps(source_manifest))
        source_pin = next(
            item for item in bad_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        source_pin["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(bad_source_pin)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, package, identity in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(step=step, evidence="missing"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate is missing or not passed: {step}",
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                wrong_target = passing_rust_gate_steps(log_dir)
                wrong_target[step]["argv"] = [
                    "cargo", "test", "--offline", "-p", package, "--test", "unrelated_target",
                    identity, "--", "--exact", "--test-threads=1",
                ]
                with self.subTest(step=step, evidence="wrong target"), self.assertRaisesRegex(
                    harness.HarnessError,
                    f"required Rust test gate invocation is not exact: {step}",
                ):
                    harness.validate_required_rust_test_evidence(wrong_target, run_root)

                for label, output in (
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("zero match", "\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34bc_rbp_disp8_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bc_ir_dword_rbp_disp8_load_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_rbp_disp8_load_binds_all_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34bc_host_rbp_disp8_dword_load_checked_semantics", "local_acceptance",
                "od-host", "tests::s3_20dr_rbp_disp8_dword_load_preserves_ss_fault_and_checked_state",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bc_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bc_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                zero_match = passing_rust_gate_steps(log_dir)
                replace_step_log_output(
                    zero_match[step],
                    "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                )
                with self.subTest(step=step, evidence="zero matched tests"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(zero_match, run_root)

                wrong_identity = passing_rust_gate_steps(log_dir)
                replace_step_log_output(wrong_identity[step], "test tests::unrelated_test ... ok\n")
                with self.subTest(step=step, evidence="wrong identity"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(wrong_identity, run_root)

    def test_s3_34bg_addr32_rbp_disp8_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bg_ir_dword_addr32_rbp_disp8_load_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_addr32_rbp_disp8_load_binds_all_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34bg_host_addr32_rbp_disp8_dword_load_checked_state", "local_acceptance",
                "od-host", "tests::s3_20dv_addr32_rbp_disp8_dword_load_preserves_checked_state",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bg_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bg_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                for label, output in (
                    (
                        "zero matched tests",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    if label == "failed":
                        changed[step].update(
                            {"status": "failed", "returncode": 101, "failure": "test failed"}
                        )
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34bh_addr32_rbp_disp8_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bh_ir_addr32_rbp_disp8_dword_store_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_addr32_rbp_disp8_store_binds_all_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34bh_host_addr32_rbp_disp8_dword_store_wrapped_ebp", "local_acceptance",
                "od-host", "tests::s3_20dw_addr32_rbp_disp8_dword_store_uses_wrapped_ebp",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bh_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bh_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest_gate = {**source_manifest, "gates": manifest[:-1]}
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                zero_match = passing_rust_gate_steps(log_dir)
                replace_step_log_output(
                    zero_match[step],
                    "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                )
                with self.subTest(step=step, evidence="zero matched tests"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(zero_match, run_root)

                wrong_identity = passing_rust_gate_steps(log_dir)
                replace_step_log_output(wrong_identity[step], "test tests::unrelated_test ... ok\n")
                with self.subTest(step=step, evidence="wrong identity"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(wrong_identity, run_root)

                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing evidence"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[0][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)

    def test_s3_34bi_addr32_rbp_disp32_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bi_ir_addr32_rbp_disp32_dword_load_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_addr32_rbp_disp32_load_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34bi_host_addr32_rbp_disp32_dword_load_checked_state", "local_acceptance",
                "od-host", "tests::s3_20dx_addr32_rbp_disp32_dword_load_preserves_checked_state",
            ),
        )
        self.assertIn(
            "test_s3_34bi_addr32_rbp_disp32_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bi_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bi_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest_gate = {**source_manifest, "gates": manifest[:-1]}
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                zero_match = passing_rust_gate_steps(log_dir)
                replace_step_log_output(
                    zero_match[step],
                    "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                )
                with self.subTest(step=step, evidence="zero matched tests"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(zero_match, run_root)

                wrong_identity = passing_rust_gate_steps(log_dir)
                replace_step_log_output(wrong_identity[step], "test tests::unrelated_test ... ok\n")
                with self.subTest(step=step, evidence="wrong identity"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(wrong_identity, run_root)

                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing evidence"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[0][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)

    def test_s3_34bj_addr32_rbp_disp32_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bj_ir_addr32_rbp_disp32_dword_store_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_addr32_rbp_disp32_store_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34bj_host_addr32_rbp_disp32_dword_store_wrapped_ebp", "local_acceptance",
                "od-host", "tests::s3_20dy_addr32_rbp_disp32_dword_store_uses_wrapped_ebp",
            ),
        )
        self.assertIn(
            "test_s3_34bj_addr32_rbp_disp32_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bj_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bj_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest_gate = {**source_manifest, "gates": manifest[:-1]}
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                zero_match = passing_rust_gate_steps(log_dir)
                replace_step_log_output(
                    zero_match[step],
                    "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                )
                with self.subTest(step=step, evidence="zero matched tests"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(zero_match, run_root)

                wrong_identity = passing_rust_gate_steps(log_dir)
                replace_step_log_output(wrong_identity[step], "test tests::unrelated_test ... ok\n")
                with self.subTest(step=step, evidence="wrong identity"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(wrong_identity, run_root)

                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing evidence"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[0][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)

    def test_s3_34bd_rbp_disp8_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bd_ir_dword_rbp_disp8_store_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_rbp_disp8_store_binds_all_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34bd_host_rbp_disp8_dword_store_checked_semantics", "local_acceptance",
                "od-host", "tests::s3_20ds_rbp_disp8_dword_store_preserves_ss_fault_and_checked_state",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bd_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bd_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                zero_match = passing_rust_gate_steps(log_dir)
                replace_step_log_output(
                    zero_match[step],
                    "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                )
                with self.subTest(step=step, evidence="zero matched tests"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(zero_match, run_root)

                wrong_identity = passing_rust_gate_steps(log_dir)
                replace_step_log_output(wrong_identity[step], "test tests::unrelated_test ... ok\n")
                with self.subTest(step=step, evidence="wrong identity"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(wrong_identity, run_root)

    def test_s3_34be_rbp_disp32_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34be_ir_dword_rbp_disp32_load_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_rbp_disp32_load_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34be_host_rbp_disp32_dword_load_checked_semantics", "local_acceptance",
                "od-host", "tests::s3_20dt_rbp_disp32_dword_load_preserves_ss_fault_and_checked_state",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34be_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34be_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_gate = passing_rust_gate_steps(log_dir)
                del missing_gate[step]
                with self.subTest(step=step, evidence="missing gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_gate, run_root)

                zero_match = passing_rust_gate_steps(log_dir)
                replace_step_log_output(
                    zero_match[step],
                    "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                )
                with self.subTest(step=step, evidence="zero matched tests"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(zero_match, run_root)

                wrong_identity = passing_rust_gate_steps(log_dir)
                replace_step_log_output(wrong_identity[step], "test tests::unrelated_test ... ok\n")
                with self.subTest(step=step, evidence="wrong identity"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(wrong_identity, run_root)

    def test_s3_34bf_rbp_disp32_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bf_ir_dword_rbp_disp32_store_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_rbp_disp32_store_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34bf_host_rbp_disp32_dword_store_checked_semantics", "local_acceptance",
                "od-host", "tests::s3_20du_rbp_disp32_dword_store_preserves_ss_fault_and_checked_state",
            ),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bf_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bf_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        missing_manifest_gate = dict(source_manifest)
        missing_manifest_gate["gates"] = [
            gate for gate in manifest if gate["step"] != expected[0][0]
        ]
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_gate = passing_rust_gate_steps(log_dir)
                del missing_gate[step]
                with self.subTest(step=step, evidence="omitted summary"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_gate, run_root)

                for label, output in (
                    ("zero match", "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

                wrong_identity = passing_rust_gate_steps(log_dir)
                replace_step_log_output(wrong_identity[step], "test tests::unrelated_test ... ok\n")
                with self.subTest(step=step, evidence="wrong identity"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(wrong_identity, run_root)

    def test_s3_34bk_rsp_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bk_ir_rsp_dword_load_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_rsp_sib_load_binds_exact_source_and_site",
            ),
            (
                "s3_34bk_host_rsp_dword_load_no_replay_retry_checked_state", "local_acceptance",
                "od-host", "tests::s3_20dz_rsp_dword_load_preserves_ss_fault_and_checked_state",
            ),
        )
        self.assertIn(
            "test_s3_34bk_rsp_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bk_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bk_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    ("zero tests", "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    summary = passing_final_summary(run_id, summary_run_root / "logs")
                    summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                    input_manifest_path = temporary_dir / "input_manifest.json"
                    manifest_raw = harness.write_json_create(
                        input_manifest_path, harness.input_manifest()
                    )
                    summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)


    def test_s3_34bl_rsp_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bl_ir_rsp_dword_store_source_binding", "local_acceptance",
                "od-jit", "memory_load_v2::tests::ir_dword_rsp_sib_store_binds_exact_source_and_site",
            ),
            (
                "s3_34bl_host_rsp_dword_store_checked_state", "local_acceptance",
                "od-host", "tests::s3_20ea_rsp_dword_store_preserves_ss_fault_and_checked_state",
            ),
        )
        self.assertIn(
            "test_s3_34bl_rsp_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bl_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bl_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    ("zero tests", "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)

    def test_s3_34bm_rsp_disp8_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bm_ir_rsp_disp8_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rsp_sib_disp8_load_binds_all_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34bm_host_rsp_disp8_dword_load_checked_state", "local_acceptance",
                "od-host",
                "tests::s3_20eb_rsp_disp8_dword_load_preserves_ss_fault_and_checked_state",
            ),
        )
        self.assertIn(
            "test_s3_34bm_rsp_disp8_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bm_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bm_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)


    def test_s3_34bn_rsp_disp8_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bn_ir_rsp_disp8_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rsp_sib_disp8_store_binds_all_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34bn_host_rsp_disp8_dword_store_checked_state", "local_acceptance",
                "od-host",
                "tests::s3_20ec_rsp_disp8_dword_store_preserves_ss_fault_and_checked_state",
            ),
        )
        self.assertIn(
            "test_s3_34bn_rsp_disp8_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bn_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bn_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)


    def test_s3_34bo_rsp_disp32_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bo_ir_rsp_disp32_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rsp_sib_disp32_load_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34bo_host_rsp_disp32_dword_load_checked_state", "local_acceptance",
                "od-host",
                "tests::s3_20ed_rsp_disp32_dword_load_preserves_ss_fault_and_checked_state",
            ),
        )
        self.assertIn(
            "test_s3_34bo_rsp_disp32_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bo_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bo_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)

    def test_s3_34bp_rsp_disp32_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bp_ir_rsp_disp32_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rsp_sib_disp32_store_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34bp_host_rsp_disp32_dword_store_checked_state", "local_acceptance",
                "od-host",
                "tests::s3_20ee_rsp_disp32_dword_store_preserves_ss_fault_and_checked_state",
            ),
        )
        self.assertIn(
            "test_s3_34bp_rsp_disp32_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bp_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bp_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)

    def test_s3_34bq_addr32_rsp_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bq_ir_addr32_rsp_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_rsp_sib_load_binds_exact_source_and_site",
            ),
            (
                "s3_34bq_host_addr32_rsp_dword_load_zero_extended_esp", "local_acceptance",
                "od-host",
                "tests::s3_20ef_addr32_rsp_dword_load_uses_zero_extended_esp",
            ),
        )
        self.assertIn(
            "test_s3_34bq_addr32_rsp_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bq_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bq_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)


    def test_s3_34br_addr32_rsp_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34br_ir_addr32_rsp_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_rsp_sib_store_binds_exact_source_and_site",
            ),
            (
                "s3_34br_host_addr32_rsp_dword_store_zero_extended_esp", "local_acceptance",
                "od-host",
                "tests::s3_20eg_addr32_rsp_dword_store_uses_zero_extended_esp",
            ),
        )
        self.assertIn(
            "test_s3_34br_addr32_rsp_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34br_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34br_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)


    def test_s3_34bs_addr32_rsp_disp8_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bs_ir_addr32_rsp_disp8_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_rsp_sib_disp8_load_binds_all_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34bs_host_addr32_rsp_disp8_dword_load_wrapped_esp", "local_acceptance",
                "od-host",
                "tests::s3_20eh_addr32_rsp_disp8_dword_load_uses_wrapped_esp",
            ),
        )
        self.assertIn(
            "test_s3_34bs_addr32_rsp_disp8_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bs_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bs_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)


    def test_s3_34bt_addr32_rsp_disp8_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bt_ir_addr32_rsp_disp8_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_rsp_sib_disp8_store_binds_all_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34bt_host_addr32_rsp_disp8_dword_store_wrapped_esp", "local_acceptance",
                "od-host",
                "tests::s3_20ei_addr32_rsp_disp8_dword_store_uses_wrapped_esp",
            ),
        )
        self.assertIn(
            "test_s3_34bt_addr32_rsp_disp8_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bt_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bt_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        pinned_jit_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        self.assertEqual(
            pinned_jit_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-jit/src/memory_load_v2.rs"),
        )
        wrong_source_pin = {
            **source_manifest,
            "sources": [dict(item) for item in source_manifest["sources"]],
        }
        next(
            item for item in wrong_source_pin["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(wrong_source_pin)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)


    def test_s3_34bu_addr32_rsp_disp32_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bu_ir_addr32_rsp_disp32_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_rsp_sib_disp32_load_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34bu_host_addr32_rsp_disp32_dword_load_wrapped_esp", "local_acceptance",
                "od-host",
                "tests::s3_20ej_addr32_rsp_disp32_dword_load_uses_wrapped_esp",
            ),
        )
        self.assertIn(
            "test_s3_34bu_addr32_rsp_disp32_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bu_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bu_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        pinned_jit_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        self.assertEqual(
            pinned_jit_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-jit/src/memory_load_v2.rs"),
        )
        pinned_host_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )
        self.assertEqual(
            pinned_host_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-host/src/lib.rs"),
        )
        wrong_source_pin = {
            **source_manifest,
            "sources": [dict(item) for item in source_manifest["sources"]],
        }
        next(
            item for item in wrong_source_pin["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(wrong_source_pin)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)


    def test_s3_34bv_addr32_rsp_disp32_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bv_ir_addr32_rsp_disp32_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_rsp_sib_disp32_store_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34bv_host_addr32_rsp_disp32_dword_store_wrapped_esp", "local_acceptance",
                "od-host",
                "tests::s3_20ek_addr32_rsp_disp32_dword_store_uses_wrapped_esp",
            ),
        )
        self.assertIn(
            "test_s3_34bv_addr32_rsp_disp32_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bv_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bv_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        pinned_jit_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        self.assertEqual(
            pinned_jit_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-jit/src/memory_load_v2.rs"),
        )
        pinned_host_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )
        self.assertEqual(
            pinned_host_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-host/src/lib.rs"),
        )
        wrong_source_pin = {
            **source_manifest,
            "sources": [dict(item) for item in source_manifest["sources"]],
        }
        next(
            item for item in wrong_source_pin["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(wrong_source_pin)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)


    def test_s3_34bw_absolute_sib_disp32_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bw_ir_absolute_sib_disp32_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_absolute_sib_disp32_load_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34bw_host_absolute_sib_disp32_dword_load_signed_absolute_address", "local_acceptance",
                "od-host",
                "tests::s3_20el_absolute_sib_disp32_dword_load_uses_signed_absolute_address",
            ),
        )
        self.assertIn(
            "test_s3_34bw_absolute_sib_disp32_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bw_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bw_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        pinned_jit_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        self.assertEqual(
            pinned_jit_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-jit/src/memory_load_v2.rs"),
        )
        pinned_host_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )
        self.assertEqual(
            pinned_host_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-host/src/lib.rs"),
        )
        wrong_source_pin = {
            **source_manifest,
            "sources": [dict(item) for item in source_manifest["sources"]],
        }
        next(
            item for item in wrong_source_pin["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(wrong_source_pin)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)


    def test_s3_34bx_absolute_sib_disp32_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bx_ir_absolute_sib_disp32_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_absolute_sib_disp32_store_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34bx_host_absolute_sib_disp32_dword_store_signed_absolute_address", "local_acceptance",
                "od-host",
                "tests::s3_20em_absolute_sib_disp32_dword_store_uses_signed_absolute_address",
            ),
        )
        self.assertIn(
            "test_s3_34bx_absolute_sib_disp32_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bx_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bx_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        pinned_jit_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        self.assertEqual(
            pinned_jit_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-jit/src/memory_load_v2.rs"),
        )
        pinned_host_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )
        self.assertEqual(
            pinned_host_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-host/src/lib.rs"),
        )
        wrong_source_pin = {
            **source_manifest,
            "sources": [dict(item) for item in source_manifest["sources"]],
        }
        next(
            item for item in wrong_source_pin["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(wrong_source_pin)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)



    def test_s3_34by_addr32_absolute_sib_disp32_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34by_ir_addr32_absolute_sib_disp32_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_absolute_sib_disp32_load_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34by_host_addr32_absolute_sib_disp32_dword_load_zero_extended_absolute_address", "local_acceptance",
                "od-host",
                "tests::s3_20en_addr32_absolute_sib_disp32_dword_load_uses_zero_extended_absolute_address",
            ),
        )
        self.assertIn(
            "test_s3_34by_addr32_absolute_sib_disp32_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34by_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34by_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        pinned_jit_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        self.assertEqual(
            pinned_jit_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-jit/src/memory_load_v2.rs"),
        )
        pinned_host_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )
        self.assertEqual(
            pinned_host_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-host/src/lib.rs"),
        )
        wrong_source_pin = {
            **source_manifest,
            "sources": [dict(item) for item in source_manifest["sources"]],
        }
        next(
            item for item in wrong_source_pin["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(wrong_source_pin)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)




    def test_s3_34bz_addr32_absolute_sib_disp32_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34bz_ir_addr32_absolute_sib_disp32_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_absolute_sib_disp32_store_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34bz_host_addr32_absolute_sib_disp32_dword_store_zero_extended_absolute_address", "local_acceptance",
                "od-host",
                "tests::s3_20eo_addr32_absolute_sib_disp32_dword_store_uses_zero_extended_absolute_address",
            ),
        )
        self.assertIn(
            "test_s3_34bz_addr32_absolute_sib_disp32_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34bz_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34bz_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        pinned_jit_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        self.assertEqual(
            pinned_jit_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-jit/src/memory_load_v2.rs"),
        )
        pinned_host_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )
        self.assertEqual(
            pinned_host_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-host/src/lib.rs"),
        )
        wrong_source_pin = {
            **source_manifest,
            "sources": [dict(item) for item in source_manifest["sources"]],
        }
        next(
            item for item in wrong_source_pin["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(wrong_source_pin)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)







    def test_s3_34ca_rcx_index_no_base_sib_disp32_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34ca_ir_rcx_index_no_base_sib_disp32_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rcx_index_no_base_sib_disp32_load_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34ca_host_rcx_index_no_base_sib_disp32_dword_load_signed_displacement", "local_acceptance",
                "od-host",
                "tests::s3_20ep_rcx_index_no_base_sib_disp32_dword_load_uses_signed_displacement",
            ),
        )
        self.assertIn(
            "test_s3_34ca_rcx_index_no_base_sib_disp32_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34ca_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34ca_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        pinned_jit_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        self.assertEqual(
            pinned_jit_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-jit/src/memory_load_v2.rs"),
        )
        pinned_host_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )
        self.assertEqual(
            pinned_host_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-host/src/lib.rs"),
        )
        wrong_source_pin = {
            **source_manifest,
            "sources": [dict(item) for item in source_manifest["sources"]],
        }
        next(
            item for item in wrong_source_pin["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(wrong_source_pin)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)


    def test_s3_34cb_rcx_index_no_base_sib_disp32_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34cb_ir_rcx_index_no_base_sib_disp32_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rcx_index_no_base_sib_disp32_store_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34cb_host_rcx_index_no_base_sib_disp32_dword_store_signed_displacement", "local_acceptance",
                "od-host",
                "tests::s3_20eq_rcx_index_no_base_sib_disp32_dword_store_uses_signed_displacement",
            ),
        )
        self.assertIn(
            "test_s3_34cb_rcx_index_no_base_sib_disp32_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34cb_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34cb_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        pinned_jit_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        self.assertEqual(
            pinned_jit_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-jit/src/memory_load_v2.rs"),
        )
        pinned_host_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )
        self.assertEqual(
            pinned_host_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-host/src/lib.rs"),
        )
        wrong_source_pin = {
            **source_manifest,
            "sources": [dict(item) for item in source_manifest["sources"]],
        }
        next(
            item for item in wrong_source_pin["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(wrong_source_pin)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)


    def test_s3_34cc_rcx_index_no_base_sib_scaled_disp32_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34cc_ir_rcx_index_no_base_sib_scaled_disp32_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rcx_index_no_base_sib_scaled_disp32_load_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34cc_host_rcx_index_no_base_sib_scaled_disp32_dword_load_full_width_scaled_index", "local_acceptance",
                "od-host",
                "tests::s3_20er_rcx_index_no_base_sib_scaled_disp32_dword_load_uses_full_width_scaled_index",
            ),
        )
        self.assertIn(
            "test_s3_34cc_rcx_index_no_base_sib_scaled_disp32_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34cc_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34cc_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        pinned_jit_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        self.assertEqual(
            pinned_jit_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-jit/src/memory_load_v2.rs"),
        )
        pinned_host_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )
        self.assertEqual(
            pinned_host_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-host/src/lib.rs"),
        )
        wrong_source_pin = {
            **source_manifest,
            "sources": [dict(item) for item in source_manifest["sources"]],
        }
        next(
            item for item in wrong_source_pin["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(wrong_source_pin)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)


    def test_s3_34cd_rcx_index_no_base_sib_scaled_disp32_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34cd_ir_rcx_index_no_base_sib_scaled_disp32_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rcx_index_no_base_sib_scaled_disp32_store_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34cd_host_rcx_index_no_base_sib_disp32_dword_store_full_width_scaled_index", "local_acceptance",
                "od-host",
                "tests::s3_20es_rcx_index_no_base_sib_scaled_disp32_dword_store_uses_full_width_scaled_index",
            ),
        )
        self.assertIn(
            "test_s3_34cd_rcx_index_no_base_sib_scaled_disp32_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34cd_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34cd_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        pinned_jit_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        self.assertEqual(
            pinned_jit_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-jit/src/memory_load_v2.rs"),
        )
        pinned_host_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )
        self.assertEqual(
            pinned_host_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-host/src/lib.rs"),
        )
        wrong_source_pin = {
            **source_manifest,
            "sources": [dict(item) for item in source_manifest["sources"]],
        }
        next(
            item for item in wrong_source_pin["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(wrong_source_pin)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)


    def test_s3_34ce_addr32_rcx_index_no_base_sib_disp32_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34ce_ir_addr32_rcx_index_no_base_sib_disp32_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_rcx_index_no_base_sib_disp32_load_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34ce_host_addr32_rcx_index_no_base_sib_disp32_dword_load_wrapped_ecx", "local_acceptance",
                "od-host",
                "tests::s3_20et_addr32_rcx_index_no_base_sib_disp32_dword_load_uses_wrapped_ecx",
            ),
        )
        self.assertIn(
            "test_s3_34ce_addr32_rcx_index_no_base_sib_disp32_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34ce_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34ce_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        pinned_jit_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        self.assertEqual(
            pinned_jit_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-jit/src/memory_load_v2.rs"),
        )
        pinned_host_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )
        self.assertEqual(
            pinned_host_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-host/src/lib.rs"),
        )
        wrong_source_pin = {
            **source_manifest,
            "sources": [dict(item) for item in source_manifest["sources"]],
        }
        next(
            item for item in wrong_source_pin["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(wrong_source_pin)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)


    def test_s3_34cf_addr32_rcx_index_no_base_sib_disp32_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34cf_ir_addr32_rcx_index_no_base_sib_disp32_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_rcx_index_no_base_sib_disp32_store_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34cf_host_addr32_rcx_index_no_base_sib_disp32_dword_store_wrapped_ecx", "local_acceptance",
                "od-host",
                "tests::s3_20eu_addr32_rcx_index_no_base_sib_disp32_dword_store_uses_wrapped_ecx",
            ),
        )
        self.assertIn(
            "test_s3_34cf_addr32_rcx_index_no_base_sib_disp32_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34cf_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34cf_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        pinned_jit_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        self.assertEqual(
            pinned_jit_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-jit/src/memory_load_v2.rs"),
        )
        pinned_host_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )
        self.assertEqual(
            pinned_host_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-host/src/lib.rs"),
        )
        wrong_source_pin = {
            **source_manifest,
            "sources": [dict(item) for item in source_manifest["sources"]],
        }
        next(
            item for item in wrong_source_pin["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(wrong_source_pin)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)


    def test_s3_34cg_addr32_rcx_index_no_base_sib_scaled_disp32_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34cg_ir_addr32_rcx_index_no_base_sib_scaled_disp32_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_rcx_index_no_base_sib_scaled_disp32_load_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34cg_host_addr32_rcx_index_no_base_sib_scaled_disp32_dword_load_wrapped_scaled_ecx", "local_acceptance",
                "od-host",
                "tests::s3_20ev_addr32_rcx_index_no_base_sib_scaled_disp32_dword_load_uses_wrapped_scaled_ecx",
            ),
        )
        self.assertIn(
            "test_s3_34cg_addr32_rcx_index_no_base_sib_scaled_disp32_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34cg_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34cg_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        pinned_jit_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-jit/src/memory_load_v2.rs"
        )
        self.assertEqual(
            pinned_jit_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-jit/src/memory_load_v2.rs"),
        )
        pinned_host_source = next(
            item for item in source_manifest["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )
        self.assertEqual(
            pinned_host_source["sha256"],
            harness.sha256_file(harness.ROOT / "crates/od-host/src/lib.rs"),
        )
        wrong_source_pin = {
            **source_manifest,
            "sources": [dict(item) for item in source_manifest["sources"]],
        }
        next(
            item for item in wrong_source_pin["sources"]
            if item["path"] == "crates/od-host/src/lib.rs"
        )["sha256"] = "f" * 64
        with self.assertRaisesRegex(harness.HarnessError, "source pin differs"):
            harness.validate_rust_test_manifest(wrong_source_pin)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

            run_id = harness.new_run_id()
            summary_run_root = harness.ROOT / "work" / "stage3-harness" / run_id
            (summary_run_root / "logs").mkdir(parents=True)
            try:
                summary = passing_final_summary(run_id, summary_run_root / "logs")
                summary["suites"]["local_acceptance"]["tests"].remove(expected[1][0])
                input_manifest_path = temporary_dir / "input_manifest.json"
                manifest_raw = harness.write_json_create(input_manifest_path, harness.input_manifest())
                summary["input_manifest_sha256"] = harness.sha256_bytes(manifest_raw)
                with mock.patch.object(harness, "source_fingerprint", return_value="a" * 64):
                    with (
                        mock.patch.object(harness, "validate_local_vmhost_result_state"),
                        mock.patch.object(harness, "validate_native_register_v1_artifacts"),
                    ):
                        with self.assertRaisesRegex(harness.HarnessError, "suite membership"):
                            harness.validate_final_summary(summary, input_manifest_path)
            finally:
                shutil.rmtree(summary_run_root, ignore_errors=True)

    def test_s3_34ch_addr32_rcx_index_no_base_sib_scaled_disp32_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34ch_ir_addr32_rcx_index_no_base_sib_scaled_disp32_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_rcx_index_no_base_sib_scaled_disp32_store_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34ch_host_addr32_rcx_index_no_base_sib_scaled_disp32_dword_store_wrapped_scaled_ecx", "local_acceptance",
                "od-host",
                "tests::s3_20ew_addr32_rcx_index_no_base_sib_scaled_disp32_dword_store_uses_wrapped_scaled_ecx",
            ),
        )
        self.assertIn(
            "test_s3_34ch_addr32_rcx_index_no_base_sib_scaled_disp32_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34ch_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34ch_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34ci_rax_index_no_base_sib_disp32_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34ci_ir_rax_index_no_base_sib_disp32_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rax_index_no_base_sib_disp32_load_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34ci_host_rax_index_no_base_sib_disp32_dword_load_original_rax", "local_acceptance",
                "od-host",
                "tests::s3_20ex_rax_index_no_base_sib_disp32_dword_load_uses_original_rax",
            ),
        )
        self.assertIn(
            "test_s3_34ci_rax_index_no_base_sib_disp32_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34ci_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34ci_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, identity in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)



    def test_s3_34cj_rax_index_no_base_sib_disp32_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34cj_ir_rax_index_no_base_sib_disp32_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rax_index_no_base_sib_disp32_store_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34cj_host_rax_index_no_base_sib_disp32_dword_store_rax_address_and_data", "local_acceptance",
                "od-host",
                "tests::s3_20ey_rax_index_no_base_sib_disp32_dword_store_uses_full_rax_address_and_low_dword_data",
            ),
        )
        self.assertIn(
            "test_s3_34cj_rax_index_no_base_sib_disp32_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34cj_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34cj_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, identity in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)



    def test_s3_34ck_rax_index_no_base_sib_scaled_disp32_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34ck_ir_rax_index_no_base_sib_scaled_disp32_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rax_index_no_base_sib_scaled_disp32_load_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34ck_host_rax_index_no_base_sib_scaled_disp32_dword_load_original_rax", "local_acceptance",
                "od-host",
                "tests::s3_20ez_rax_index_no_base_sib_scaled_disp32_dword_load_uses_original_scaled_rax",
            ),
        )
        self.assertIn(
            "test_s3_34ck_rax_index_no_base_sib_scaled_disp32_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34ck_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34ck_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, identity in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)



    def test_s3_34cl_rax_index_no_base_sib_scaled_disp32_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34cl_ir_rax_index_no_base_sib_scaled_disp32_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rax_index_no_base_sib_scaled_disp32_store_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34cl_host_rax_index_no_base_sib_scaled_disp32_dword_store_rax_address_and_data", "local_acceptance",
                "od-host",
                "tests::s3_20fa_rax_index_no_base_sib_scaled_disp32_dword_store_uses_scaled_rax_address_and_low_dword_data",
            ),
        )
        self.assertIn(
            "test_s3_34cl_rax_index_no_base_sib_scaled_disp32_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34cl_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34cl_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, identity in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)


    def test_s3_34cm_addr32_rax_index_no_base_sib_disp32_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34cm_ir_addr32_rax_index_no_base_sib_disp32_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_rax_index_no_base_sib_disp32_load_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34cm_host_addr32_rax_index_no_base_sib_disp32_dword_load_original_eax", "local_acceptance",
                "od-host",
                "tests::s3_20fb_addr32_rax_index_no_base_sib_disp32_dword_load_uses_original_wrapped_eax",
            ),
        )
        self.assertIn(
            "test_s3_34cm_addr32_rax_index_no_base_sib_disp32_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34cm_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34cm_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, identity in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34cn_addr32_rax_index_no_base_sib_disp32_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34cn_ir_addr32_rax_index_no_base_sib_disp32_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_rax_index_no_base_sib_disp32_store_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34cn_host_addr32_rax_index_no_base_sib_disp32_dword_store_eax_address_and_data", "local_acceptance",
                "od-host",
                "tests::s3_20fc_addr32_rax_index_no_base_sib_disp32_dword_store_uses_wrapped_eax_address_and_low_dword_data",
            ),
        )
        self.assertIn(
            "test_s3_34cn_addr32_rax_index_no_base_sib_disp32_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34cn_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34cn_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, identity in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34co_addr32_rax_index_no_base_sib_scaled_disp32_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34co_ir_addr32_rax_index_no_base_sib_scaled_disp32_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_rax_index_no_base_sib_scaled_disp32_load_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34co_host_addr32_rax_index_no_base_sib_scaled_disp32_dword_load_original_eax", "local_acceptance",
                "od-host",
                "tests::s3_20fd_addr32_rax_index_no_base_sib_scaled_disp32_dword_load_uses_original_wrapped_scaled_eax",
            ),
        )
        self.assertIn(
            "test_s3_34co_addr32_rax_index_no_base_sib_scaled_disp32_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34co_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34co_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, identity in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34cp_addr32_rax_index_no_base_sib_scaled_disp32_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34cp_ir_addr32_rax_index_no_base_sib_scaled_disp32_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_rax_index_no_base_sib_scaled_disp32_store_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34cp_host_addr32_rax_index_no_base_sib_scaled_disp32_dword_store_eax_address_and_data", "local_acceptance",
                "od-host",
                "tests::s3_20fe_addr32_rax_index_no_base_sib_scaled_disp32_dword_store_uses_wrapped_scaled_eax_address_and_low_dword_data",
            ),
        )
        self.assertIn(
            "test_s3_34cp_addr32_rax_index_no_base_sib_scaled_disp32_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34cp_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34cp_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, identity in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34cq_rax_disp8_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34cq_ir_rax_disp8_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rax_disp8_load_binds_all_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34cq_host_rax_disp8_dword_load_original_rax", "local_acceptance",
                "od-host",
                "tests::s3_20ff_rax_disp8_dword_load_uses_original_rax",
            ),
        )
        self.assertIn(
            "test_s3_34cq_rax_disp8_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34cq_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34cq_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, identity in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34cr_rax_disp8_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34cr_ir_rax_disp8_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rax_disp8_store_binds_all_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34cr_host_rax_disp8_dword_store_rax_address_and_data", "local_acceptance",
                "od-host",
                "tests::s3_20fg_rax_disp8_dword_store_uses_full_rax_address_and_low_dword_data",
            ),
        )
        self.assertIn(
            "test_s3_34cr_rax_disp8_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34cr_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34cr_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, identity in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

    def test_s3_34cs_rax_disp32_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34cs_ir_rax_disp32_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rax_disp32_load_binds_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34cs_host_rax_disp32_dword_load_original_rax", "local_acceptance",
                "od-host",
                "tests::s3_20fh_rax_disp32_dword_load_uses_original_full_rax",
            ),
        )
        self.assertIn(
            "test_s3_34cs_rax_disp32_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34cs_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34cs_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, identity in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(step=step, evidence="forged argv"), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)

    def test_s3_34dj_addr32_rcx_disp8_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34dj_ir_addr32_rcx_disp8_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_rcx_disp8_store_binds_all_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34dj_host_addr32_rcx_disp8_dword_store_wrapped_ecx_and_low_eax_payload",
                "local_acceptance", "od-host",
                "tests::s3_20fy_addr32_rcx_disp8_dword_store_uses_wrapped_ecx_and_low_eax_payload",
            ),
        )
        self.assertIn(
            "test_s3_34dj_addr32_rcx_disp8_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34dj_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34dj_")],
            list(expected),
        )

        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(
                    harness.rust_test_argv(package, identity),
                    [
                        "cargo", "test", "--offline", "-p", package, "--lib", identity,
                        "--", "--exact", "--test-threads=1",
                    ],
                )

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(step=step, evidence="forged argv"), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)

    def test_s3_34dk_addr32_rcx_disp32_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34dk_ir_addr32_rcx_disp32_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_rcx_disp32_load_binds_exact_source_and_site",
            ),
            (
                "s3_34dk_host_addr32_rcx_disp32_dword_load_wrapped_ecx", "local_acceptance",
                "od-host",
                "tests::s3_20fz_addr32_rcx_disp32_dword_load_uses_wrapped_ecx",
            ),
        )
        self.assertIn(
            "test_s3_34dk_addr32_rcx_disp32_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34dk_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34dk_")],
            list(expected),
        )

        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(
                    harness.rust_test_argv(package, identity),
                    [
                        "cargo", "test", "--offline", "-p", package, "--lib", identity,
                        "--", "--exact", "--test-threads=1",
                    ],
                )

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(step=step, evidence="forged argv"), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)


    def test_s3_34dl_addr32_rcx_disp32_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34dl_ir_addr32_rcx_disp32_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_rcx_disp32_store_binds_exact_source_and_site",
            ),
            (
                "s3_34dl_host_addr32_rcx_disp32_dword_store_wrapped_ecx_and_low_eax_payload",
                "local_acceptance", "od-host",
                "tests::s3_20ga_addr32_rcx_disp32_dword_store_uses_wrapped_ecx_and_low_eax_payload",
            ),
        )
        self.assertIn(
            "test_s3_34dl_addr32_rcx_disp32_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34dl_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34dl_")],
            list(expected),
        )

        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(
                    harness.rust_test_argv(package, identity),
                    [
                        "cargo", "test", "--offline", "-p", package, "--lib", identity,
                        "--", "--exact", "--test-threads=1",
                    ],
                )

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(step=step, evidence="forged argv"), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)

    def test_s3_34dd_rcx_disp8_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34dd_ir_rcx_disp8_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rcx_disp8_store_binds_all_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34dd_host_rcx_disp8_dword_store_full_rcx_and_low_eax_payload",
                "local_acceptance", "od-host",
                "tests::s3_20fs_rcx_disp8_dword_store_uses_full_rcx_and_low_eax_payload",
            ),
        )
        self.assertIn(
            "test_s3_34dd_rcx_disp8_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34dd_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34dd_")],
            list(expected),
        )

        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(step=step, evidence="forged argv"), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)

    def test_s3_34db_rcx_base_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34db_ir_rcx_base_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rcx_base_store_binds_exact_source_and_site",
            ),
            (
                "s3_34db_host_rcx_base_dword_store_full_rcx_and_low_eax_payload",
                "local_acceptance", "od-host",
                "tests::s3_20fq_rcx_base_dword_store_uses_full_rcx_and_low_eax_payload",
            ),
        )
        self.assertIn(
            "test_s3_34db_rcx_base_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34db_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34db_")],
            list(expected),
        )

        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(step=step, evidence="forged argv"), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)


    def test_s3_34ct_rax_disp32_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34ct_ir_rax_disp32_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rax_disp32_store_binds_all_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34ct_host_rax_disp32_dword_store_rax_address_and_data", "local_acceptance",
                "od-host",
                "tests::s3_20fi_rax_disp32_dword_store_uses_full_rax_address_and_low_dword_data",
            ),
        )
        self.assertIn(
            "test_s3_34ct_rax_disp32_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34ct_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34ct_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, identity in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(step=step, evidence="forged argv"), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)

    def test_s3_34cu_addr32_alias_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34cu_ir_addr32_alias_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_alias_load_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34cu_host_addr32_alias_dword_load_original_eax", "local_acceptance",
                "od-host",
                "tests::s3_20fj_addr32_alias_dword_load_uses_original_eax",
            ),
        )
        self.assertIn(
            "test_s3_34cu_addr32_alias_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34cu_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34cu_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    ("zero matches", "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n"),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(step=step, evidence="forged argv"), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)

    def test_s3_34cv_addr32_alias_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34cv_ir_addr32_alias_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_alias_store_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34cv_host_addr32_alias_dword_store_original_eax_address_and_payload",
                "local_acceptance", "od-host",
                "tests::s3_20fk_addr32_alias_dword_store_uses_original_eax_address_and_payload",
            ),
        )
        self.assertIn(
            "test_s3_34cv_addr32_alias_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34cv_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34cv_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(step=step, evidence="forged argv"), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)


    def test_s3_34cw_addr32_alias_disp8_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34cw_ir_addr32_alias_disp8_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_alias_disp8_load_binds_all_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34cw_host_addr32_alias_disp8_dword_load_wrapped_original_eax",
                "local_acceptance", "od-host",
                "tests::s3_20fl_addr32_alias_disp8_dword_load_uses_wrapped_original_eax",
            ),
        )
        self.assertIn(
            "test_s3_34cw_addr32_alias_disp8_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34cw_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34cw_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(step=step, evidence="forged argv"), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)

    def test_s3_34cx_addr32_alias_disp8_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34cx_ir_addr32_alias_disp8_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_alias_disp8_store_binds_all_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34cx_host_addr32_alias_disp8_dword_store_wrapped_original_eax_address_and_payload",
                "local_acceptance", "od-host",
                "tests::s3_20fm_addr32_disp8_dword_store_uses_wrapped_original_eax_address_and_payload",
            ),
        )
        self.assertIn(
            "test_s3_34cx_addr32_alias_disp8_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34cx_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34cx_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(step=step, evidence="forged argv"), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)

    def test_s3_34cy_addr32_alias_disp32_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34cy_ir_addr32_alias_disp32_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_alias_disp32_load_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34cy_host_addr32_alias_disp32_dword_load_wrapped_original_eax",
                "local_acceptance", "od-host",
                "tests::s3_20fn_addr32_alias_disp32_dword_load_uses_wrapped_original_eax",
            ),
        )
        self.assertIn(
            "test_s3_34cy_addr32_alias_disp32_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34cy_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34cy_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(step=step, evidence="forged argv"), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)
    def test_s3_34cz_addr32_alias_disp32_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34cz_ir_addr32_alias_disp32_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_alias_disp32_store_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34cz_host_addr32_alias_disp32_dword_store_wrapped_original_eax_address_and_payload",
                "local_acceptance", "od-host",
                "tests::s3_20fo_addr32_alias_disp32_dword_store_uses_wrapped_original_eax_address_and_payload",
            ),
        )
        self.assertIn(
            "test_s3_34cz_addr32_alias_disp32_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34cz_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34cz_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(step=step, evidence="forged argv"), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)

    def test_s3_34da_rcx_base_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34da_ir_rcx_base_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rcx_base_load_binds_exact_source_and_site",
            ),
            (
                "s3_34da_host_rcx_base_dword_load_full_rcx_and_zero_extended_eax",
                "local_acceptance", "od-host",
                "tests::s3_20fp_rcx_base_dword_load_uses_full_rcx_and_zero_extends_eax",
            ),
        )
        self.assertIn(
            "test_s3_34da_rcx_base_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34da_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34da_")],
            list(expected),
        )
        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(step=step, evidence="forged argv"), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)


    def test_s3_34dc_rcx_disp8_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34dc_ir_rcx_disp8_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rcx_disp8_load_binds_all_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34dc_host_rcx_disp8_dword_load_full_rcx_and_signed_displacement",
                "local_acceptance", "od-host",
                "tests::s3_20fr_rcx_disp8_dword_load_uses_full_rcx_and_signed_displacement",
            ),
        )
        self.assertIn(
            "test_s3_34dc_rcx_disp8_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34dc_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34dc_")],
            list(expected),
        )

        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(step=step, evidence="forged argv"), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)



    def test_s3_34de_rcx_disp32_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34de_ir_rcx_disp32_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rcx_disp32_load_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34de_host_rcx_disp32_dword_load_full_rcx_and_signed_displacement",
                "local_acceptance", "od-host",
                "tests::s3_20ft_rcx_disp32_dword_load_uses_full_rcx_and_signed_displacement",
            ),
        )
        self.assertIn(
            "test_s3_34de_rcx_disp32_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34de_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34de_")],
            list(expected),
        )

        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(step=step, evidence="forged argv"), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)


    def test_s3_34df_rcx_disp32_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34df_ir_rcx_disp32_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_rcx_disp32_store_binds_all_source_bytes_to_its_site",
            ),
            (
                "s3_34df_host_rcx_disp32_dword_store_full_rcx_and_low_eax_payload",
                "local_acceptance", "od-host",
                "tests::s3_20fu_rcx_disp32_dword_store_uses_full_rcx_and_low_eax_payload",
            ),
        )
        self.assertIn(
            "test_s3_34df_rcx_disp32_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34df_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34df_")],
            list(expected),
        )

        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(harness.rust_test_argv(package, identity), [
                    "cargo", "test", "--offline", "-p", package, "--lib", identity,
                    "--", "--exact", "--test-threads=1",
                ])

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(step=step, evidence="forged argv"), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)

    def test_s3_34dg_addr32_rcx_base_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34dg_ir_addr32_rcx_base_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_rcx_base_load_binds_exact_source_and_site",
            ),
            (
                "s3_34dg_host_addr32_rcx_base_dword_load_zero_extended_ecx",
                "local_acceptance", "od-host",
                "tests::s3_20fv_addr32_rcx_base_dword_load_uses_zero_extended_ecx",
            ),
        )
        self.assertIn(
            "test_s3_34dg_addr32_rcx_base_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34dg_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34dg_")],
            list(expected),
        )

        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(
                    harness.rust_test_argv(package, identity),
                    [
                        "cargo", "test", "--offline", "-p", package, "--lib", identity,
                        "--", "--exact", "--test-threads=1",
                    ],
                )

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(step=step, evidence="forged argv"), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)


    def test_s3_34dh_addr32_rcx_base_dword_store_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34dh_ir_addr32_rcx_base_dword_store_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_rcx_base_store_binds_exact_source_and_site",
            ),
            (
                "s3_34dh_host_addr32_rcx_base_dword_store_zero_extended_ecx_and_low_eax_payload",
                "local_acceptance", "od-host",
                "tests::s3_20fw_addr32_rcx_base_dword_store_uses_zero_extended_ecx_and_low_eax_payload",
            ),
        )
        self.assertIn(
            "test_s3_34dh_addr32_rcx_base_dword_store_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34dh_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34dh_")],
            list(expected),
        )

        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(
                    harness.rust_test_argv(package, identity),
                    [
                        "cargo", "test", "--offline", "-p", package, "--lib", identity,
                        "--", "--exact", "--test-threads=1",
                    ],
                )

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(step=step, evidence="forged argv"), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)

    def test_s3_34di_addr32_rcx_disp8_dword_load_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34di_ir_addr32_rcx_disp8_dword_load_source_binding", "local_acceptance",
                "od-jit",
                "memory_load_v2::tests::ir_dword_addr32_rcx_disp8_load_binds_all_signed_source_bytes_to_its_site",
            ),
            (
                "s3_34di_host_addr32_rcx_disp8_dword_load_wrapped_ecx",
                "local_acceptance", "od-host",
                "tests::s3_20fx_addr32_rcx_disp8_dword_load_uses_wrapped_ecx",
            ),
        )
        self.assertIn(
            "test_s3_34di_addr32_rcx_disp8_dword_load_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34di_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34di_")],
            list(expected),
        )

        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(
                    harness.rust_test_argv(package, identity),
                    [
                        "cargo", "test", "--offline", "-p", package, "--lib", identity,
                        "--", "--exact", "--test-threads=1",
                    ],
                )

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        harness.validate_rust_test_manifest(source_manifest)
        for step, _, _, _ in expected:
            missing_manifest_gate = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(step=step, evidence="missing manifest gate"), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            temporary_dir = Path(temporary)
            run_root = temporary_dir / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)
            for step, _, _, identity in expected:
                missing_evidence = passing_rust_gate_steps(log_dir)
                del missing_evidence[step]
                with self.subTest(step=step, evidence="missing summary gate"), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing_evidence, run_root)

                for label, output in (
                    (
                        "zero matches",
                        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                    ),
                    ("ignored", f"test {identity} ... ignored\n"),
                    ("failed", f"test {identity} ... FAILED\n"),
                    ("wrong identity", "test tests::unrelated_test ... ok\n"),
                ):
                    changed = passing_rust_gate_steps(log_dir)
                    replace_step_log_output(changed[step], output)
                    with self.subTest(step=step, evidence=label), self.assertRaisesRegex(
                        harness.HarnessError, step
                    ):
                        harness.validate_required_rust_test_evidence(changed, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(step=step, evidence="forged argv"), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)

    def test_s3_34dm_addr32_rsp_dword_store_corpus_gate_is_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34dm_addr32_rsp_dword_store_fixed_seed_corpus", "local_acceptance",
                "od-host",
                "fixed_seed_addr32_rsp_dword_store_corpus_matches_interpreter_twice_per_input",
            ),
        )
        self.assertIn(
            "test_s3_34dm_addr32_rsp_dword_store_corpus_gate_is_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34dm_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34dm_")],
            list(expected),
        )

        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(
                    harness.rust_test_argv(package, identity),
                    [
                        "cargo", "test", "--offline", "-p", "od-host", "--test",
                        "stage3_dword_store_corpus", identity, "--", "--exact",
                        "--test-threads=1",
                    ],
                )

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        corpus_source = "tests/cpu/stage3_dword_store_corpus.rs"
        self.assertEqual(
            [source["path"] for source in source_manifest["sources"]
             if source["path"] == corpus_source],
            [corpus_source],
        )
        self.assertEqual(
            next(source for source in source_manifest["sources"] if source["path"] == corpus_source),
            {
                "path": corpus_source,
                "sha256": harness.sha256_file(harness.ROOT / corpus_source),
            },
        )
        harness.validate_rust_test_manifest(source_manifest)
        step = expected[0][0]
        missing_manifest_gate = {
            **source_manifest,
            "gates": [gate for gate in manifest if gate["step"] != step],
        }
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            missing_evidence = passing_rust_gate_steps(log_dir)
            del missing_evidence[step]
            with self.assertRaisesRegex(harness.HarnessError, step):
                harness.validate_required_rust_test_evidence(missing_evidence, run_root)

            _, _, _, identity = expected[0]
            for label, output in (
                (
                    "zero matches",
                    "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                ),
                ("ignored", f"test {identity} ... ignored\n"),
                ("failed", f"test {identity} ... FAILED\n"),
            ):
                changed = passing_rust_gate_steps(log_dir)
                replace_step_log_output(changed[step], output)
                with self.subTest(evidence=label), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(changed, run_root)

            forged_argv = passing_rust_gate_steps(log_dir)
            forged_argv[step]["argv"] = ["cargo", "test"]
            with self.assertRaisesRegex(harness.HarnessError, "invocation is not exact"):
                harness.validate_required_rust_test_evidence(forged_argv, run_root)

    def test_s3_34do_value_only_reducer_self_check_gate_is_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34do_value_only_reducer_self_check", "local_acceptance", "od-host",
                "s3_34do_value_only_reducer_self_checks_are_bounded_and_preserve_signature",
            ),
        )
        self.assertIn(
            "test_s3_34do_value_only_reducer_self_check_gate_is_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(len(harness.SELF_TEST_NAMES), EXPECTED_SELF_TESTS)
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34do_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34do_")],
            list(expected),
        )

        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    harness.RUST_TEST_TARGET_OVERRIDES.get((package, identity)),
                    ("--test", "stage3_dword_store_corpus"),
                )
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(
                    harness.rust_test_argv(package, identity),
                    [
                        "cargo", "test", "--offline", "-p", "od-host", "--test",
                        "stage3_dword_store_corpus", identity, "--", "--exact",
                        "--test-threads=1",
                    ],
                )

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        corpus_source = "tests/cpu/stage3_dword_store_corpus.rs"
        matching_sources = [
            source for source in source_manifest["sources"] if source["path"] == corpus_source
        ]
        self.assertEqual(
            matching_sources,
            [{"path": corpus_source, "sha256": harness.sha256_file(harness.ROOT / corpus_source)}],
        )
        harness.validate_rust_test_manifest(source_manifest)
        step = expected[0][0]
        missing_manifest_gate = {
            **source_manifest,
            "gates": [gate for gate in manifest if gate["step"] != step],
        }
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            missing_evidence = passing_rust_gate_steps(log_dir)
            del missing_evidence[step]
            with self.assertRaisesRegex(harness.HarnessError, step):
                harness.validate_required_rust_test_evidence(missing_evidence, run_root)

            _, _, _, identity = expected[0]
            for label, output in (
                (
                    "zero matches",
                    "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                ),
                ("ignored", f"test {identity} ... ignored\n"),
                ("failed", f"test {identity} ... FAILED\n"),
            ):
                changed = passing_rust_gate_steps(log_dir)
                replace_step_log_output(changed[step], output)
                with self.subTest(evidence=label), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(changed, run_root)

            forged_argv = passing_rust_gate_steps(log_dir)
            forged_argv[step]["argv"] = ["cargo", "test"]
            with self.assertRaisesRegex(harness.HarnessError, "invocation is not exact"):
                harness.validate_required_rust_test_evidence(forged_argv, run_root)

    def test_s3_34dn_replay_artifact_self_check_gate_is_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34dn_replay_artifact_self_check", "local_acceptance", "od-host",
                "s3_34dn_replay_artifact_self_checks_roundtrip_and_fail_closed",
            ),
        )
        self.assertIn(
            "test_s3_34dn_replay_artifact_self_check_gate_is_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(len(harness.SELF_TEST_NAMES), EXPECTED_SELF_TESTS)
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34dn_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34dn_")],
            list(expected),
        )

        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    harness.RUST_TEST_TARGET_OVERRIDES.get((package, identity)),
                    ("--test", "stage3_dword_store_corpus"),
                )
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(
                    harness.rust_test_argv(package, identity),
                    [
                        "cargo", "test", "--offline", "-p", "od-host", "--test",
                        "stage3_dword_store_corpus", identity, "--", "--exact",
                        "--test-threads=1",
                    ],
                )

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        corpus_source = "tests/cpu/stage3_dword_store_corpus.rs"
        matching_sources = [
            source for source in source_manifest["sources"] if source["path"] == corpus_source
        ]
        self.assertEqual(
            matching_sources,
            [{"path": corpus_source, "sha256": harness.sha256_file(harness.ROOT / corpus_source)}],
        )
        harness.validate_rust_test_manifest(source_manifest)
        step = expected[0][0]
        missing_manifest_gate = {
            **source_manifest,
            "gates": [gate for gate in manifest if gate["step"] != step],
        }
        with self.assertRaisesRegex(harness.HarnessError, "exact required Rust test gates"):
            harness.validate_rust_test_manifest(missing_manifest_gate)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid_steps = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid_steps, run_root)

            missing_evidence = passing_rust_gate_steps(log_dir)
            del missing_evidence[step]
            with self.assertRaisesRegex(harness.HarnessError, step):
                harness.validate_required_rust_test_evidence(missing_evidence, run_root)

            _, _, _, identity = expected[0]
            for label, output in (
                (
                    "zero matches",
                    "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                ),
                ("ignored", f"test {identity} ... ignored\n"),
                ("failed", f"test {identity} ... FAILED\n"),
            ):
                changed = passing_rust_gate_steps(log_dir)
                replace_step_log_output(changed[step], output)
                with self.subTest(evidence=label), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(changed, run_root)

            forged_argv = passing_rust_gate_steps(log_dir)
            forged_argv[step]["argv"] = ["cargo", "test"]
            with self.assertRaisesRegex(harness.HarnessError, "invocation is not exact"):
                harness.validate_required_rust_test_evidence(forged_argv, run_root)


    def test_s3_16_not_rcx_fallback_gate_is_pinned_and_fail_closed(self):
        expected = [
            (
                "s3_16_not_rcx_fallback", "local_acceptance", "od-jit",
                "memory_dispatch::tests::not_rcx_fallback_is_exact_and_strict_run_stays_strict",
            ),
            (
                "s3_16_nop_u64_max_regression", "local_acceptance", "od-jit",
                "memory_dispatch::tests::nop_fallback_handles_code_end_at_u64_max",
            ),
            (
                "s3_16_not_rcx_host_continuation", "local_acceptance", "od-host",
                "tests::bounded_memory_dispatch_not_rcx_fallback_composes_native_prefix_and_suffix",
            ),
        ]
        for gate in expected:
            self.assertIn(gate, harness.RUST_TEST_GATES)
            self.assertEqual(harness.RUST_TEST_GATES.count(gate), 1)
            self.assertEqual(harness.LOCAL_STEPS.count(gate[0]), 1)
            self.assertEqual(
                [item for item in harness.rust_test_gate_manifest() if item["step"] == gate[0]],
                [{"step": gate[0], "suite": gate[1], "package": gate[2], "identity": gate[3]}],
            )
            command = ["cargo", "test", "--offline", "-p", gate[2]]
            command.extend(["--lib"] if gate[2] in {"od-jit", "od-host"} else [])
            command.extend([gate[3], "--", "--exact", "--test-threads=1"])
            self.assertEqual(harness.rust_test_argv(gate[2], gate[3]), command)
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.rust_test_gate_manifest()), EXPECTED_RUST_GATES)

        source_manifest = {
            "gates": harness.rust_test_gate_manifest(),
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        for step, _, _, _ in expected:
            missing_gate_manifest = {
                **source_manifest,
                "gates": [gate for gate in source_manifest["gates"] if gate["step"] != step],
            }
            with self.subTest(missing_step=step), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_gate_manifest)
        harness.validate_rust_test_manifest(source_manifest)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid, run_root)

            for step, _, _, _ in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(missing_evidence=step), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                zero_match = passing_rust_gate_steps(log_dir)
                replace_step_log_output(
                    zero_match[step],
                    "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                )
                with self.subTest(zero_match=step), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(zero_match, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(forged_argv=step), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)

    def test_s3_38f_session_proxy_gates_are_pinned_once_and_fail_closed(self):
        expected = [
            (
                "s3_38f_session_abi_create_preflight", "local_lifecycle", "od-host",
                "session_abi_v1::tests::s3_38f_abi_create_preflight_runs_before_map_jit_lease",
            ),
            (
                "s3_38f_session_proxy_stop_ids_destroy", "local_lifecycle", "od-host",
                "session_abi_v1::tests::s3_38f_proxy_stop_request_ids_and_destroy_busy_are_exact",
            ),
            (
                "s3_38f_session_proxy_fresh_state_lease", "local_lifecycle", "od-host",
                "session_abi_v1::tests::s3_38f_proxy_reuses_code_with_fresh_state_and_exclusive_lease",
            ),
        ]
        for gate in expected:
            self.assertIn(gate, harness.RUST_TEST_GATES)
            self.assertEqual(harness.RUST_TEST_GATES.count(gate), 1)
            self.assertEqual(harness.LOCAL_STEPS.count(gate[0]), 1)
            self.assertEqual(
                [item for item in harness.rust_test_gate_manifest() if item["step"] == gate[0]],
                [{"step": gate[0], "suite": gate[1], "package": gate[2], "identity": gate[3]}],
            )
            self.assertEqual(
                harness.rust_test_argv(gate[2], gate[3]),
                ["cargo", "test", "--offline", "-p", "od-host", "--lib", gate[3], "--", "--exact", "--test-threads=1"],
            )

        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        self.assertIn("crates/od-host/src/session_abi_v1.rs", harness.LOCAL_RUST_TEST_SOURCES)
        for step, _, _, _ in expected:
            missing_manifest = {
                **source_manifest,
                "gates": [gate for gate in manifest if gate["step"] != step],
            }
            with self.subTest(missing_manifest_gate=step), self.assertRaisesRegex(
                harness.HarnessError, "exact required Rust test gates"
            ):
                harness.validate_rust_test_manifest(missing_manifest)
        harness.validate_rust_test_manifest(source_manifest)

        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid, run_root)
            for step, _, _, _ in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(missing_evidence=step), self.assertRaisesRegex(harness.HarnessError, step):
                    harness.validate_required_rust_test_evidence(missing, run_root)
                zero_match = passing_rust_gate_steps(log_dir)
                replace_step_log_output(zero_match[step], "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n")
                with self.subTest(zero_match=step), self.assertRaisesRegex(harness.HarnessError, step):
                    harness.validate_required_rust_test_evidence(zero_match, run_root)
                forged = passing_rust_gate_steps(log_dir)
                forged[step]["argv"] = ["cargo", "test"]
                with self.subTest(forged_argv=step), self.assertRaisesRegex(harness.HarnessError, "invocation is not exact"):
                    harness.validate_required_rust_test_evidence(forged, run_root)

    def test_s3_34dr_load_load_profile_gates_are_pinned_and_fail_closed(self):
        expected = (
            (
                "s3_34dr_load_load_profile_exact_sequences", "local_acceptance", "od-jit",
                "memory_cache::tests::profiles_admit_only_the_exact_load_store_sequences",
            ),
            (
                "s3_34dr_load_load_internal_rip_rejected", "local_acceptance", "od-jit",
                "memory_cache::tests::internal_second_site_rip_remains_rejected_for_load_pair",
            ),
            (
                "s3_34dr_retained_load_load_host_regression", "local_acceptance", "od-host",
                "tests::retained_load_load_pair_uses_fresh_reads_and_continues_after_second_read_fault",
            ),
        )
        self.assertIn(
            "test_s3_34dr_load_load_profile_gates_are_pinned_and_fail_closed",
            harness.SELF_TEST_NAMES,
        )
        self.assertEqual(len(harness.SELF_TEST_NAMES), EXPECTED_SELF_TESTS)
        self.assertEqual(
            [gate for gate in harness.ADDITIONAL_TEST_GATES if gate[0].startswith("s3_34dr_")],
            list(expected),
        )
        self.assertEqual(
            [gate for gate in harness.RUST_TEST_GATES if gate[0].startswith("s3_34dr_")],
            list(expected),
        )

        manifest = harness.rust_test_gate_manifest()
        self.assertEqual(len(harness.RUST_TEST_GATES), EXPECTED_RUST_GATES)
        self.assertEqual(len(manifest), EXPECTED_RUST_GATES)
        self.assertEqual(len({gate["step"] for gate in manifest}), EXPECTED_RUST_GATES)
        self.assertEqual(len(harness.LOCAL_STEPS), EXPECTED_LOCAL_STEPS)
        self.assertEqual(
            sum(gate[1] == "local_acceptance" for gate in harness.RUST_TEST_GATES), 308
        )
        self.assertEqual(
            sum(gate[1] == "local_lifecycle" for gate in harness.RUST_TEST_GATES), 18
        )
        source_path = "crates/od-jit/src/memory_cache.rs"
        self.assertEqual(harness.LOCAL_RUST_TEST_SOURCES.count(source_path), 1)
        self.assertEqual(len(harness.LOCAL_RUST_TEST_SOURCES), 18)

        source_manifest = {
            "gates": manifest,
            "sources": [
                {"path": source, "sha256": harness.sha256_file(harness.ROOT / source)}
                for source in harness.LOCAL_RUST_TEST_SOURCES
            ],
        }
        missing_source = {
            **source_manifest,
            "sources": [item for item in source_manifest["sources"] if item["path"] != source_path],
        }
        with self.assertRaisesRegex(
            harness.HarnessError, "input manifest does not pin the required Rust test sources"
        ):
            harness.validate_rust_test_manifest(missing_source)

        for step, suite, package, identity in expected:
            with self.subTest(step=step):
                self.assertEqual(harness.RUST_TEST_GATES.count((step, suite, package, identity)), 1)
                self.assertEqual(harness.LOCAL_STEPS.count(step), 1)
                self.assertEqual(
                    [gate for gate in manifest if gate["step"] == step],
                    [{"step": step, "suite": suite, "package": package, "identity": identity}],
                )
                self.assertEqual(
                    harness.rust_test_argv(package, identity),
                    [
                        "cargo", "test", "--offline", "-p", package, "--lib", identity,
                        "--", "--exact", "--test-threads=1",
                    ],
                )
                missing_gate = {
                    **source_manifest,
                    "gates": [gate for gate in manifest if gate["step"] != step],
                }
                with self.assertRaisesRegex(
                    harness.HarnessError, "input manifest does not pin the exact required Rust test gates"
                ):
                    harness.validate_rust_test_manifest(missing_gate)

        harness.validate_rust_test_manifest(source_manifest)
        with temporary_fixture_directory() as temporary:
            run_root = Path(temporary) / "run"
            log_dir = run_root / "logs"
            log_dir.mkdir(parents=True)
            valid = passing_rust_gate_steps(log_dir)
            harness.validate_required_rust_test_evidence(valid, run_root)

            for step, _, _, _ in expected:
                missing = passing_rust_gate_steps(log_dir)
                del missing[step]
                with self.subTest(missing_evidence=step), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(missing, run_root)

                zero_match = passing_rust_gate_steps(log_dir)
                replace_step_log_output(
                    zero_match[step],
                    "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
                )
                with self.subTest(zero_match=step), self.assertRaisesRegex(
                    harness.HarnessError, step
                ):
                    harness.validate_required_rust_test_evidence(zero_match, run_root)

                forged_argv = passing_rust_gate_steps(log_dir)
                forged_argv[step]["argv"] = ["cargo", "test"]
                with self.subTest(forged_argv=step), self.assertRaisesRegex(
                    harness.HarnessError, "invocation is not exact"
                ):
                    harness.validate_required_rust_test_evidence(forged_argv, run_root)


if __name__ == "__main__":
    unittest.main(verbosity=2)
