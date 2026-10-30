#!/usr/bin/env python3
"""Run the bounded local Stage 3 correctness suites and preserve their evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import signal
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
    "reducer_preserves_load_pair_site_fault_boundary_and_mismatch_class",
    "named_fault_stop_budget_and_continuation_cases_are_separate",
    "comparator_rejects_corruption_and_serializes_named_diagnostics",
    "bounded_reducer_is_deterministic_preserves_class_and_saves_report",
    "reducer_rejects_malformed_and_unavailable_inputs_and_stops_on_nonreproduction",
    "reducer_rejects_same_mismatch_moving_to_continuation_phase",
    "generic_reducer_can_clear_marker_when_comparator_allows_candidate",
    "mismatch_report_preserves_empty_first_and_mismatching_continuation_phases",
    "replay_envelope_round_trips_and_rejects_malformed_reports",
    "replay_success_fault_stop_budget_and_continuation_for_all_sequences",
    "saved_mismatch_report_replays_and_preserves_original_classes",
    "replay_refuses_to_claim_equivalence_without_native_backend",
}
MEMORY_EXACT_GATE = "reducer_preserves_load_pair_site_fault_boundary_and_mismatch_class"
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
    "test_s3_34h_exact_gate_requires_exact_identity_and_argv",
    "test_allowed_environment_replay_tests_do_not_hide_unexpected_ignored_tests",
    "test_self_test_output_validator_rejects_zero_match_and_all_skipped",
    "test_stage1_child_fingerprint_and_both_builds_are_pinned",
    "test_manual_frozen_input_and_result_bind_to_parent_manifest",
    "test_rust_test_manifest_rejects_a_missing_mandatory_gate",
    "test_s3_33_native_gates_are_pinned_once_in_manifest_steps_and_summary",
    "test_rust_gate_summary_rejects_an_omitted_mandatory_gate",
    "test_rust_gate_summary_rejects_forged_passed_evidence",
    "test_rust_gate_rejects_zero_match_ignored_and_failed_runs",
    "test_final_summary_rejects_missing_invalid_and_traversal_run_id",
    "test_final_summary_rejects_run_directory_and_logs_symlinks",
    "test_final_summary_rejects_step_log_symlink_and_external_log_path",
    "test_vmhost_process_step_rejects_missing_step",
    "test_vmhost_process_step_rejects_malformed_log_evidence",
    "test_vmhost_process_step_rejects_timeout",
    "test_vmhost_process_step_rejects_forged_passed",
    "test_vmhost_process_child_log_binding_matches_command",
    "test_vmhost_codesign_commands_use_resolved_tool_path",
    "test_vmhost_process_manifest_covers_every_gate_once",
    "test_generated_fault_smoke_cleans_child_group_on_success_and_failure",
    "test_app_bundle_smoke_preserves_and_executes_five_argument_bundle",
    "test_vmhost_process_timeout_kills_child_group",
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
    "active_code_v3_pc_map_survives_artifact_drop_invalidation_and_safe_reuse",
    "direct_memory_v2_pc_map_survives_artifact_drop_and_arena_slot_reuse",
    "active_code_v3_architectural_write_fault_matches_interpreter_and_is_atomic",
    "ir_two_load_pair_preserves_callbacks_faults_and_continuation",
)

ADDITIONAL_TEST_GATES = (
    (
        "s3_27d_direct_memory_v2_pc_map", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_checked_load_uses_distinct_v2_artifact_and_pc_map",
    ),
    (
        "s3_27d_direct_memory_v2_pc_map_lifecycle", "local_acceptance", "od-host",
        "tests::direct_memory_v2_pc_map_survives_artifact_drop_and_arena_slot_reuse",
    ),
    (
        "s3_27d_direct_memory_v2_pc_map_invalidation", "local_acceptance", "od-host",
        "tests::tracked_v2_store_invalidates_alias_after_return_and_preserves_late_stop_commit",
    ),
    (
        "s3_27d_direct_memory_v2_error_distinction", "local_acceptance", "od-host",
        "tests::address32_low_base_rsi_rdi_index_sib_matches_all_execution_paths",
    ),
    (
        "s3_27c_active_code_v3_pc_map", "local_acceptance", "od-jit",
        "memory_load_v2::tests::active_code_v3_compiles_only_exact_three_instruction_source",
    ),
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
    (
        "s3_33a_native_inc_rax_host", "local_acceptance", "od-host",
        "tests::native_inc_rax_fallback_carries_canonical_flags_into_native_branches",
    ),
    (
        "s3_33a_native_inc_rax_compiler", "local_acceptance", "od-jit",
        "block_compile::tests::native_inc_rax_is_opt_in_and_compiles_following_branch_consumers",
    ),
    (
        "s3_33b_native_dec_rax_host", "local_acceptance", "od-host",
        "tests::native_dec_rax_preserves_cf_and_matches_interpreter_branches",
    ),
    (
        "s3_33b_native_dec_rax_compiler", "local_acceptance", "od-jit",
        "block_compile::tests::native_dec_rax_is_opt_in_and_compiles_following_branch_consumers",
    ),
    (
        "s3_33c_native_neg_rax_host", "local_acceptance", "od-host",
        "tests::native_neg_rax_replaces_full_pending_flags_and_matches_interpreter",
    ),
    (
        "s3_33c_native_neg_rax_compiler", "local_acceptance", "od-jit",
        "block_compile::tests::native_neg_rax_is_opt_in_and_compiles_following_branch_consumers",
    ),
    (
        "s3_33d_native_add_rax_imm8_host", "local_acceptance", "od-host",
        "tests::native_add_rax_imm8_matches_interpreter_full_state_and_pending_branch",
    ),
    (
        "s3_33d_native_add_rax_imm8_compiler", "local_acceptance", "od-jit",
        "block_compile::tests::native_add_rax_imm8_compiles_eager_and_candidate_branch_consumers",
    ),
    (
        "s3_33d_native_add_rax_imm8_admission", "local_acceptance", "od-jit",
        "block::tests::add_rax_imm8_admission_covers_signed_byte_range_and_rejects_neighbors",
    ),
    (
        "s3_33e_native_cmp_rax_imm8_host", "local_acceptance", "od-host",
        "tests::native_cmp_rax_imm8_matches_interpreter_and_preserves_gprs",
    ),
    (
        "s3_33e_native_cmp_rax_imm8_compiler", "local_acceptance", "od-jit",
        "block_compile::tests::native_cmp_rax_imm8_compiles_eager_and_candidate_branch_consumers",
    ),
    (
        "s3_33e_native_cmp_rax_imm8_admission", "local_acceptance", "od-jit",
        "block::tests::cmp_rax_imm8_admission_covers_signed_byte_range_and_rejects_neighbors",
    ),
    (
        "s3_33f_native_sub_rax_imm8_host", "local_acceptance", "od-host",
        "tests::native_sub_rax_imm8_matches_interpreter_full_state_and_pending_branch",
    ),
    (
        "s3_33f_native_sub_rax_imm8_compiler", "local_acceptance", "od-jit",
        "block_compile::tests::native_sub_rax_imm8_compiles_eager_and_candidate_branch_consumers",
    ),
    (
        "s3_33f_native_sub_rax_imm8_admission", "local_acceptance", "od-jit",
        "block::tests::sub_rax_imm8_admission_covers_signed_byte_range_and_rejects_neighbors",
    ),
    (
        "s3_33g_native_not_rax_admission", "local_acceptance", "od-jit",
        "block::tests::not_rax_admission_is_exact_and_separate_from_neg_profile",
    ),
    (
        "s3_33g_native_not_rax_emitter", "local_acceptance", "od-jit",
        "block_emitter::tests::native_not_rax_preserving_subtract_emits_no_helper_for_register_homes",
    ),
    (
        "s3_33g_native_not_rax_compiler", "local_acceptance", "od-jit",
        "block_compile::tests::native_not_rax_is_opt_in_and_emits_no_flag_helper_for_not",
    ),
    (
        "s3_33g_native_not_rax_host", "local_acceptance", "od-host",
        "tests::native_not_rax_preserves_full_flags_and_pending_writer_state",
    ),
    (
        "s3_33h_native_lea_rax_rbx_admission", "local_acceptance", "od-jit",
        "block::tests::lea_rax_rbx_admission_is_exact_and_separate_from_other_profiles",
    ),
    (
        "s3_33h_native_lea_rax_rbx_compiler", "local_acceptance", "od-jit",
        "block_compile::tests::native_lea_rax_rbx_compiles_eager_and_candidate_without_memory_helper",
    ),
    (
        "s3_33h_native_lea_rax_rbx_host", "local_acceptance", "od-host",
        "tests::native_lea_rax_rbx_copies_unmapped_addresses_without_data_access",
    ),
    (
        "s3_33i_native_xchg_rax_rbx_admission", "local_acceptance", "od-jit",
        "block::tests::xchg_rax_rbx_admission_is_exact_and_separate_from_other_profiles",
    ),
    (
        "s3_33i_native_xchg_rax_rbx_emitter", "local_acceptance", "od-jit",
        "block_emitter::tests::dual_write_runtime_shape_is_limited_to_atomic_rax_rbx_exchange",
    ),
    (
        "s3_33i_native_xchg_rax_rbx_compiler", "local_acceptance", "od-jit",
        "block_compile::tests::native_xchg_rax_rbx_compiles_with_atomic_pending_statepoints",
    ),
    (
        "s3_33i_native_xchg_rax_rbx_host", "local_acceptance", "od-host",
        "tests::native_xchg_rax_rbx_matches_interpreter_and_preserves_flags",
    ),
    (
        "s3_33j_native_lea_rbx_disp8_admission", "local_acceptance", "od-jit",
        "block::tests::lea_rax_rbx_disp8_admission_covers_all_signed_bytes_and_rejects_neighbors",
    ),
    (
        "s3_33j_native_lea_rbx_disp8_emitter", "local_acceptance", "od-jit",
        "block_emitter::tests::preserving_integer_support_is_limited_to_w64_add_or_subtract_with_result",
    ),
    (
        "s3_33j_native_lea_rbx_disp8_compiler", "local_acceptance", "od-jit",
        "block_compile::tests::native_lea_rax_rbx_disp8_compiles_eager_and_candidate_without_memory_helper",
    ),
    (
        "s3_33j_native_lea_rbx_disp8_host", "local_acceptance", "od-host",
        "tests::native_lea_rax_rbx_disp8_matches_interpreter_without_data_access",
    ),
    (
        "s3_33k_native_mov_rax_imm32_admission", "local_acceptance", "od-jit",
        "block::tests::mov_rax_imm32_admission_sign_extends_and_rejects_neighbors",
    ),
    (
        "s3_33k_native_mov_rax_imm32_compiler", "local_acceptance", "od-jit",
        "block_compile::tests::native_mov_rax_imm32_compiles_eager_and_candidate_without_flag_helpers",
    ),
    (
        "s3_33k_native_mov_rax_imm32_host", "local_acceptance", "od-host",
        "tests::native_mov_rax_imm32_matches_interpreter_full_state_and_preserves_flags",
    ),
    (
        "s3_33l_native_lea_rbx_rcx_admission", "local_acceptance", "od-jit",
        "block::tests::lea_rax_rbx_rcx_admission_is_exact_and_separate_from_other_lea_profiles",
    ),
    (
        "s3_33l_native_lea_rbx_rcx_compiler", "local_acceptance", "od-jit",
        "block_compile::tests::native_lea_rax_rbx_rcx_compiles_eager_and_candidate_without_memory_helper",
    ),
    (
        "s3_33l_native_lea_rbx_rcx_host", "local_acceptance", "od-host",
        "tests::native_lea_rax_rbx_rcx_matches_interpreter_and_preserves_flags_without_data_access",
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
    "memory_s3_34h_exact_gate",
    *(step for step, _, _, _ in RUST_TEST_GATES),
)
UNSELECTED_SUITES = ("physical_x86", "mac_mini", "performance", "owner_backed", "stage2")
LOCAL_SUITE_NAMES = (
    "local_lifecycle", "local_acceptance", "stage1_dispatch", "memory_corpus",
)
PROCESS_SUITE_STEPS = (
    "process_sw_vers",
    "process_uname",
    "process_rustc_version",
    "process_cargo_version",
    "process_swift_version",
    "process_xcode_version",
    "process_codesign_path",
    "process_build_od_host",
    "process_compile_vmhost",
    "process_sign_vmhost",
    "process_verify_vmhost",
    "process_vmhost_signature",
    "process_vmhost_entitlements",
    "process_compile_launcher",
    "process_sign_launcher",
    "process_verify_launcher",
    "process_launcher_signature",
    "process_launcher_entitlements",
    "process_compile_fault_helper",
    "process_sign_fault_helper",
    "process_verify_fault_helper",
    "process_fault_helper_signature",
    "process_fault_helper_entitlements",
    "process_control_race",
    "process_isolation",
    "process_app_bundle",
    "process_bundle_verify",
    "process_bundle_signature",
    "process_bundle_vmhost_signature",
    "process_bundle_vmhost_entitlements",
    "process_bundle_launcher_signature",
    "process_bundle_launcher_entitlements",
    "process_generated_fault",
)
PROCESS_SUITE_NAME = "vmhost_process"
ALL_SUITES = (*LOCAL_SUITE_NAMES, PROCESS_SUITE_NAME, *UNSELECTED_SUITES)
ALL_STEPS = (*LOCAL_STEPS, *PROCESS_SUITE_STEPS)
PROCESS_STEP_TIMEOUTS = {
    "process_sw_vers": 30,
    "process_uname": 30,
    "process_rustc_version": 30,
    "process_cargo_version": 30,
    "process_swift_version": 60,
    "process_xcode_version": 60,
    "process_codesign_path": 30,
    "process_build_od_host": 1800,
    "process_compile_vmhost": 900,
    "process_sign_vmhost": 120,
    "process_verify_vmhost": 120,
    "process_vmhost_signature": 120,
    "process_vmhost_entitlements": 120,
    "process_compile_launcher": 900,
    "process_sign_launcher": 120,
    "process_verify_launcher": 120,
    "process_launcher_signature": 120,
    "process_launcher_entitlements": 120,
    "process_compile_fault_helper": 900,
    "process_sign_fault_helper": 120,
    "process_verify_fault_helper": 120,
    "process_fault_helper_signature": 120,
    "process_fault_helper_entitlements": 120,
    "process_control_race": 900,
    "process_isolation": 120,
    "process_generated_fault": 30,
    "process_app_bundle": 180,
    "process_bundle_verify": 120,
    "process_bundle_signature": 120,
    "process_bundle_vmhost_signature": 120,
    "process_bundle_vmhost_entitlements": 120,
    "process_bundle_launcher_signature": 120,
    "process_bundle_launcher_entitlements": 120,
}
PROCESS_SOURCE_PATHS = (
    "Cargo.lock",
    "Cargo.toml",
    "crates/od-host/Cargo.toml",
    "crates/od-host/src/lib.rs",
    "include/open_desktop/host_abi.h",
    "apps/vmhost/Sources/ControlFrame.swift",
    "apps/vmhost/Sources/main.swift",
    "apps/macos/Sources/launcher.swift",
    "apps/vmhost/Configuration/vmhost-smoke.entitlements",
    "tests/system/vmhost_control_race_smoke.py",
    "tests/system/vmhost_isolation_smoke.py",
    "tests/system/vmhost_generated_fault_smoke.py",
    "tests/system/vmhost_generated_fault_helper.swift",
    "tests/system/vmhost_app_bundle_smoke.sh",
    "tests/cpu/stage3_harness.py",
)
LOCAL_RUST_TEST_SOURCES = (
    "crates/od-host/tests/memory_corpus.rs",
    "crates/od-host/src/lib.rs",
    "crates/od-jit/src/block.rs",
    "crates/od-jit/src/block_emitter.rs",
    "crates/od-jit/src/block_compile.rs",
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


def vmhost_process_paths(run_root: Path) -> dict[str, Path]:
    artifacts = run_root / "artifacts"
    build_target = run_root / "build-target"
    return {
        "artifacts": artifacts,
        "build_target": build_target,
        "swift_modules": run_root / "swift-module-cache",
        "od_host_library": build_target / "release" / "libod_host.a",
        "vmhost": artifacts / "od-vmhost-smoke",
        "launcher": artifacts / "open-desktop-launcher-smoke",
        "fault_helper": artifacts / "od-vmhost-generated-fault-smoke",
        "app_bundle": artifacts / "OpenDesktop-Stage0-Smoke.app",
        "entitlements": ROOT / "apps/vmhost/Configuration/vmhost-smoke.entitlements",
    }


def vmhost_process_commands(
    run_root: Path, codesign_path: str = "${CODESIGN_PATH}"
) -> dict[str, list[str]]:
    paths = vmhost_process_paths(run_root)
    frameworks = [
        "-framework", "Security",
        "-framework", "SystemConfiguration",
        "-framework", "CoreFoundation",
        "-framework", "CoreServices",
        "-framework", "Foundation",
    ]
    swift_common = [
        "xcrun", "swiftc", "-module-cache-path", str(paths["swift_modules"]),
        "-parse-as-library", "-O", "-import-objc-header",
        "include/open_desktop/host_abi.h", "apps/vmhost/Sources/ControlFrame.swift",
    ]
    link_args = ["-ldl", "-lpthread", "-lm", *frameworks]
    return {
        "process_sw_vers": ["sw_vers"],
        "process_uname": ["uname", "-a"],
        "process_rustc_version": ["rustc", "-vV"],
        "process_cargo_version": ["cargo", "--version"],
        "process_swift_version": ["xcrun", "swiftc", "--version"],
        "process_xcode_version": ["xcodebuild", "-version"],
        "process_codesign_path": ["xcrun", "--find", "codesign"],
        "process_build_od_host": [
            "cargo", "build", "--release", "-p", "od-host", "--offline",
            "--target-dir", str(paths["build_target"]),
        ],
        "process_compile_vmhost": [
            *swift_common, "apps/vmhost/Sources/main.swift",
            str(paths["od_host_library"]), *link_args, "-o", str(paths["vmhost"]),
        ],
        "process_sign_vmhost": [
            codesign_path, "--force", "--sign", "-", "--options", "runtime",
            "--entitlements", str(paths["entitlements"]), str(paths["vmhost"]),
        ],
        "process_verify_vmhost": [
            codesign_path, "--verify", "--strict", "--verbose=2", str(paths["vmhost"]),
        ],
        "process_vmhost_signature": [
            codesign_path, "-d", "--verbose=4", str(paths["vmhost"]),
        ],
        "process_vmhost_entitlements": [
            codesign_path, "-d", "--entitlements", ":-", str(paths["vmhost"]),
        ],
        "process_compile_launcher": [
            *swift_common, "apps/macos/Sources/launcher.swift",
            str(paths["od_host_library"]), *link_args, "-o", str(paths["launcher"]),
        ],
        "process_sign_launcher": [
            codesign_path, "--force", "--sign", "-", "--options", "runtime",
            str(paths["launcher"]),
        ],
        "process_verify_launcher": [
            codesign_path, "--verify", "--strict", "--verbose=2", str(paths["launcher"]),
        ],
        "process_launcher_signature": [
            codesign_path, "-d", "--verbose=4", str(paths["launcher"]),
        ],
        "process_launcher_entitlements": [
            codesign_path, "-d", "--entitlements", ":-", str(paths["launcher"]),
        ],
        "process_compile_fault_helper": [
            *swift_common, "tests/system/vmhost_generated_fault_helper.swift",
            str(paths["od_host_library"]), *link_args, "-o", str(paths["fault_helper"]),
        ],
        "process_sign_fault_helper": [
            codesign_path, "--force", "--sign", "-", "--options", "runtime",
            "--entitlements", str(paths["entitlements"]), str(paths["fault_helper"]),
        ],
        "process_verify_fault_helper": [
            codesign_path, "--verify", "--strict", "--verbose=2", str(paths["fault_helper"]),
        ],
        "process_fault_helper_signature": [
            codesign_path, "-d", "--verbose=4", str(paths["fault_helper"]),
        ],
        "process_fault_helper_entitlements": [
            codesign_path, "-d", "--entitlements", ":-", str(paths["fault_helper"]),
        ],
        "process_control_race": [
            sys.executable, "tests/system/vmhost_control_race_smoke.py",
            str(paths["launcher"]), str(paths["vmhost"]), "--diagnostics-dir",
            str(run_root / "logs" / "process_control_race_children"),
        ],
        "process_isolation": [
            sys.executable, "tests/system/vmhost_isolation_smoke.py",
            str(paths["vmhost"]), "--diagnostics-dir",
            str(run_root / "logs" / "process_isolation_children"),
        ],
        "process_generated_fault": [
            sys.executable, "tests/system/vmhost_generated_fault_smoke.py",
            str(paths["launcher"]), str(paths["fault_helper"]), "--diagnostics-dir",
            str(run_root / "logs" / "process_generated_fault_children"),
        ],
        "process_app_bundle": [
            "sh", "tests/system/vmhost_app_bundle_smoke.sh",
            str(paths["launcher"]), str(paths["vmhost"]), str(paths["entitlements"]),
            str(paths["app_bundle"]), codesign_path,
        ],
        "process_bundle_verify": [
            codesign_path, "--verify", "--strict", "--deep", "--verbose=2",
            str(paths["app_bundle"]),
        ],
        "process_bundle_signature": [
            codesign_path, "-d", "--verbose=4", str(paths["app_bundle"]),
        ],
        "process_bundle_vmhost_signature": [
            codesign_path, "-d", "--verbose=4",
            str(paths["app_bundle"] / "Contents/MacOS/od-vmhost-smoke"),
        ],
        "process_bundle_vmhost_entitlements": [
            codesign_path, "-d", "--entitlements", ":-",
            str(paths["app_bundle"] / "Contents/MacOS/od-vmhost-smoke"),
        ],
        "process_bundle_launcher_signature": [
            codesign_path, "-d", "--verbose=4",
            str(paths["app_bundle"] / "Contents/MacOS/OpenDesktopLauncher"),
        ],
        "process_bundle_launcher_entitlements": [
            codesign_path, "-d", "--entitlements", ":-",
            str(paths["app_bundle"] / "Contents/MacOS/OpenDesktopLauncher"),
        ],
    }


def vmhost_process_command_manifest() -> list[dict[str, object]]:
    run_placeholder = ROOT / "work" / "stage3-harness" / "<RUN_ID>"
    return [
        {"step": name, "argv": [
            value.replace(str(run_placeholder), "${RUN_ROOT}")
            for value in argv
        ]}
        for name, argv in vmhost_process_commands(run_placeholder).items()
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


def validate_unselected_suites(
    suites: dict[str, dict], names: tuple[str, ...] = UNSELECTED_SUITES
) -> None:
    invalid = [
        name for name in names
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


def validate_memory_exact_gate_evidence(step: dict, run_root: Path | None = None) -> None:
    expected_argv = [
        "cargo", "test", "--offline", "-p", "od-host", "--test", "memory_corpus",
        MEMORY_EXACT_GATE, "--", "--exact", "--test-threads=1",
    ]
    if (
        step.get("name") != "memory_s3_34h_exact_gate"
        or step.get("argv") != expected_argv
        or step.get("returncode") != 0
        or step.get("failure") is not None
        or step.get("test_identity") != MEMORY_EXACT_GATE
    ):
        raise HarnessError("S3-34h exact memory gate invocation is missing or forged")
    try:
        outcomes = validate_exact_rust_test_output(step.get("output_tail", ""), MEMORY_EXACT_GATE)
    except HarnessError as error:
        raise HarnessError(f"S3-34h exact memory gate lacks passing output: {error}") from error
    if step.get("test_outcomes") != outcomes:
        raise HarnessError("S3-34h exact memory gate outcome is forged")
    log_rel = step.get("log")
    output_digest = step.get("output_sha256")
    if not isinstance(log_rel, str) or not isinstance(output_digest, str):
        raise HarnessError("S3-34h exact memory gate log evidence is missing")
    log_lexical = ROOT / log_rel
    log_path = log_lexical.resolve()
    if ROOT not in log_path.parents:
        raise HarnessError("S3-34h exact memory gate log escaped repository root")
    if run_root is not None and (
        log_lexical != run_root / "logs" / "memory_s3_34h_exact_gate.log"
        or log_path != log_lexical
    ):
        raise HarnessError("S3-34h exact memory gate log path differs")
    try:
        raw = log_path.read_bytes()
        output = raw.decode("utf-8", errors="replace")
    except OSError as error:
        raise HarnessError("S3-34h exact memory gate log is missing") from error
    if sha256_bytes(raw) != output_digest or step.get("output_tail") != output[-4000:]:
        raise HarnessError("S3-34h exact memory gate log binding differs")


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


def run_process_step(
    name: str,
    argv: list[str],
    run_dir: Path,
    timeout_seconds: int,
    fingerprint: str,
) -> dict:
    """Run a VMHost process-suite command in its own killable process group."""
    started = time.monotonic()
    log_path = run_dir / "logs" / f"{name}.log"
    stdout_log = run_dir / "logs" / f"{name}.stdout.log"
    stderr_log = run_dir / "logs" / f"{name}.stderr.log"
    if any(path.exists() for path in (log_path, stdout_log, stderr_log)):
        raise HarnessError(f"refusing to overwrite existing process log: {name}")
    stdout = ""
    stderr = ""
    returncode: int | None = None
    failure: str | None = None
    process: subprocess.Popen | None = None
    group_cleanup = "empty"
    timeout_diagnostics: dict[str, object] | None = None
    try:
        process = subprocess.Popen(
            argv,
            cwd=ROOT,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            start_new_session=True,
        )
        try:
            stdout, stderr = process.communicate(timeout=timeout_seconds)
        except subprocess.TimeoutExpired as timeout_error:
            failure = "timeout"
            group_cleanup = "timed_out_and_killed"
            partial_stdout = decode_output(timeout_error.output)
            partial_stderr = decode_output(timeout_error.stderr)
            timeout_diagnostics = {}
            for stream_name, contents in (
                ("stdout", partial_stdout), ("stderr", partial_stderr)
            ):
                diagnostic_path = (
                    run_dir / "logs" /
                    f"{name}.timeout-before-cleanup.{stream_name}.log"
                )
                with diagnostic_path.open("x", encoding="utf-8") as output:
                    output.write(contents)
                timeout_diagnostics[stream_name] = {
                    "path": diagnostic_path.relative_to(ROOT).as_posix(),
                    "sha256": sha256_bytes(contents.encode("utf-8")),
                    "size_bytes": len(contents.encode("utf-8")),
                }
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                stdout, stderr = process.communicate(timeout=3)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                stdout, stderr = process.communicate()
        returncode = process.returncode
        if failure is None and returncode != 0:
            failure = f"signal_{-returncode}" if returncode < 0 else f"exit_{returncode}"
        if process_group_exists(process.pid):
            group_cleanup = (
                "timed_out_group_terminated" if failure == "timeout"
                else "orphaned_group_terminated"
            )
            failure = failure or "orphaned_process_group"
            terminate_process_group(process.pid)
            if process_group_exists(process.pid):
                group_cleanup = (
                    "timed_out_group_remained" if failure == "timeout"
                    else "orphaned_group_remained"
                )
                if failure != "timeout":
                    failure = "process_group_cleanup_failed"
    except OSError as error:
        stderr = f"{type(error).__name__}: {error}\n"
        failure = "spawn_error"

    combined = stdout
    if stderr:
        combined += "\n--- stderr ---\n" + stderr
    with log_path.open("x", encoding="utf-8") as output:
        output.write(combined)
    with stdout_log.open("x", encoding="utf-8") as output:
        output.write(stdout)
    with stderr_log.open("x", encoding="utf-8") as output:
        output.write(stderr)
    output_sha256 = sha256_bytes(combined.encode("utf-8"))
    try:
        end_fingerprint = source_fingerprint()
        if fingerprint != end_fingerprint:
            failure = "source_fingerprint_changed: source files changed during the command"
    except HarnessError as error:
        end_fingerprint = None
        failure = f"source_fingerprint_changed: {error}"

    result = {
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
        "stdout_log": stdout_log.relative_to(ROOT).as_posix(),
        "stderr_log": stderr_log.relative_to(ROOT).as_posix(),
        "output_tail": combined[-4000:],
        "output_sha256": output_sha256,
        "stdout_sha256": sha256_bytes(stdout.encode("utf-8")),
        "stderr_sha256": sha256_bytes(stderr.encode("utf-8")),
        "process_group_cleanup": group_cleanup,
        "process_group_id": process.pid if process is not None else None,
    }
    if timeout_diagnostics is not None:
        result["timeout_diagnostics"] = timeout_diagnostics
    return result


def process_group_exists(process_group_id: int) -> bool:
    try:
        os.killpg(process_group_id, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def terminate_process_group(process_group_id: int) -> None:
    try:
        os.killpg(process_group_id, signal.SIGTERM)
    except ProcessLookupError:
        return
    deadline = time.monotonic() + 3
    while time.monotonic() < deadline:
        if not process_group_exists(process_group_id):
            return
        time.sleep(0.05)
    try:
        os.killpg(process_group_id, signal.SIGKILL)
    except ProcessLookupError:
        return
    deadline = time.monotonic() + 3
    while time.monotonic() < deadline and process_group_exists(process_group_id):
        time.sleep(0.05)


def child_log_evidence(run_root: Path, step_name: str) -> list[dict[str, object]]:
    diagnostics_dir = run_root / "logs" / f"{step_name}_children"
    if diagnostics_dir.is_symlink() or not diagnostics_dir.is_dir():
        raise HarnessError(f"missing process child diagnostics directory: {step_name}")
    entries = []
    for path in sorted(diagnostics_dir.iterdir()):
        if path.is_symlink() or not path.is_file() or path.suffix != ".log":
            raise HarnessError(f"unexpected child diagnostics entry: {path}")
        entries.append({
            "path": path.relative_to(ROOT).as_posix(),
            "sha256": sha256_file(path),
            "size_bytes": path.stat().st_size,
        })
    if not entries or len(entries) % 2 != 0:
        raise HarnessError(f"process child diagnostics are incomplete: {step_name}")
    return entries


def require_process_marker(step: dict, marker: str, step_name: str) -> None:
    output = step.get("output_tail")
    if not isinstance(output, str) or marker not in output:
        raise HarnessError(f"process step omitted its success marker: {step_name}")


def process_artifact_record(path: Path) -> dict[str, object]:
    resolved = path.resolve()
    if ROOT not in resolved.parents or path.is_symlink() or not path.is_file():
        raise HarnessError(f"required signed process artifact is missing or escaped: {path}")
    return {
        "path": path.relative_to(ROOT).as_posix(),
        "sha256": sha256_file(path),
        "size_bytes": path.stat().st_size,
    }


def process_tree_record(path: Path) -> dict[str, object]:
    resolved = path.resolve()
    if ROOT not in resolved.parents or path.is_symlink() or not path.is_dir():
        raise HarnessError(f"required process bundle is missing or escaped: {path}")
    digest = hashlib.sha256()
    files = []
    for item in sorted(path.rglob("*")):
        if item.is_symlink():
            raise HarnessError(f"process bundle contains a symlink: {item}")
        if not item.is_file():
            continue
        relative = item.relative_to(path).as_posix()
        payload = item.read_bytes()
        digest.update(relative.encode("utf-8"))
        digest.update(b"\0")
        digest.update(len(payload).to_bytes(8, "big"))
        digest.update(payload)
        files.append({
            "path": item.relative_to(ROOT).as_posix(),
            "sha256": sha256_bytes(payload),
            "size_bytes": len(payload),
        })
    if not files:
        raise HarnessError(f"process bundle is empty: {path}")
    return {
        "path": path.relative_to(ROOT).as_posix(),
        "tree_sha256": digest.hexdigest(),
        "file_count": len(files),
        "files": files,
    }


def process_signature_output(step: dict, *, requires_jit: bool, step_name: str) -> None:
    output = step.get("output_tail")
    if not isinstance(output, str) or not output:
        raise HarnessError(f"codesign did not return signature evidence: {step_name}")
    if "runtime" not in output.lower():
        raise HarnessError(f"signed process artifact lacks Hardened Runtime evidence: {step_name}")
    has_allow_jit = "com.apple.security.cs.allow-jit" in output
    if requires_jit and (not has_allow_jit or re.search(r"<true\s*/>", output) is None):
        raise HarnessError(f"signed process artifact lacks allow-jit entitlement: {step_name}")
    if not requires_jit and has_allow_jit:
        raise HarnessError(f"launcher unexpectedly has the allow-jit entitlement: {step_name}")


def process_entitlements_output(step: dict, *, requires_jit: bool, step_name: str) -> None:
    output = step.get("output_tail")
    if not isinstance(output, str):
        raise HarnessError(f"codesign entitlement output is missing: {step_name}")
    has_allow_jit = "com.apple.security.cs.allow-jit" in output
    if requires_jit and (not has_allow_jit or re.search(r"<true\s*/>", output) is None):
        raise HarnessError(f"signed helper lacks allow-jit entitlement: {step_name}")
    if not requires_jit and has_allow_jit:
        raise HarnessError(f"launcher unexpectedly has allow-jit entitlement: {step_name}")


def collect_vmhost_process_artifacts(run_root: Path) -> dict[str, object]:
    paths = vmhost_process_paths(run_root)
    artifacts = {
        "od_host_library": process_artifact_record(paths["od_host_library"]),
        "vmhost": process_artifact_record(paths["vmhost"]),
        "launcher": process_artifact_record(paths["launcher"]),
        "fault_helper": process_artifact_record(paths["fault_helper"]),
        "entitlements_source": process_artifact_record(paths["entitlements"]),
    }
    if paths["app_bundle"].exists():
        bundle = process_tree_record(paths["app_bundle"])
        artifacts["app_bundle"] = bundle
        artifacts["bundle_vmhost"] = process_artifact_record(
            paths["app_bundle"] / "Contents/MacOS/od-vmhost-smoke"
        )
        artifacts["bundle_launcher"] = process_artifact_record(
            paths["app_bundle"] / "Contents/MacOS/OpenDesktopLauncher"
        )
    return artifacts


def process_binary_inputs(artifacts: dict[str, object], *names: str) -> dict[str, str]:
    result = {}
    for name in names:
        record = artifacts.get(name)
        if not isinstance(record, dict) or not isinstance(record.get("sha256"), str):
            raise HarnessError(f"missing process binary evidence: {name}")
        result[name] = record["sha256"]
    return result


def run_vmhost_process_suite(
    summary: dict, run_root: Path, fingerprint: str
) -> None:
    host_target, _ = rust_host()
    require_local_arm64_backend(host_target)
    paths = vmhost_process_paths(run_root)
    paths["artifacts"].mkdir(parents=True, exist_ok=False)
    paths["swift_modules"].mkdir(parents=True, exist_ok=False)
    codesign_path = "${CODESIGN_PATH}"
    artifacts: dict[str, object] = {}
    toolchain_outputs = {}

    for step_name in PROCESS_SUITE_STEPS:
        argv = vmhost_process_commands(run_root, codesign_path)[step_name]
        if step_name == "process_control_race":
            (run_root / "logs" / "process_control_race_children").mkdir()
        elif step_name == "process_isolation":
            (run_root / "logs" / "process_isolation_children").mkdir()
        elif step_name == "process_generated_fault":
            (run_root / "logs" / "process_generated_fault_children").mkdir()

        step = run_process_step(
            step_name,
            argv,
            run_root,
            PROCESS_STEP_TIMEOUTS[step_name],
            fingerprint,
        )
        step["suite"] = PROCESS_SUITE_NAME
        step["expected_returncode"] = 0
        if (
            step_name == "process_generated_fault"
            and "generated_fault_timeout_diagnostics=" in step.get("output_tail", "")
        ):
            step["runner_failure"] = step.get("failure")
            step["failure"] = "generated_fault_inner_timeout"
        if step_name in {"process_sw_vers", "process_uname", "process_rustc_version",
                         "process_cargo_version", "process_swift_version",
                         "process_xcode_version", "process_codesign_path"}:
            toolchain_outputs[step_name] = step["output_tail"]
            if step_name == "process_codesign_path" and step["status"] == "passed":
                tool_path_text = step["output_tail"].strip()
                tool_path = Path(tool_path_text)
                if not tool_path.is_absolute() or tool_path.is_symlink() or not tool_path.is_file():
                    step["status"] = "failed"
                    step["failure"] = "xcrun codesign path is invalid"
                else:
                    toolchain_outputs["codesign_binary"] = {
                        "path": str(tool_path),
                        "sha256": sha256_file(tool_path),
                        "size_bytes": tool_path.stat().st_size,
                    }
                    codesign_path = str(tool_path)

        if step_name in {
            "process_control_race", "process_isolation", "process_generated_fault"
        }:
            try:
                step["child_diagnostics"] = child_log_evidence(run_root, step_name)
            except HarnessError as error:
                step["child_diagnostics_failure"] = str(error)

        try:
            if step["status"] != "passed":
                raise HarnessError(
                    f"process command failed before semantic validation: {step.get('failure')}"
                )
            if step_name in {"process_control_race", "process_isolation",
                             "process_generated_fault", "process_app_bundle"}:
                if step_name == "process_control_race":
                    artifacts = collect_vmhost_process_artifacts(run_root)
                    step["binary_inputs"] = process_binary_inputs(
                        artifacts, "launcher", "vmhost"
                    )
                    require_process_marker(step, "VMHost control smoke passed:", step_name)
                elif step_name == "process_isolation":
                    step["binary_inputs"] = process_binary_inputs(artifacts, "vmhost")
                    require_process_marker(step, "VMHost isolation smoke passed:", step_name)
                elif step_name == "process_app_bundle":
                    bundle_path_marker = f"vmhost_app_bundle_path={paths['app_bundle']}"
                    require_process_marker(step, bundle_path_marker, step_name)
                    require_process_marker(step, "vmhost_app_bundle_status=passed", step_name)
                    artifacts = collect_vmhost_process_artifacts(run_root)
                    step["binary_inputs"] = process_binary_inputs(
                        artifacts, "bundle_launcher", "bundle_vmhost"
                    )
                else:
                    step["binary_inputs"] = process_binary_inputs(
                        artifacts, "launcher", "fault_helper"
                    )
                    observation = generated_fault_observation(step)
                    step["process_observation"] = observation
            elif step_name in {
                "process_vmhost_signature", "process_fault_helper_signature",
                "process_bundle_signature", "process_bundle_vmhost_signature",
                "process_bundle_launcher_signature",
            }:
                process_signature_output(step, requires_jit=False, step_name=step_name)
            elif step_name in {
                "process_vmhost_entitlements", "process_fault_helper_entitlements",
                "process_bundle_vmhost_entitlements",
            }:
                process_entitlements_output(step, requires_jit=True, step_name=step_name)
            elif step_name in {
                "process_launcher_entitlements", "process_bundle_launcher_entitlements",
            }:
                process_entitlements_output(step, requires_jit=False, step_name=step_name)
            elif step_name in {"process_sw_vers", "process_uname", "process_rustc_version",
                               "process_cargo_version", "process_swift_version",
                               "process_xcode_version", "process_codesign_path"}:
                if not step.get("output_tail"):
                    raise HarnessError(f"empty toolchain evidence: {step_name}")
            if step["status"] != "passed":
                raise HarnessError(f"process step failed: {step_name}: {step.get('failure')}")
        except HarnessError as error:
            if step["status"] == "passed":
                step["status"] = "failed"
                step["failure"] = str(error)
            else:
                step["evidence_validation_failure"] = str(error)

        summary["steps"][step_name] = step
        if artifacts:
            summary["process_artifacts"] = artifacts
        if toolchain_outputs:
            summary["process_toolchain"] = toolchain_outputs
        replace_json(run_root / "summary.json", summary)
        if step["status"] != "passed":
            raise HarnessError(f"required VMHost process step failed: {step_name}")

    artifacts = collect_vmhost_process_artifacts(run_root)
    summary["process_artifacts"] = artifacts
    summary["suites"][PROCESS_SUITE_NAME] = {
        "status": "passed",
        "steps": list(PROCESS_SUITE_STEPS),
        "artifacts": artifacts,
    }


def generated_fault_observation(step: dict) -> dict:
    output = step.get("output_tail")
    prefix = "generated_fault_observation="
    rows = [line[len(prefix):] for line in output.splitlines() if line.startswith(prefix)] \
        if isinstance(output, str) else []
    if len(rows) != 1:
        raise HarnessError("generated fault smoke did not preserve exactly one launcher observation")
    try:
        observation = json.loads(rows[0])
    except json.JSONDecodeError as error:
        raise HarnessError(f"generated fault launcher observation is malformed: {error}") from error
    if (
        not isinstance(observation, dict)
        or observation.get("launcher_returncode") != 1
        or observation.get("expected_reason") != "VMHost terminated by SIGSEGV"
        or not isinstance(observation.get("launcher_stdout"), str)
        or "VMHost terminated by SIGSEGV" not in observation.get("launcher_stderr", "")
    ):
        raise HarnessError("generated fault smoke did not prove the expected launcher SIGSEGV report")
    return observation
    deadline = time.monotonic() + 3
    while time.monotonic() < deadline:
        if not process_group_exists(process_group_id):
            return
        time.sleep(0.05)
    try:
        os.killpg(process_group_id, signal.SIGKILL)
    except ProcessLookupError:
        return


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
        "stage": "S3-34g-vmhost-process",
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
            "expected_unique_inputs": 3600,
            "replays_per_input": 2,
            "exact_gate": {
                "test_identity": MEMORY_EXACT_GATE,
                "argv": [
                    "cargo", "test", "--offline", "-p", "od-host", "--test",
                    "memory_corpus", MEMORY_EXACT_GATE, "--", "--exact", "--test-threads=1",
                ],
            },
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
        "vmhost_process": {
            "host_requirement": {
                "system": "darwin",
                "machine": "arm64",
                "rust_target_prefix": "aarch64-apple-darwin",
            },
            "build_profile": {
                "cargo": ["build", "--release", "-p", "od-host", "--offline"],
                "cargo_target_dir": "${RUN_ROOT}/build-target",
                "swift": ["xcrun", "swiftc", "-parse-as-library", "-O"],
                "swift_module_cache": "${RUN_ROOT}/swift-module-cache",
                "signing": [
                    "${CODESIGN_PATH}", "--force", "--sign", "-", "--options", "runtime"
                ],
                "entitlements": {
                    "path": "apps/vmhost/Configuration/vmhost-smoke.entitlements",
                    "sha256": sha256_file(
                        ROOT / "apps/vmhost/Configuration/vmhost-smoke.entitlements"
                    ),
                },
                "commands": vmhost_process_command_manifest(),
            },
            "inputs": [
                {"path": name, "sha256": sha256_file(ROOT / name)}
                for name in PROCESS_SOURCE_PATHS
            ],
            "success_markers": {
                "control_race": "VMHost control smoke passed:",
                "isolation": "VMHost isolation smoke passed:",
                "generated_fault": "VMHost terminated by SIGSEGV",
                "app_bundle": "vmhost_app_bundle_status=passed",
            },
            "automatic_retry": False,
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
    memory = value.get("memory_corpus")
    expected_memory_gate = {
        "test_identity": MEMORY_EXACT_GATE,
        "argv": [
            "cargo", "test", "--offline", "-p", "od-host", "--test",
            "memory_corpus", MEMORY_EXACT_GATE, "--", "--exact", "--test-threads=1",
        ],
    }
    if (
        not isinstance(memory, dict)
        or memory.get("source") != "crates/od-host/tests/memory_corpus.rs"
        or memory.get("source_sha256") != sha256_file(ROOT / "crates/od-host/tests/memory_corpus.rs")
        or memory.get("required_tests") != sorted(MEMORY_REQUIRED_TESTS)
        or memory.get("expected_unique_inputs") != 3600
        or memory.get("replays_per_input") != REPEATS
        or memory.get("exact_gate") != expected_memory_gate
    ):
        raise HarnessError("memory corpus manifest does not pin the S3-34h exact gate")
    validate_rust_test_manifest(value.get("local_rust_tests"))
    process = value.get("vmhost_process")
    if not isinstance(process, dict) or process.get("automatic_retry") is not False:
        raise HarnessError("VMHost process manifest is missing or permits automatic retry")
    if process.get("host_requirement") != {
        "system": "darwin", "machine": "arm64",
        "rust_target_prefix": "aarch64-apple-darwin",
    }:
        raise HarnessError("VMHost process host requirement differs")
    if process.get("build_profile", {}).get("commands") != vmhost_process_command_manifest():
        raise HarnessError("VMHost process command manifest differs from the exact run profile")
    inputs = process.get("inputs")
    expected_inputs = [
        {"path": name, "sha256": sha256_file(ROOT / name)}
        for name in PROCESS_SOURCE_PATHS
    ]
    if inputs != expected_inputs:
        raise HarnessError("VMHost process source inputs differ from the pinned manifest")
    return value


def validate_process_step_evidence(
    step: dict,
    step_name: str,
    run_root: Path,
    fingerprint: str,
    codesign_path: str = "${CODESIGN_PATH}",
) -> None:
    expected_argv = vmhost_process_commands(run_root, codesign_path).get(step_name)
    if expected_argv is None:
        raise HarnessError(f"unknown VMHost process step: {step_name}")
    if (
        step.get("name") != step_name
        or step.get("suite") != PROCESS_SUITE_NAME
        or step.get("argv") != expected_argv
        or step.get("command") != shlex.join(expected_argv)
        or step.get("status") != "passed"
        or step.get("returncode") != 0
        or step.get("expected_returncode") != 0
        or step.get("failure") is not None
        or step.get("timeout_seconds") != PROCESS_STEP_TIMEOUTS[step_name]
        or step.get("process_group_cleanup") != "empty"
        or step.get("source_fingerprint_after") != fingerprint
    ):
        raise HarnessError(f"VMHost process step evidence is missing, forged, or failed: {step_name}")
    duration = step.get("duration_seconds")
    if isinstance(duration, bool) or not isinstance(duration, (int, float)) or duration < 0:
        raise HarnessError(f"VMHost process step has invalid duration: {step_name}")

    logs = {
        "log": f"{step_name}.log",
        "stdout_log": f"{step_name}.stdout.log",
        "stderr_log": f"{step_name}.stderr.log",
    }
    contents: dict[str, bytes] = {}
    for field, basename in logs.items():
        expected_path = run_root / "logs" / basename
        relative = expected_path.relative_to(ROOT).as_posix()
        if step.get(field) != relative or expected_path.is_symlink() or not expected_path.is_file():
            raise HarnessError(f"VMHost process log path is invalid: {step_name}/{field}")
        contents[field] = expected_path.read_bytes()
    stdout = contents["stdout_log"].decode("utf-8", errors="replace")
    stderr = contents["stderr_log"].decode("utf-8", errors="replace")
    combined = stdout + ("\n--- stderr ---\n" + stderr if stderr else "")
    if (
        sha256_bytes(contents["log"]) != step.get("output_sha256")
        or sha256_bytes(contents["stdout_log"]) != step.get("stdout_sha256")
        or sha256_bytes(contents["stderr_log"]) != step.get("stderr_sha256")
        or contents["log"].decode("utf-8", errors="replace") != combined
        or step.get("output_tail") != combined[-4000:]
    ):
        raise HarnessError(f"VMHost process log digest or stream evidence differs: {step_name}")


def validate_process_child_logs(step: dict, step_name: str, run_root: Path) -> None:
    observed = child_log_evidence(run_root, step_name)
    if step.get("child_diagnostics") != observed:
        raise HarnessError(f"VMHost child diagnostics differ from saved files: {step_name}")
    if step_name == "process_isolation" and len(observed) != 4:
        raise HarnessError("VMHost isolation did not preserve both child stdout/stderr pairs")


def validate_vmhost_process_summary(summary: dict, manifest: dict, run_root: Path) -> None:
    if summary.get("status") != "vmhost_process_only" or summary.get("result_scope") != "vmhost_process_only":
        raise HarnessError("VMHost process suite must remain separate from local acceptance")
    suites = summary.get("suites", {})
    expected_local_status = {name: {"status": "not_run"} for name in LOCAL_SUITE_NAMES}
    for name, expected in expected_local_status.items():
        if suites.get(name) != expected:
            raise HarnessError(f"local acceptance suite must remain not_run during process suite: {name}")
    validate_unselected_suites(suites)

    steps = summary.get("steps", {})
    codesign_step = steps.get("process_codesign_path", {})
    codesign_path = codesign_step.get("output_tail", "").strip()
    if not Path(codesign_path).is_absolute():
        raise HarnessError("VMHost process codesign tool path is missing or not absolute")
    for name in LOCAL_STEPS:
        if steps.get(name, {}).get("status") != "not_run":
            raise HarnessError(f"local step ran during process-only suite: {name}")
    for name in PROCESS_SUITE_STEPS:
        step = steps.get(name)
        if not isinstance(step, dict):
            raise HarnessError(f"VMHost process step is missing: {name}")
        validate_process_step_evidence(
            step, name, run_root, summary["source"]["fingerprint_start"], codesign_path
        )

    process_suite = suites.get(PROCESS_SUITE_NAME)
    if not isinstance(process_suite, dict) or process_suite.get("status") != "passed" \
            or process_suite.get("steps") != list(PROCESS_SUITE_STEPS):
        raise HarnessError("VMHost process suite membership is missing or forged")

    expected_artifacts = collect_vmhost_process_artifacts(run_root)
    if summary.get("process_artifacts") != expected_artifacts \
            or process_suite.get("artifacts") != expected_artifacts:
        raise HarnessError("VMHost process artifact hashes differ from the current run outputs")
    if set(expected_artifacts) != {
        "od_host_library", "vmhost", "launcher", "fault_helper", "entitlements_source",
        "app_bundle", "bundle_vmhost", "bundle_launcher",
    }:
        raise HarnessError("VMHost process artifact set is incomplete")

    for name in ("process_vmhost_signature", "process_launcher_signature",
                 "process_fault_helper_signature", "process_bundle_signature",
                 "process_bundle_vmhost_signature", "process_bundle_launcher_signature"):
        process_signature_output(
            steps[name], requires_jit=False, step_name=name
        )
    for name in ("process_vmhost_entitlements", "process_fault_helper_entitlements",
                 "process_bundle_vmhost_entitlements"):
        process_entitlements_output(
            steps[name], requires_jit=True, step_name=name
        )
    for name in ("process_launcher_entitlements", "process_bundle_launcher_entitlements"):
        process_entitlements_output(
            steps[name], requires_jit=False, step_name=name
        )

    for name in (
        "process_control_race", "process_isolation", "process_generated_fault"
    ):
        validate_process_child_logs(steps[name], name, run_root)
    if len(steps["process_generated_fault"]["child_diagnostics"]) != 2:
        raise HarnessError("generated-fault launcher stdout/stderr diagnostics are incomplete")
    require_process_marker(steps["process_control_race"], "VMHost control smoke passed:", "process_control_race")
    require_process_marker(steps["process_isolation"], "VMHost isolation smoke passed:", "process_isolation")
    generated = generated_fault_observation(steps["process_generated_fault"])
    if steps["process_generated_fault"].get("process_observation") != generated:
        raise HarnessError("generated-fault process observation does not match captured output")
    bundle_path = vmhost_process_paths(run_root)["app_bundle"]
    require_process_marker(
        steps["process_app_bundle"], f"vmhost_app_bundle_path={bundle_path}", "process_app_bundle"
    )
    require_process_marker(steps["process_app_bundle"], "vmhost_app_bundle_status=passed", "process_app_bundle")

    expected_binary_inputs = {
        "process_control_race": process_binary_inputs(expected_artifacts, "launcher", "vmhost"),
        "process_isolation": process_binary_inputs(expected_artifacts, "vmhost"),
        "process_generated_fault": process_binary_inputs(expected_artifacts, "launcher", "fault_helper"),
        "process_app_bundle": process_binary_inputs(expected_artifacts, "bundle_launcher", "bundle_vmhost"),
    }
    for name, expected in expected_binary_inputs.items():
        if steps[name].get("binary_inputs") != expected:
            raise HarnessError(f"VMHost process binary binding differs: {name}")

    toolchain = summary.get("process_toolchain")
    expected_toolchain_steps = (
        "process_sw_vers", "process_uname", "process_rustc_version",
        "process_cargo_version", "process_swift_version", "process_xcode_version",
        "process_codesign_path",
    )
    expected_toolchain = {
        name: steps[name]["output_tail"] for name in expected_toolchain_steps
    }
    codesign_path = Path(steps["process_codesign_path"]["output_tail"].strip())
    if not codesign_path.is_absolute() or codesign_path.is_symlink() or not codesign_path.is_file():
        raise HarnessError("VMHost process codesign tool path is invalid")
    expected_toolchain["codesign_binary"] = {
        "path": str(codesign_path),
        "sha256": sha256_file(codesign_path),
        "size_bytes": codesign_path.stat().st_size,
    }
    if not isinstance(toolchain, dict) or toolchain != expected_toolchain:
        raise HarnessError("VMHost process toolchain evidence is missing or forged")
    process_manifest = manifest.get("vmhost_process")
    if not isinstance(process_manifest, dict) or process_manifest.get("automatic_retry") is not False:
        raise HarnessError("VMHost process manifest does not preserve the no-retry policy")


def validate_final_summary(summary: dict, input_manifest_path: Path) -> None:
    run_root = validate_run_id(summary.get("run_id"))
    digest = summary.get("input_manifest_sha256")
    if not isinstance(digest, str):
        raise HarnessError("input manifest digest is missing from summary")
    manifest = validate_input_manifest_file(input_manifest_path, digest)
    selection = summary.get("suite_selection")
    suites = summary.get("suites", {})
    steps = summary.get("steps", {})
    if selection == "local":
        validate_unselected_suites(suites)
        validate_unselected_suites(suites, (PROCESS_SUITE_NAME,))
        statuses = {name: steps.get(name, {}).get("status") for name in LOCAL_STEPS}
        validate_required_steps(statuses)
        validate_memory_exact_gate_evidence(steps["memory_s3_34h_exact_gate"], run_root)
        validate_required_rust_test_evidence(steps, run_root)
        memory_suite = suites.get("memory_corpus", {})
        if (
            memory_suite.get("status") != "passed"
            or memory_suite.get("unique_inputs") != 3600
            or memory_suite.get("replays_per_input") != REPEATS
            or memory_suite.get("test_outcomes") != steps.get("memory_corpus", {}).get("test_outcomes")
            or memory_suite.get("s3_34h_exact_gate") != steps["memory_s3_34h_exact_gate"]
        ):
            raise HarnessError("memory corpus suite summary omits or forges S3-34h evidence")
        lifecycle_steps = [
            step for step, suite, _, _ in RUST_TEST_GATES if suite == "local_lifecycle"
        ]
        acceptance_steps = [
            step for step, suite, _, _ in RUST_TEST_GATES if suite == "local_acceptance"
        ]
        if (
            suites.get("local_lifecycle") != {"status": "passed", "tests": lifecycle_steps}
            or suites.get("local_acceptance") != {"status": "passed", "tests": acceptance_steps}
            or summary.get("status") != "local_only"
            or summary.get("result_scope") != "local_only"
        ):
            raise HarnessError("required local suite membership or scope is incomplete or forged")
        if any(steps.get(name, {}).get("status") != "not_run" for name in PROCESS_SUITE_STEPS):
            raise HarnessError("VMHost process steps must remain not_run in the local suite")
        if suites.get(PROCESS_SUITE_NAME) != {"status": "not_run"}:
            raise HarnessError("VMHost process suite must remain not_run in the local suite")
    elif selection == "vmhost-process":
        validate_vmhost_process_summary(summary, manifest, run_root)
    else:
        raise HarnessError("Stage 3 suite selection is missing or unsupported")
    source = summary.get("source", {})
    validate_fingerprint(source.get("fingerprint_start", ""), source.get("fingerprint_end", ""))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", choices=("local", "vmhost-process"), default="local")
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
        "result_scope": "local_only" if args.suite == "local" else "vmhost_process_only",
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
        "suites": {name: {"status": "not_run"} for name in ALL_SUITES},
        "steps": {name: {"status": "not_run"} for name in ALL_STEPS},
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

        if args.suite == "vmhost-process":
            run_vmhost_process_suite(summary, run_dir, fingerprint)
            summary["source"]["fingerprint_end"] = source_fingerprint()
            validate_fingerprint(fingerprint, summary["source"]["fingerprint_end"])
            summary["status"] = "vmhost_process_only"
            validate_final_summary(summary, manifest_path)
            summary["finished_utc"] = time.strftime(
                "%Y-%m-%dT%H:%M:%SZ", time.gmtime()
            )
            replace_json(summary_path, summary)
            print(f"stage3_harness_status={summary['status']}")
            print(f"input_manifest_sha256={summary['input_manifest_sha256']}")
            print(f"summary_path={summary_path.relative_to(ROOT)}")
            return 0

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
            (
                "memory_s3_34h_exact_gate",
                [
                    "cargo", "test", "--offline", "-p", "od-host", "--test",
                    "memory_corpus", MEMORY_EXACT_GATE, "--", "--exact", "--test-threads=1",
                ],
                1800,
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
                summary["steps"][name]["unique_inputs"] = 3600
                summary["steps"][name]["replays_per_input"] = 2
                summary["steps"][name]["allowed_ignored"] = sorted(MEMORY_ALLOWED_IGNORED_TESTS)
                replace_json(summary_path, summary)
            if name == "memory_s3_34h_exact_gate":
                try:
                    outcomes = validate_exact_rust_test_output(
                        step["output_tail"], MEMORY_EXACT_GATE
                    )
                except HarnessError as error:
                    step["status"] = "failed"
                    step["failure"] = str(error)
                    summary["steps"][name] = step
                    replace_json(summary_path, summary)
                    raise
                summary["steps"][name]["test_identity"] = MEMORY_EXACT_GATE
                summary["steps"][name]["test_outcomes"] = outcomes
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
            "unique_inputs": 3600,
            "replays_per_input": 2,
            "test_outcomes": summary["steps"]["memory_corpus"]["test_outcomes"],
            "s3_34h_exact_gate": summary["steps"]["memory_s3_34h_exact_gate"],
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
