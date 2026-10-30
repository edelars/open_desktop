#!/usr/bin/env python3
"""Run the bounded local Stage 3 correctness suites and preserve their evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shlex
import subprocess
import sys
import time
import uuid
from pathlib import Path

import stage1_oracle

ROOT = Path(__file__).resolve().parents[2]
SCHEMA_VERSION = 3
GENERATED_COUNT = 1000
GENERATED_SEED = "0x5eedcafe"
REPEATS = 2
RUN_ID_PATTERN = re.compile(r"stage3-\d{8}-\d{6}-[0-9a-f]{10}")

MEMORY_REQUIRED_TESTS = {
    "fixed_seed_memory_corpus_matches_interpreter_twice_per_input",
    "named_fault_stop_budget_and_continuation_cases_are_separate",
    "comparator_rejects_corruption_and_serializes_named_diagnostics",
    "bounded_reducer_is_deterministic_preserves_class_and_saves_report",
    "reducer_rejects_malformed_and_unavailable_inputs_and_stops_on_nonreproduction",
    "reducer_rejects_same_mismatch_moving_to_continuation_phase",
    "generic_reducer_can_clear_marker_when_comparator_allows_candidate",
    "mismatch_report_preserves_empty_first_and_mismatching_continuation_phases",
    "replay_envelope_round_trips_and_rejects_malformed_reports",
    "replay_success_fault_stop_budget_and_continuation_for_both_sequences",
    "saved_mismatch_report_replays_and_preserves_original_classes",
    "replay_refuses_to_claim_equivalence_without_native_backend",
}
MEMORY_ALLOWED_IGNORED_TESTS = {
    "replay_saved_case_from_environment",
    "reduce_saved_case_from_environment",
}
SELF_TEST_NAMES = (
    "test_source_fingerprint_mismatch_is_rejected",
    "test_input_manifest_digest_mismatch_is_rejected",
    "test_partial_run_cannot_be_reported_as_local_only",
    "test_unselected_suites_must_remain_not_run",
    "test_required_memory_test_must_be_present_and_non_ignored",
    "test_allowed_environment_replay_tests_do_not_hide_unexpected_ignored_tests",
    "test_self_test_output_validator_rejects_zero_match_and_all_skipped",
    "test_stage1_child_fingerprint_and_both_builds_are_pinned",
    "test_manual_frozen_input_and_result_bind_to_parent_manifest",
    "test_rust_test_manifest_rejects_a_missing_mandatory_gate",
    "test_rust_gate_summary_rejects_an_omitted_mandatory_gate",
    "test_rust_gate_summary_rejects_forged_passed_evidence",
    "test_rust_gate_rejects_zero_match_ignored_and_failed_runs",
    "test_final_summary_rejects_missing_invalid_and_traversal_run_id",
    "test_final_summary_rejects_run_directory_and_logs_symlinks",
    "test_final_summary_rejects_step_log_symlink_and_external_log_path",
)
SELF_TEST_IDENTITIES = {
    f"__main__.Stage3HarnessValidationTests.{name}" for name in SELF_TEST_NAMES
}

LIFECYCLE_TESTS = (
    "arena_cache_a_b_a_churn_reuses_only_safe_slots_at_capacity_one_and_two",
    "bounded_memory_dispatch_nop_fallback_is_exact_and_strict_run_stays_strict",
    "code_region_store_invalidates_cached_prefix_and_tail_before_redispatch",
    "active_code_v3_exits_on_dirty_prefix_and_finishes_unrelated_store_in_one_entry",
    "active_code_v3_invalidates_exact_not_yet_executed_tail_immediate",
    "active_code_v3_guards_faults_aliases_and_committed_stop_are_precise",
    "active_code_v3_slot_survives_capacity_two_invalidation_and_safe_reuse",
    "active_code_v3_architectural_write_fault_matches_interpreter_and_is_atomic",
    "ir_two_load_pair_preserves_callbacks_faults_and_continuation",
)

ADDITIONAL_TEST_GATES = (
    (
        "s3_29e_coordinator", "local_acceptance", "od-host",
        "tests::s3_29e_bounded_arithmetic_load_coordinator",
    ),
    (
        "demand_flags_replacement_spills", "local_acceptance", "od-host",
        "tests::demand_flags_candidate_defers_replaced_writers_and_survives_clobbers_and_spills",
    ),
    (
        "demand_flags_branch_materialization", "local_acceptance", "od-host",
        "tests::demand_flags_candidate_materializes_before_jz_and_jnz_after_preserving_mov",
    ),
    (
        "demand_flags_exit_materialization", "local_acceptance", "od-host",
        "tests::demand_flags_candidate_materializes_on_stop_unsupported_and_helper_failure_exits",
    ),
    (
        "s3_28_event_before_entry", "local_acceptance", "od-jit",
        "dispatch::tests::event_pending_before_entry_preserves_state_budget_cache_and_nop_fallback",
    ),
    (
        "s3_28_stop_priority_after_publication", "local_acceptance", "od-jit",
        "dispatch::tests::cached_publication_boundary_rechecks_stop_priority_before_event",
    ),
    (
        "abi_create_execute_stop_destroy", "local_acceptance", "od-host",
        "tests::c_abi_create_execute_stop_destroy_lifecycle",
    ),
    (
        "abi_stop_before_execute", "local_acceptance", "od-host",
        "tests::c_abi_stop_requested_before_execute_is_honored_once",
    ),
    (
        "abi_dispatch_interpreter_loop_reuse", "local_acceptance", "od-host",
        "tests::c_abi_register_dispatch_matches_interpreter_and_reuses_loop_block",
    ),
    (
        "abi_stop_at_code_end", "local_acceptance", "od-host",
        "tests::c_abi_pre_stop_at_code_end_honors_v0_stop_contract_once",
    ),
    (
        "abi_interleaved_handles", "local_acceptance", "od-host",
        "tests::c_abi_repeated_and_interleaved_handles_keep_state_independent",
    ),
    (
        "abi_busy_invalid_state_output_preservation", "local_acceptance", "od-host",
        "tests::c_abi_reports_live_process_mapping_as_busy",
    ),
)
RUST_TEST_GATES = (
    *(
        (name, "local_lifecycle", "od-host", f"tests::{name}")
        for name in LIFECYCLE_TESTS
    ),
    *ADDITIONAL_TEST_GATES,
)
LOCAL_STEPS = (
    "harness_self_tests",
    "stage1_dispatch",
    "memory_corpus",
    *(step for step, _, _, _ in RUST_TEST_GATES),
)
UNSELECTED_SUITES = ("physical_x86", "mac_mini", "performance", "owner_backed", "stage2")
LOCAL_RUST_TEST_SOURCES = (
    "crates/od-host/src/lib.rs",
    "crates/od-jit/src/dispatch.rs",
    "tests/cpu/stage3_harness.py",
    "tests/cpu/test_stage3_harness.py",
)


class HarnessError(RuntimeError):
    """Raised when required evidence is absent, incomplete, or inconsistent."""


def sha256_bytes(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def sha256_file(path: Path) -> str:
    try:
        return sha256_bytes(path.read_bytes())
    except OSError as error:
        raise HarnessError(f"cannot read required input {path}: {error}") from error


def source_fingerprint(root: Path = ROOT) -> str:
    """Hash every tracked and non-ignored untracked file by path, mode, and bytes."""
    try:
        listing = subprocess.run(
            ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
            cwd=root,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
    except OSError as error:
        raise HarnessError(f"cannot enumerate source tree: {error}") from error
    if listing.returncode != 0:
        detail = listing.stderr.decode(errors="replace")[-2000:]
        raise HarnessError(f"git ls-files failed: {detail}")

    paths = sorted(item for item in listing.stdout.split(b"\0") if item)
    digest = hashlib.sha256()
    for encoded_path in paths:
        relative = Path(os.fsdecode(encoded_path))
        path = root / relative
        try:
            if path.is_symlink():
                payload = os.readlink(path).encode("utf-8", errors="surrogateescape")
                mode = b"symlink"
            else:
                payload = path.read_bytes()
                mode = str(path.stat().st_mode & 0o777).encode("ascii")
        except OSError as error:
            raise HarnessError(f"cannot fingerprint {relative}: {error}") from error
        digest.update(len(encoded_path).to_bytes(8, "big"))
        digest.update(encoded_path)
        digest.update(len(mode).to_bytes(4, "big"))
        digest.update(mode)
        digest.update(len(payload).to_bytes(8, "big"))
        digest.update(payload)
    if not paths:
        raise HarnessError("source tree fingerprint has no input files")
    return digest.hexdigest()


def validate_fingerprint(expected: str, observed: str) -> None:
    if not expected or expected != observed:
        raise HarnessError("source fingerprint changed during the Stage 3 run")


def validate_digest(raw: bytes, expected: str) -> None:
    if sha256_bytes(raw) != expected:
        raise HarnessError("input manifest digest mismatch")


def canonical_json(value: object) -> bytes:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    return (encoded + "\n").encode("utf-8")


def validate_required_steps(
    statuses: dict[str, str], required: tuple[str, ...] = LOCAL_STEPS
) -> None:
    missing = [name for name in required if statuses.get(name) != "passed"]
    if missing:
        raise HarnessError("required local steps incomplete: " + ", ".join(missing))


def rust_test_argv(package: str, identity: str) -> list[str]:
    return [
        "cargo", "test", "--offline", "-p", package, "--lib", identity,
        "--", "--exact", "--test-threads=1",
    ]


def rust_test_gate_manifest() -> list[dict[str, str]]:
    return [
        {
            "step": step,
            "suite": suite,
            "package": package,
            "identity": identity,
        }
        for step, suite, package, identity in RUST_TEST_GATES
    ]


def validate_run_id(value: object) -> Path:
    if not isinstance(value, str) or RUN_ID_PATTERN.fullmatch(value) is None:
        raise HarnessError("Stage 3 run id is missing or invalid")
    harness_root = (ROOT / "work" / "stage3-harness").resolve()
    if ROOT not in harness_root.parents:
        raise HarnessError("Stage 3 harness root escaped repository root")
    requested_run_root = harness_root / value
    run_root = requested_run_root.resolve()
    if (
        run_root != requested_run_root
        or run_root.name != value
        or run_root.parent != harness_root
        or ROOT not in run_root.parents
    ):
        raise HarnessError("Stage 3 run directory escaped the harness root")
    requested_logs_root = run_root / "logs"
    logs_root = requested_logs_root.resolve()
    if (
        logs_root != requested_logs_root
        or logs_root.parent != run_root
        or ROOT not in logs_root.parents
    ):
        raise HarnessError("Stage 3 run logs escaped the harness run directory")
    return run_root


def validate_required_rust_test_evidence(
    steps: dict[str, dict], run_root: Path | None = None
) -> None:
    for step_name, suite, package, identity in RUST_TEST_GATES:
        step = steps.get(step_name)
        if not isinstance(step, dict) or step.get("status") != "passed":
            raise HarnessError(f"required Rust test gate is missing or not passed: {step_name}")
        if (
            step.get("name") != step_name
            or step.get("gate_suite") != suite
            or step.get("package") != package
            or step.get("test_identity") != identity
            or step.get("argv") != rust_test_argv(package, identity)
            or step.get("returncode") != 0
            or step.get("failure") is not None
        ):
            raise HarnessError(f"required Rust test gate invocation is not exact: {step_name}")
        log_rel = step.get("log")
        output_digest = step.get("output_sha256")
        if not isinstance(log_rel, str) or not isinstance(output_digest, str):
            raise HarnessError(f"required Rust test gate log evidence is missing: {step_name}")
        log_lexical = ROOT / log_rel
        log_path = log_lexical.resolve()
        if ROOT not in log_path.parents:
            raise HarnessError(f"required Rust test gate log escaped repository root: {step_name}")
        if run_root is not None:
            expected_log = run_root / "logs" / f"{step_name}.log"
            expected_rel = expected_log.relative_to(ROOT).as_posix()
            if (
                log_rel != expected_rel
                or log_lexical != expected_log
                or log_path != expected_log
            ):
                raise HarnessError(f"required Rust test gate log path differs: {step_name}")
        try:
            raw_output = log_path.read_bytes()
            output = raw_output.decode("utf-8", errors="replace")
        except OSError as error:
            raise HarnessError(f"required Rust test gate log is missing: {step_name}") from error
        if sha256_bytes(raw_output) != output_digest:
            raise HarnessError(f"required Rust test gate log digest differs: {step_name}")
        if step.get("output_tail") != output[-4000:]:
            raise HarnessError(f"required Rust test gate output tail differs from log: {step_name}")
        observed = parse_test_outcomes(output)
        try:
            exact_observed = validate_exact_rust_test_output(output, identity)
        except HarnessError as error:
            raise HarnessError(
                f"required Rust test gate lacks exact passing output: {step_name}: {error}"
            ) from error
        if exact_observed != observed or step.get("test_outcomes") != observed:
            raise HarnessError(f"required Rust test gate lacks exact passing output: {step_name}")


def validate_rust_test_manifest(value: object) -> None:
    if not isinstance(value, dict):
        raise HarnessError("input manifest is missing local Rust test gates")
    if value.get("gates") != rust_test_gate_manifest():
        raise HarnessError("input manifest does not pin the exact required Rust test gates")
    sources = value.get("sources")
    expected_paths = list(LOCAL_RUST_TEST_SOURCES)
    if not isinstance(sources, list) or [item.get("path") for item in sources if isinstance(item, dict)] != expected_paths:
        raise HarnessError("input manifest does not pin the required Rust test sources")
    if len(sources) != len(expected_paths):
        raise HarnessError("input manifest has malformed Rust test source pins")
    for item, relative in zip(sources, expected_paths, strict=True):
        digest = item.get("sha256")
        if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise HarnessError(f"input manifest has an invalid source pin for {relative}")
        source_path = (ROOT / relative).resolve()
        if ROOT not in source_path.parents or sha256_file(source_path) != digest:
            raise HarnessError(f"input manifest source pin differs for {relative}")


def validate_unselected_suites(suites: dict[str, dict]) -> None:
    invalid = [
        name for name in UNSELECTED_SUITES
        if suites.get(name, {}).get("status") != "not_run"
    ]
    if invalid:
        raise HarnessError("unselected suites must remain not_run: " + ", ".join(invalid))


def parse_test_outcomes(output: str) -> dict[str, str]:
    matches = re.findall(
        r"^test ([A-Za-z0-9_:]+) \.\.\. (ok|ignored(?:, .*)?|FAILED)$",
        output,
        re.MULTILINE,
    )
    return {
        name: "ignored" if outcome.startswith("ignored") else outcome
        for name, outcome in matches
    }


def validate_exact_rust_test_output(output: str, identity: str) -> dict[str, str]:
    rows = re.findall(
        r"^test ([A-Za-z0-9_:]+) \.\.\. (ok|ignored(?:, .*)?|FAILED)$",
        output,
        re.MULTILINE,
    )
    if len(rows) != 1 or rows[0][0] != identity or rows[0][1] != "ok":
        raise HarnessError(f"exact Rust test output did not contain one passing identity: {identity}")
    return {identity: "ok"}


def validate_self_test_output(output: str) -> dict[str, str]:
    rows = re.findall(
        r"^(test_[A-Za-z0-9_]+) \(([^)]+)\) \.\.\. (ok|skipped(?:, .*)?|FAIL|ERROR)$",
        output,
        re.MULTILINE,
    )
    identities = [identity for _, identity, _ in rows]
    if len(rows) != len(SELF_TEST_IDENTITIES) or set(identities) != SELF_TEST_IDENTITIES:
        raise HarnessError("harness self-tests did not run the exact required test identities")
    outcomes = {
        identity: "skipped" if status.startswith("skipped") else status
        for _, identity, status in rows
    }
    if any(status != "ok" for status in outcomes.values()):
        raise HarnessError("harness self-tests contain a skipped or failed test")
    result = re.search(r"^Ran (\d+) tests? in .+$", output, re.MULTILINE)
    if result is None or int(result.group(1)) != len(SELF_TEST_IDENTITIES):
        raise HarnessError("harness self-test runner reported a wrong or missing test count")
    if re.search(r"^OK$", output, re.MULTILINE) is None:
        raise HarnessError("harness self-test runner did not report a clean OK result")
    return outcomes


def validate_test_outcomes(
    output: str,
    required: set[str],
    allowed_ignored: set[str] | None = None,
) -> dict[str, str]:
    outcomes = parse_test_outcomes(output)
    allowed_ignored = allowed_ignored or set()
    missing = sorted(name for name in required if outcomes.get(name) != "ok")
    unexpected_ignored = sorted(
        name for name, status in outcomes.items()
        if status == "ignored" and name not in allowed_ignored
    )
    if missing:
        raise HarnessError("required tests missing, ignored, or failed: " + ", ".join(missing))
    if unexpected_ignored:
        raise HarnessError("unexpected ignored tests: " + ", ".join(unexpected_ignored))
    return outcomes


def validate_stage1_manual_bindings(
    inputs: list[dict], fixture_results: list[dict], expected_manual_inputs: list[dict]
) -> None:
    expected: dict[str, str] = {}
    for item in expected_manual_inputs:
        if not isinstance(item, dict):
            raise HarnessError("parent input manifest has a malformed manual fixture")
        case_id = item.get("case_id")
        digest = item.get("sha256")
        if (
            not isinstance(case_id, str) or not case_id
            or not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest)
            or case_id in expected
        ):
            raise HarnessError("parent input manifest has invalid manual fixture identities")
        expected[case_id] = digest

    child_manual = {
        item.get("case_id"): item for item in inputs
        if item.get("seed") is None
    }
    if set(child_manual) != set(expected):
        raise HarnessError("Stage 1 manual inputs do not match the parent input manifest")
    for case_id, expected_digest in expected.items():
        child = child_manual[case_id]
        if child.get("fixture_sha256") != expected_digest:
            raise HarnessError(f"Stage 1 frozen manual input digest differs for {case_id}")
        fixture_path = child.get("fixture_path")
        if not isinstance(fixture_path, str):
            raise HarnessError(f"Stage 1 frozen manual input path is missing for {case_id}")
        matching_results = [row for row in fixture_results if row.get("case_id") == case_id]
        if len(matching_results) != REPEATS:
            raise HarnessError(f"Stage 1 manual result repeats are incomplete for {case_id}")
        if any(
            row.get("fixture_sha256") != expected_digest
            or row.get("fixture_path") != fixture_path
            for row in matching_results
        ):
            raise HarnessError(f"Stage 1 manual result is not bound to parent input {case_id}")


def validate_stage1_fingerprint_and_builds(
    observed_fingerprint: object, expected_fingerprint: object, build: object
) -> None:
    if (
        not isinstance(expected_fingerprint, str)
        or not re.fullmatch(r"[0-9a-f]{64}", expected_fingerprint)
        or observed_fingerprint != expected_fingerprint
    ):
        raise HarnessError("Stage 1 child source fingerprint differs from pinned parent evidence")
    if not isinstance(build, dict):
        raise HarnessError("Stage 1 fixture build evidence is malformed")
    expected_revision = f"sha256:{expected_fingerprint}"
    for name in ("local_interpreter", "local_jit_dispatch"):
        detail = build.get(name)
        if not isinstance(detail, dict) or detail.get("revision") != expected_revision:
            raise HarnessError(f"Stage 1 {name} build revision differs from pinned source")


def validate_stage1_evidence(stdout: str, parent_input_manifest: dict) -> dict:
    lines = stdout.splitlines()
    summary_rel = next(
        (line.partition("=")[2] for line in reversed(lines) if line.startswith("summary_path=")),
        None,
    )
    if not summary_rel:
        raise HarnessError("Stage 1 child summary path is missing")
    summary_path = (ROOT / summary_rel).resolve()
    if ROOT not in summary_path.parents:
        raise HarnessError("Stage 1 child summary escaped repository root")
    try:
        summary = json.loads(summary_path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise HarnessError(f"Stage 1 child summary is missing or malformed: {error}") from error
    if not isinstance(summary, dict):
        raise HarnessError("Stage 1 child summary must be an object")
    suites = summary.get("suites")
    oracle_config = summary.get("oracle")
    if not isinstance(suites, dict) or not isinstance(oracle_config, dict):
        raise HarnessError("Stage 1 child summary is missing suite/oracle evidence")
    oracle = suites.get("oracle")
    local_suite = suites.get("local")
    if not isinstance(oracle, dict) or not isinstance(local_suite, dict):
        raise HarnessError("Stage 1 child summary omitted the oracle suite")
    expected = (
        summary.get("status") == "local_only"
        and summary.get("suite_selection") == "oracle"
        and local_suite.get("status") == "not_run"
        and summary.get("engine") == "jit-dispatch"
        and oracle_config.get("generated_count") == GENERATED_COUNT
        and oracle_config.get("repeats") == REPEATS
        and oracle_config.get("seed") == GENERATED_SEED
        and oracle.get("status") == "local_only"
        and oracle.get("generated_cases") == GENERATED_COUNT
    )
    if not expected:
        raise HarnessError(
            "Stage 1 child summary does not prove the required 1000×2 local dispatcher run"
        )

    manifest_rel = oracle.get("manifest")
    if not isinstance(manifest_rel, str):
        raise HarnessError("Stage 1 oracle manifest link is missing")
    manifest_path = (ROOT / manifest_rel).resolve()
    if ROOT not in manifest_path.parents:
        raise HarnessError("Stage 1 oracle manifest escaped repository root")
    try:
        manifest_raw = manifest_path.read_bytes()
        manifest = json.loads(manifest_raw)
    except (OSError, json.JSONDecodeError) as error:
        raise HarnessError(f"Stage 1 oracle manifest is missing or malformed: {error}") from error
    if not isinstance(manifest, dict):
        raise HarnessError("Stage 1 oracle manifest must be an object")
    inputs = manifest.get("inputs")
    fixture_results = manifest.get("fixtures")
    manual_ids = set()
    for path in stage1_oracle.DEFAULT_CASES:
        try:
            fixture = json.loads((ROOT / path).read_text())
        except (OSError, json.JSONDecodeError) as error:
            raise HarnessError(f"cannot read manual Stage 1 fixture {path}: {error}") from error
        case_id = fixture.get("case_id") if isinstance(fixture, dict) else None
        if not isinstance(case_id, str) or not case_id:
            raise HarnessError(f"manual Stage 1 fixture has no case_id: {path}")
        manual_ids.add(case_id)
    if (
        manifest.get("status") != "local_only"
        or manifest.get("local_comparison_status") != "passed"
        or manifest.get("stage") != "S3-15"
        or manifest.get("engine") != "jit-dispatch"
        or manifest.get("evidence_scope") != "local_only"
        or manifest.get("physical_x86") != "not_run"
        or manifest.get("repeats") != REPEATS
        or not isinstance(inputs, list)
        or not isinstance(fixture_results, list)
    ):
        raise HarnessError("Stage 1 oracle manifest does not prove a successful local-only run")
    if any(not isinstance(item, dict) for item in inputs):
        raise HarnessError("Stage 1 oracle manifest contains a malformed input record")
    if any(not isinstance(item, dict) for item in fixture_results):
        raise HarnessError("Stage 1 oracle manifest contains a malformed result record")
    case_ids = [item.get("case_id") for item in inputs if isinstance(item, dict)]
    generated_inputs = [
        item for item in inputs if isinstance(item, dict) and item.get("seed") is not None
    ]
    actual_manual_ids = {
        item.get("case_id") for item in inputs
        if isinstance(item, dict) and item.get("seed") is None
    }
    if (
        not manual_ids.issubset(actual_manual_ids)
        or len(actual_manual_ids) != len(manual_ids)
        or len(generated_inputs) != GENERATED_COUNT
        or len(inputs) != len(manual_ids) + GENERATED_COUNT
        or len(case_ids) != len(inputs)
        or any(not isinstance(case_id, str) or not case_id for case_id in case_ids)
        or len(set(case_ids)) != len(case_ids)
        or any(
            not isinstance(item.get("fixture_sha256"), str)
            or not re.fullmatch(r"[0-9a-f]{64}", item["fixture_sha256"])
            for item in inputs
            if isinstance(item, dict)
        )
    ):
        raise HarnessError(
            "Stage 1 oracle manifest does not contain all manual and 1000 generated inputs"
        )
    source_fingerprint = manifest.get("source_fingerprint")
    if not isinstance(source_fingerprint, str) or not re.fullmatch(
        r"[0-9a-f]{64}", source_fingerprint
    ):
        raise HarnessError("Stage 1 oracle manifest has no valid source fingerprint")
    stage1_expected = (
        parent_input_manifest.get("stage1_dispatch", {}).get("source_fingerprint")
        if isinstance(parent_input_manifest.get("stage1_dispatch"), dict) else None
    )
    if (
        not isinstance(stage1_expected, str)
        or not re.fullmatch(r"[0-9a-f]{64}", stage1_expected)
        or source_fingerprint != stage1_expected
    ):
        raise HarnessError("Stage 1 child source fingerprint differs from pinned parent evidence")
    for item in inputs:
        fixture_rel = item.get("fixture_path")
        if not isinstance(fixture_rel, str):
            raise HarnessError("Stage 1 oracle manifest input has no fixture path")
        fixture_path = (ROOT / fixture_rel).resolve()
        if manifest_path.parent not in fixture_path.parents:
            raise HarnessError("Stage 1 frozen fixture escaped its child evidence directory")
        if sha256_file(fixture_path) != item["fixture_sha256"]:
            raise HarnessError(f"Stage 1 frozen fixture digest mismatch: {fixture_rel}")
    if len(fixture_results) != len(inputs) * REPEATS:
        raise HarnessError("Stage 1 oracle manifest has missing repeat results")
    validate_stage1_manual_bindings(
        inputs,
        fixture_results,
        parent_input_manifest.get("stage1_dispatch", {}).get("manual_inputs", []),
    )
    for item in fixture_results:
        if not isinstance(item.get("case_id"), str) or type(item.get("repeat")) is not int:
            raise HarnessError("Stage 1 oracle manifest has an invalid result identity")
        differences = item.get("differences")
        repeat_differences = item.get("repeat_differences")
        build = item.get("build")
        if (
            not isinstance(differences, dict)
            or not isinstance(repeat_differences, dict)
            or not isinstance(build, dict)
        ):
            raise HarnessError("Stage 1 oracle manifest contains malformed comparison evidence")
        if (
            differences.get("interpreter_vs_jit_dispatch") != 0
            or any(value != 0 for value in repeat_differences.values())
        ):
            raise HarnessError("Stage 1 oracle manifest contains a non-matching fixture")
        validate_stage1_fingerprint_and_builds(source_fingerprint, stage1_expected, build)
    for repeat in range(1, REPEATS + 1):
        repeated = [item.get("case_id") for item in fixture_results if item.get("repeat") == repeat]
        if len(repeated) != len(inputs) or set(repeated) != set(case_ids):
            raise HarnessError(
                f"Stage 1 oracle manifest does not contain a complete repeat {repeat}"
            )

    return {
        "summary_path": summary_path.relative_to(ROOT).as_posix(),
        "summary_sha256": sha256_file(summary_path),
        "manifest_path": manifest_path.relative_to(ROOT).as_posix(),
        "manifest_sha256": sha256_bytes(manifest_raw),
        "source_fingerprint": source_fingerprint,
        "manual_input_count": len(manual_ids),
        "generated_input_count": GENERATED_COUNT,
        "unique_input_count": len(inputs),
        "repeat_count": REPEATS,
        "fixture_result_count": len(fixture_results),
    }


def new_run_id() -> str:
    return time.strftime("stage3-%Y%m%d-%H%M%S", time.gmtime()) + "-" + uuid.uuid4().hex[:10]


def write_json_create(path: Path, value: object) -> bytes:
    raw = json.dumps(value, indent=2, ensure_ascii=False).encode("utf-8") + b"\n"
    with path.open("xb") as output:
        output.write(raw)
    return raw


def replace_json(path: Path, value: object) -> None:
    temporary = path.with_suffix(path.suffix + ".tmp")
    raw = json.dumps(value, indent=2, ensure_ascii=False).encode("utf-8") + b"\n"
    with temporary.open("xb") as output:
        output.write(raw)
    os.replace(temporary, path)


def decode_output(value: str | bytes | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return value


def run_step(
    name: str, argv: list[str], run_dir: Path, timeout_seconds: int, fingerprint: str
) -> dict:
    started = time.monotonic()
    log_path = run_dir / "logs" / f"{name}.log"
    if log_path.exists():
        raise HarnessError(f"refusing to overwrite existing log: {log_path}")
    stdout = ""
    stderr = ""
    returncode: int | None = None
    failure: str | None = None
    try:
        process = subprocess.run(
            argv,
            cwd=ROOT,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout_seconds,
            check=False,
        )
        stdout = process.stdout
        stderr = process.stderr
        returncode = process.returncode
        if returncode != 0:
            failure = f"signal_{-returncode}" if returncode < 0 else f"exit_{returncode}"
    except subprocess.TimeoutExpired as error:
        stdout = decode_output(error.stdout)
        stderr = decode_output(error.stderr)
        failure = "timeout"
    except OSError as error:
        stderr = f"{type(error).__name__}: {error}\n"
        failure = "spawn_error"

    combined = stdout
    if stderr:
        combined += "\n--- stderr ---\n" + stderr
    with log_path.open("x", encoding="utf-8") as output:
        output.write(combined)
    output_sha256 = sha256_bytes(combined.encode("utf-8"))

    try:
        end_fingerprint = source_fingerprint()
        if fingerprint != end_fingerprint:
            failure = "source_fingerprint_changed: source files changed during the command"
    except HarnessError as error:
        end_fingerprint = None
        failure = f"source_fingerprint_changed: {error}"

    return {
        "name": name,
        "argv": argv,
        "command": shlex.join(argv),
        "status": "passed" if failure is None else "failed",
        "failure": failure,
        "returncode": returncode,
        "timeout_seconds": timeout_seconds,
        "duration_seconds": round(time.monotonic() - started, 3),
        "source_fingerprint_after": end_fingerprint,
        "log": log_path.relative_to(ROOT).as_posix(),
        "output_tail": combined[-4000:],
        "output_sha256": output_sha256,
    }


def rust_host() -> tuple[str, str]:
    try:
        process = subprocess.run(
            ["rustc", "-vV"], cwd=ROOT, text=True, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, timeout=30, check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise HarnessError(f"rustc host query failed: {error}") from error
    if process.returncode != 0:
        raise HarnessError(f"rustc -vV failed: {process.stderr[-2000:]}")
    match = re.search(r"^host: (.+)$", process.stdout, re.MULTILINE)
    if not match:
        raise HarnessError("rustc -vV omitted host target")
    return match.group(1), process.stdout.splitlines()[0]


def cargo_version() -> str:
    try:
        process = subprocess.run(
            ["cargo", "--version"], cwd=ROOT, text=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30, check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise HarnessError(f"cargo version query failed: {error}") from error
    if process.returncode != 0:
        raise HarnessError(f"cargo --version failed: {process.stderr[-2000:]}")
    return process.stdout.strip()


def require_local_arm64_backend(host_target: str) -> None:
    if sys.platform != "darwin" or platform.machine().lower() not in {"arm64", "aarch64"}:
        raise HarnessError("local Stage 3 acceptance requires macOS ARM64")
    if not host_target.startswith("aarch64-apple-darwin"):
        raise HarnessError(f"Rust host target has no required ARM64 macOS backend: {host_target}")


def input_manifest() -> dict:
    manual = []
    for name in stage1_oracle.DEFAULT_CASES:
        path = (ROOT / name).resolve()
        if ROOT not in path.parents:
            raise HarnessError(f"manual Stage 1 input escapes repository root: {name}")
        fixture = json.loads(path.read_text())
        case_id = fixture.get("case_id") if isinstance(fixture, dict) else None
        if not isinstance(case_id, str) or not case_id:
            raise HarnessError(f"manual Stage 1 input has no case_id: {name}")
        manual.append({
            "case_id": case_id,
            "path": path.relative_to(ROOT).as_posix(),
            "sha256": sha256_file(path),
        })
    memory_path = ROOT / "crates/od-host/tests/memory_corpus.rs"
    host_path = ROOT / "crates/od-host/src/lib.rs"
    return {
        "schema_version": SCHEMA_VERSION,
        "stage": "S3-34e-local-correctness",
        "stage1_dispatch": {
            "source_fingerprint": stage1_oracle.source_fingerprint(ROOT),
            "command_profile": [
                "stage1_harness.py", "--suite", "oracle", "--engine", "jit-dispatch",
                "--local-only", "--generated-count", GENERATED_COUNT,
                "--seed", GENERATED_SEED, "--repeats", REPEATS,
            ],
            "manual_inputs": manual,
        },
        "memory_corpus": {
            "source": memory_path.relative_to(ROOT).as_posix(),
            "source_sha256": sha256_file(memory_path),
            "required_tests": sorted(MEMORY_REQUIRED_TESTS),
            "expected_unique_inputs": 2400,
            "replays_per_input": 2,
        },
        "lifecycle": {
            "source": host_path.relative_to(ROOT).as_posix(),
            "source_sha256": sha256_file(host_path),
            "exact_tests": list(LIFECYCLE_TESTS),
        },
        "local_rust_tests": {
            "gates": rust_test_gate_manifest(),
            "sources": [
                {"path": name, "sha256": sha256_file(ROOT / name)}
                for name in LOCAL_RUST_TEST_SOURCES
            ],
        },
    }


def validate_input_manifest_file(path: Path, expected_digest: str) -> dict:
    try:
        raw = path.read_bytes()
        validate_digest(raw, expected_digest)
        value = json.loads(raw)
    except (OSError, json.JSONDecodeError) as error:
        raise HarnessError(f"input manifest is missing or malformed: {error}") from error
    if not isinstance(value, dict) or value.get("schema_version") != SCHEMA_VERSION:
        raise HarnessError("input manifest has an unsupported schema")
    validate_rust_test_manifest(value.get("local_rust_tests"))
    return value


def validate_final_summary(summary: dict, input_manifest_path: Path) -> None:
    run_root = validate_run_id(summary.get("run_id"))
    validate_unselected_suites(summary.get("suites", {}))
    statuses = {name: summary.get("steps", {}).get(name, {}).get("status") for name in LOCAL_STEPS}
    validate_required_steps(statuses)
    steps = summary.get("steps", {})
    validate_required_rust_test_evidence(steps, run_root)
    lifecycle_steps = [step for step, suite, _, _ in RUST_TEST_GATES if suite == "local_lifecycle"]
    acceptance_steps = [step for step, suite, _, _ in RUST_TEST_GATES if suite == "local_acceptance"]
    suites = summary.get("suites", {})
    if (
        suites.get("local_lifecycle") != {"status": "passed", "tests": lifecycle_steps}
        or suites.get("local_acceptance") != {"status": "passed", "tests": acceptance_steps}
    ):
        raise HarnessError("required local Rust test suite membership is incomplete or forged")
    digest = summary.get("input_manifest_sha256")
    if not isinstance(digest, str):
        raise HarnessError("input manifest digest is missing from summary")
    validate_input_manifest_file(input_manifest_path, digest)
    if summary.get("status") != "local_only":
        raise HarnessError("successful Stage 3 local harness result must be local_only")
    source = summary.get("source", {})
    validate_fingerprint(source.get("fingerprint_start", ""), source.get("fingerprint_end", ""))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", choices=("local",), default="local")
    args = parser.parse_args(argv)

    run_id = new_run_id()
    run_dir = ROOT / "work" / "stage3-harness" / run_id
    try:
        run_dir.mkdir(parents=True, exist_ok=False)
    except OSError as error:
        print(f"stage3 harness: cannot create unique run directory: {error}", file=sys.stderr)
        return 1
    (run_dir / "logs").mkdir()

    summary_path = run_dir / "summary.json"
    summary = {
        "schema_version": SCHEMA_VERSION,
        "run_id": run_id,
        "status": "running",
        "suite_selection": args.suite,
        "result_scope": "local_only",
        "source": {
            "revision": None,
            "fingerprint_start": None,
            "fingerprint_end": None,
            "fingerprint_method": (
                "sha256 of sorted tracked and non-ignored untracked paths, modes and contents"
            ),
        },
        "host": {
            "os": platform.platform(),
            "system": sys.platform,
            "machine": platform.machine(),
            "python": platform.python_version(),
            "rust_target": None,
            "rustc": None,
            "cargo": None,
        },
        "suites": {name: {"status": "not_run"} for name in UNSELECTED_SUITES},
        "steps": {name: {"status": "not_run"} for name in LOCAL_STEPS},
        "children": {},
        "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }
    write_json_create(summary_path, summary)

    try:
        revision = subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30, check=False,
        )
        if revision.returncode != 0:
            raise HarnessError(f"cannot read git revision: {revision.stderr[-2000:]}")
        fingerprint = source_fingerprint()
        host_target, rustc_version = rust_host()
        cargo_version_string = cargo_version()
        summary["source"]["revision"] = revision.stdout.strip()
        summary["source"]["fingerprint_start"] = fingerprint
        summary["host"]["rust_target"] = host_target
        summary["host"]["rustc"] = rustc_version
        summary["host"]["cargo"] = cargo_version_string
        replace_json(summary_path, summary)

        manifest = input_manifest()
        manifest_path = run_dir / "input_manifest.json"
        manifest_raw = write_json_create(manifest_path, manifest)
        summary["input_manifest"] = manifest_path.relative_to(ROOT).as_posix()
        summary["input_manifest_sha256"] = sha256_bytes(manifest_raw)
        replace_json(summary_path, summary)

        require_local_arm64_backend(host_target)

        commands = [
            (
                "harness_self_tests",
                [sys.executable, "tests/cpu/test_stage3_harness.py"],
                120,
            ),
            (
                "stage1_dispatch",
                [
                    sys.executable, "tests/cpu/stage1_harness.py", "--suite", "oracle",
                    "--engine", "jit-dispatch", "--local-only", "--generated-count",
                    str(GENERATED_COUNT), "--seed", GENERATED_SEED, "--repeats", str(REPEATS),
                ],
                7200,
            ),
            (
                "memory_corpus",
                [
                    "cargo", "test", "--offline", "-p", "od-host", "--test",
                    "memory_corpus", "--", "--test-threads=1",
                ],
                7200,
            ),
        ]
        for name, command, timeout in commands:
            step = run_step(name, command, run_dir, timeout, fingerprint)
            summary["steps"][name] = step
            replace_json(summary_path, summary)
            if step["status"] != "passed":
                raise HarnessError(f"required step {name} failed: {step['failure']}")
            if name == "harness_self_tests":
                try:
                    self_test_output = (ROOT / step["log"]).read_text()
                    outcomes = validate_self_test_output(self_test_output)
                except HarnessError as error:
                    step["status"] = "failed"
                    step["failure"] = f"invalid self-test evidence: {error}"
                    summary["steps"][name] = step
                    replace_json(summary_path, summary)
                    raise
                summary["steps"][name]["test_outcomes"] = outcomes
                summary["steps"][name]["test_count"] = len(outcomes)
                replace_json(summary_path, summary)
            if name == "stage1_dispatch":
                try:
                    child_output = (ROOT / step["log"]).read_text()
                    child = validate_stage1_evidence(child_output, manifest)
                except HarnessError as error:
                    step["status"] = "failed"
                    step["failure"] = f"invalid child evidence: {error}"
                    summary["steps"][name] = step
                    replace_json(summary_path, summary)
                    raise
                summary["children"]["stage1"] = child
                replace_json(summary_path, summary)
            if name == "memory_corpus":
                corpus_output = (ROOT / step["log"]).read_text()
                outcomes = validate_test_outcomes(
                    corpus_output, MEMORY_REQUIRED_TESTS, MEMORY_ALLOWED_IGNORED_TESTS
                )
                summary["steps"][name]["test_outcomes"] = outcomes
                summary["steps"][name]["unique_inputs"] = 2400
                summary["steps"][name]["replays_per_input"] = 2
                summary["steps"][name]["allowed_ignored"] = sorted(MEMORY_ALLOWED_IGNORED_TESTS)
                replace_json(summary_path, summary)

        rust_test_steps: dict[str, list[dict]] = {
            "local_lifecycle": [],
            "local_acceptance": [],
        }
        for step_name, suite, package, exact_test_name in RUST_TEST_GATES:
            command = rust_test_argv(package, exact_test_name)
            step = run_step(step_name, command, run_dir, 1800, fingerprint)
            step["gate_suite"] = suite
            step["package"] = package
            step["test_identity"] = exact_test_name
            if step["status"] == "passed":
                try:
                    outcomes = validate_exact_rust_test_output(
                        step["output_tail"], exact_test_name
                    )
                    step["test_outcomes"] = outcomes
                except HarnessError as error:
                    step["status"] = "failed"
                    step["failure"] = str(error)
            summary["steps"][step_name] = step
            rust_test_steps[suite].append(step)
            replace_json(summary_path, summary)
            if step["status"] != "passed":
                raise HarnessError(
                    f"required Rust test gate {step_name} failed: {step.get('failure')}"
                )

        summary["suites"]["local_lifecycle"] = {
            "status": "passed",
            "tests": [item["name"] for item in rust_test_steps["local_lifecycle"]],
        }
        summary["suites"]["local_acceptance"] = {
            "status": "passed",
            "tests": [item["name"] for item in rust_test_steps["local_acceptance"]],
        }
        summary["suites"]["stage1_dispatch"] = {
            "status": "local_only",
            "child": summary["children"]["stage1"],
        }
        summary["suites"]["memory_corpus"] = {
            "status": "passed",
            "unique_inputs": 2400,
            "replays_per_input": 2,
            "test_outcomes": summary["steps"]["memory_corpus"]["test_outcomes"],
        }
        summary["source"]["fingerprint_end"] = source_fingerprint()
        validate_fingerprint(fingerprint, summary["source"]["fingerprint_end"])
        summary["status"] = "local_only"
        validate_final_summary(summary, manifest_path)
        summary["finished_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        replace_json(summary_path, summary)
        print(f"stage3_harness_status={summary['status']}")
        print(f"input_manifest_sha256={summary['input_manifest_sha256']}")
        print(f"summary_path={summary_path.relative_to(ROOT)}")
        return 0
    except Exception as error:
        try:
            if "summary" in locals():
                summary["status"] = "failed"
                summary["error"] = f"{type(error).__name__}: {error}"
                summary["source"]["fingerprint_end"] = _safe_fingerprint()
                summary["finished_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
                replace_json(summary_path, summary)
        except Exception:
            pass
        print(f"stage3 harness failed: {type(error).__name__}: {error}", file=sys.stderr)
        if "summary_path" in locals():
            print(f"summary_path={summary_path.relative_to(ROOT)}", file=sys.stderr)
        return 1


def _safe_fingerprint() -> str | None:
    try:
        return source_fingerprint()
    except HarnessError:
        return None


if __name__ == "__main__":
    raise SystemExit(main())
