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
MEMORY_CORPUS_UNIQUE_INPUTS = 4800
RUN_ID_PATTERN = re.compile(r"stage3-\d{8}-\d{6}-[0-9a-f]{10}")
S3_30P3_GATE_STEP = "s3_30p3_allocator_helper_clobber_values"
S3_30P3_GATE_IDENTITY = "tests::allocated_register_and_spill_values_survive_helper_clobbers"

MEMORY_REQUIRED_TESTS = {
    "fixed_seed_memory_corpus_matches_interpreter_twice_per_input",
    "fixed_seed_s3_26f_store_store_corpus_matches_interpreter_twice_per_input",
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
    "test_run_scoped_artifacts_reuse_real_directory_and_reject_symlink",
    "test_source_fingerprint_mismatch_is_rejected",
    "test_input_manifest_digest_mismatch_is_rejected",
    "test_partial_run_cannot_be_reported_as_local_only",
    "test_unselected_suites_must_remain_not_run",
    "test_required_memory_test_must_be_present_and_non_ignored",
    "test_s3_34h_exact_gate_requires_exact_identity_and_argv",
    "test_s3_37e_memory_cache_gate_is_pinned_once_and_fail_closed",
    "test_s3_23_memory_pair_invalidation_gate_is_pinned_once_and_fail_closed",
    "test_s3_23_source_permission_gates_are_pinned_and_fail_closed",
    "test_s3_18g_dependency_generation_gate_is_pinned_once_and_fail_closed",
    "test_allowed_environment_replay_tests_do_not_hide_unexpected_ignored_tests",
    "test_self_test_output_validator_rejects_zero_match_and_all_skipped",
    "test_stage1_child_fingerprint_and_both_builds_are_pinned",
    "test_manual_frozen_input_and_result_bind_to_parent_manifest",
    "test_rust_test_manifest_rejects_a_missing_mandatory_gate",
    "test_s3_32a_candidate_gates_are_unique_and_fail_closed",
    "test_s3_33_native_gates_are_pinned_once_in_manifest_steps_and_summary",
    "test_s3_34n_native_register_corpus_gates_are_pinned_once",
    "test_s3_34p_cached_native_register_trace_gates_are_pinned_once",
    "test_s3_34q_retained_hit_reducer_gate_is_pinned_once",
    "test_s3_38f_session_proxy_gates_are_pinned_once_and_fail_closed",
    "test_s3_34r_cross_thread_idle_handle_gate_is_pinned_once_and_fail_closed",
    "test_s3_34s_memory_event_gates_are_pinned_once_and_fail_closed",
    "test_s3_34t_register_event_gates_are_pinned_once_and_fail_closed",
    "test_s3_34u_memory_load_and_pc_map_gates_are_pinned_once_and_fail_closed",
    "test_s3_26f_store_store_gates_are_pinned_once_and_fail_closed",
    "test_s3_34v_dword_store_gates_are_pinned_once_and_fail_closed",
    "test_s3_34w_dword_pc_map_gates_are_pinned_once_and_fail_closed",
    "test_s3_34x_dword_alias_load_gates_are_pinned_once_and_fail_closed",
    "test_s3_34y_dword_alias_store_gates_are_pinned_once_and_fail_closed",
    "test_s3_34z_rbx_disp8_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34aa_rbx_disp32_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34ab_rbx_disp8_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34ac_published_dword_disp8_disp32_load_maps_gate_is_pinned_and_fail_closed",
    "test_s3_34ad_rbx_disp32_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34ae_disp32_dword_store_boundary_gate_is_pinned_and_fail_closed",
    "test_s3_34af_dword_store_publication_gate_is_pinned_and_fail_closed",
    "test_s3_16o_store_store_not_rcx_mixed_corpus_gate_is_pinned_once_and_fail_closed",
    "test_s3_34ag_rip_relative_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34ah_rip_relative_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34ai_addr32_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34aj_addr32_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34ak_addr32_disp8_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34al_addr32_disp32_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34am_addr32_disp8_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34an_addr32_disp32_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34ao_rbx_rcx_sib_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34ap_rbx_rcx_sib_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34aq_rbx_rcx_sib_scaled_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34ar_rbx_rcx_sib_scaled_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34as_rbx_rcx_scaled_disp8_sib_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34at_rbx_rcx_scaled_disp8_sib_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34au_rbx_rcx_scaled_disp32_sib_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34av_rbx_rcx_scaled_disp32_sib_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34aw_addr32_scaled_sib_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34ax_addr32_scaled_sib_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34ay_addr32_scaled_disp8_sib_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34az_addr32_scaled_disp8_sib_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34ba_addr32_scaled_disp32_sib_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34bb_addr32_scaled_disp32_sib_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34bc_rbp_disp8_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34bd_rbp_disp8_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34be_rbp_disp32_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34bf_rbp_disp32_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34bg_addr32_rbp_disp8_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34bh_addr32_rbp_disp8_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34bi_addr32_rbp_disp32_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34bj_addr32_rbp_disp32_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34bk_rsp_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34bl_rsp_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34bm_rsp_disp8_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34bn_rsp_disp8_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34bo_rsp_disp32_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34bp_rsp_disp32_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34bq_addr32_rsp_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34br_addr32_rsp_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34bs_addr32_rsp_disp8_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34bt_addr32_rsp_disp8_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34bu_addr32_rsp_disp32_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34bv_addr32_rsp_disp32_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34bw_absolute_sib_disp32_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34bx_absolute_sib_disp32_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34by_addr32_absolute_sib_disp32_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34bz_addr32_absolute_sib_disp32_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34ca_rcx_index_no_base_sib_disp32_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34cb_rcx_index_no_base_sib_disp32_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34cc_rcx_index_no_base_sib_scaled_disp32_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34cd_rcx_index_no_base_sib_scaled_disp32_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34ce_addr32_rcx_index_no_base_sib_disp32_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34cf_addr32_rcx_index_no_base_sib_disp32_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34cg_addr32_rcx_index_no_base_sib_scaled_disp32_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34ch_addr32_rcx_index_no_base_sib_scaled_disp32_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34ci_rax_index_no_base_sib_disp32_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34cj_rax_index_no_base_sib_disp32_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34ck_rax_index_no_base_sib_scaled_disp32_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34cl_rax_index_no_base_sib_scaled_disp32_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34cm_addr32_rax_index_no_base_sib_disp32_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34cn_addr32_rax_index_no_base_sib_disp32_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34co_addr32_rax_index_no_base_sib_scaled_disp32_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34cp_addr32_rax_index_no_base_sib_scaled_disp32_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34cq_rax_disp8_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34cr_rax_disp8_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34cs_rax_disp32_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34ct_rax_disp32_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34cu_addr32_alias_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34cv_addr32_alias_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34cw_addr32_alias_disp8_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34cx_addr32_alias_disp8_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34cy_addr32_alias_disp32_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34cz_addr32_alias_disp32_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34da_rcx_base_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34db_rcx_base_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34dc_rcx_disp8_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34dd_rcx_disp8_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34de_rcx_disp32_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34df_rcx_disp32_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34dg_addr32_rcx_base_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34dh_addr32_rcx_base_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34di_addr32_rcx_disp8_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34dj_addr32_rcx_disp8_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34dk_addr32_rcx_disp32_dword_load_gates_are_pinned_and_fail_closed",
    "test_s3_34dl_addr32_rcx_disp32_dword_store_gates_are_pinned_and_fail_closed",
    "test_s3_34dm_addr32_rsp_dword_store_corpus_gate_is_pinned_and_fail_closed",
    "test_s3_34dn_replay_artifact_self_check_gate_is_pinned_and_fail_closed",
    "test_s3_34do_value_only_reducer_self_check_gate_is_pinned_and_fail_closed",
    "test_s3_34dr_load_load_profile_gates_are_pinned_and_fail_closed",
    "test_s3_38b_native_register_cache_gates_are_pinned_once",
    "test_s3_55_mixed_dispatch_gates_are_pinned_and_fail_closed",
    "test_s3_38e_session_lifecycle_gates_are_pinned_once_and_fail_closed",
    "test_s3_38d_result_state_validator_rejects_corruption",
    "test_s3_34o_reducer_artifact_inventory_rejects_tampering_and_symlinks",
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
    "test_vmhost_process_manifest_self_check_rejects_code_mismatch_drift",
    "test_vmhost_process_v1_isolation_gate_is_pinned_once_and_fails_closed",
    "test_vmhost_process_run_stop_id_gate_is_pinned_once_and_fails_closed",
    "test_vmhost_process_run_id_exhaustion_gate_is_pinned_once_and_fails_closed",
    "test_generated_fault_smoke_cleans_child_group_on_success_and_failure",
    "test_app_bundle_smoke_preserves_and_executes_five_argument_bundle",
    "test_vmhost_process_timeout_kills_child_group",
    "test_s3_30p3_allocator_helper_clobber_gate_is_pinned_and_fails_closed",
    "test_s3_16_not_rcx_fallback_gate_is_pinned_and_fail_closed",
    "test_s3_16_publication_refusal_gate_is_pinned",
    "test_s3_27_fault_boundary_gates_are_pinned_and_fail_closed",
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
    "ir_two_store_pair_preserves_callbacks_faults_and_continuation",
)

ADDITIONAL_TEST_GATES = (
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
    (
        "s3_27_pc_map_snapshot_boundary", "local_acceptance", "od-jit",
        "statepoints::tests::maps_cover_code_with_checked_half_open_ranges_and_separate_boundaries",
    ),
    (
        "s3_27_uncached_memory_fault_boundary", "local_acceptance", "od-jit",
        "memory_dispatch::tests::guest_fault_requires_pending_memory_site",
    ),
    (
        "s3_27_cached_memory_fault_boundary", "local_acceptance", "od-jit",
        "memory_cache::tests::guest_fault_requires_pending_cached_memory_site",
    ),
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
        "s3_38b_native_register_v1_cache_profile_invalidation", "local_acceptance", "od-jit",
        "dispatch::tests::native_cached_profile_isolated_and_dependency_context_changes_miss",
    ),
    (
        "s3_38b_native_register_v1_cache_guards_capacity", "local_acceptance", "od-jit",
        "dispatch::tests::native_cached_guards_and_failures_preserve_publication_and_entry_counts",
    ),
    (
        "s3_38b_native_register_v1_host_reuse_lifetime_parity", "local_acceptance", "od-host",
        "tests::cached_native_register_v1_reuses_eager_block_with_full_state_parity",
    ),
    (
        "s3_38c_native_register_v1_strategy_key_isolation", "local_acceptance", "od-jit",
        "dispatch::tests::native_cached_demand_candidate_strategy_key_isolated",
    ),
    (
        "s3_38c_native_register_v1_candidate_lifecycle_guards", "local_acceptance", "od-jit",
        "dispatch::tests::native_cached_demand_candidate_guards_and_lifetime",
    ),
    (
        "s3_38c_native_register_v1_candidate_cold_hit_parity", "local_acceptance", "od-host",
        "tests::cached_native_register_v1_demand_candidate_cold_hit_full_state_parity",
    ),
    (
        "s3_55_mixed_witness_exact", "local_acceptance", "od-jit",
        "block::s3_52_admission_equivalence_tests::s3_55_mixed_witness_preserves_strict_admission_and_exact_helper_forms",
    ),
    (
        "s3_55_helper_control_and_native_progress", "local_acceptance", "od-jit",
        "dispatch::tests::s3_55_helper_only_budget_stop_event_and_zero_progress",
    ),
    (
        "s3_55_post_helper_snapshot_guards", "local_acceptance", "od-jit",
        "dispatch::tests::s3_55_post_helper_validation_rejects_changed_source_and_invalid_dependencies",
    ),
    (
        "s3_55_interpreter_zero_progress", "local_acceptance", "od-jit",
        "dispatch::tests::s3_55_interpreter_zero_progress_is_typed_and_not_retired",
    ),
    (
        "s3_55_interpreter_error_retirement", "local_acceptance", "od-jit",
        "dispatch::tests::s3_55_interpreter_failure_does_not_retire",
    ),
    (
        "s3_55_mixed_native_full_state", "local_acceptance", "od-host",
        "tests::s3_54_native_multi_cache_reuses_published_handle",
    ),
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
    (
        "s3_34v_ir_dword_store_admission_provenance", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_store_admits_only_exact_8903_and_preserves_its_provenance",
    ),
    (
        "s3_34v_ir_dword_store_commits_one_four_byte_write", "local_acceptance", "od-host",
        "tests::s3_26e_ir_dword_store_matches_interpreter_and_commits_one_four_byte_write",
    ),
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
    (
        "s3_33m_native_lea_rax_rip_disp32_admission", "local_acceptance", "od-jit",
        "block::tests::lea_rax_rip_disp32_admission_is_exact_and_uses_signed_decoded_disp",
    ),
    (
        "s3_33m_native_lea_rax_rip_disp32_compiler", "local_acceptance", "od-jit",
        "block_compile::tests::native_lea_rax_rip_disp32_compiles_eager_and_candidate_from_guest_next_rip",
    ),
    (
        "s3_33m_native_lea_rax_rip_disp32_host", "local_acceptance", "od-host",
        "tests::native_lea_rax_rip_disp32_matches_interpreter_without_data_access",
    ),
    (
        "s3_33n_native_register_v1_admission", "local_acceptance", "od-jit",
        "block::tests::native_register_v1_admission_is_exact_and_combines_all_committed_forms",
    ),
    (
        "s3_33n_native_register_v1_compiler", "local_acceptance", "od-jit",
        "block_compile::tests::native_register_v1_mixed_compiler_preserves_statepoints_and_boundaries",
    ),
    (
        "s3_33n_native_register_v1_host", "local_acceptance", "od-host",
        "tests::native_register_v1_mixed_matches_interpreter_and_bounded_ordered_pair_corpus",
    ),
    (
        "s3_34n_native_register_v1_corpus", "local_acceptance", "od-host",
        "tests::native_register_v1_corpus::generated_corpus_runs_twice_across_all_backends",
    ),
    (
        "s3_34n_native_register_v1_replay", "local_acceptance", "od-host",
        "tests::native_register_v1_corpus::saved_case_round_trip_and_replay",
    ),
    (
        "s3_34n_native_register_v1_selfcheck", "local_acceptance", "od-host",
        "tests::native_register_v1_corpus::comparator_and_envelope_fail_closed",
    ),
    (
        "s3_34o_native_register_v1_reducer", "local_acceptance", "od-host",
        "tests::native_register_v1_corpus::reduce_saved_native_register_v1_case",
    ),
    (
        "s3_34o_native_register_v1_reducer_selfcheck", "local_acceptance", "od-host",
        "tests::native_register_v1_corpus::native_register_v1_reducer_rejects_unstable_and_structural_changes",
    ),
    (
        "s3_34p_cached_native_register_v1_corpus", "local_acceptance", "od-host",
        "tests::native_register_v1_corpus::cached_corpus_matches_existing_observations_and_saves_versioned_trace",
    ),
    (
        "s3_34p_cached_native_register_v1_replay", "local_acceptance", "od-host",
        "tests::native_register_v1_corpus::cached_trace_replays_from_empty_cache_and_rejects_corruption",
    ),
    (
        "s3_34p_cached_native_register_v1_failure_replay", "local_acceptance", "od-host",
        "tests::native_register_v1_corpus::cached_failure_trace_replays_saved_prefix_and_failure",
    ),
    (
        "s3_34q_retained_hit_state_reducer_selfcheck", "local_acceptance", "od-host",
        "tests::native_register_v1_corpus::retained_hit_failure_reducer_and_selfcheck",
    ),
    (
        "s3_34r_abi_idle_handle_cross_thread_mapping_release", "local_acceptance", "od-host",
        "tests::c_abi_idle_handle_runs_sequentially_on_host_threads_and_releases_mapping",
    ),
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
    (
        S3_30P3_GATE_STEP, "local_acceptance", "od-host", S3_30P3_GATE_IDENTITY,
    ),
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
    (
        "s3_37e_memory_cache_lifecycle_republishes", "local_acceptance", "od-host",
        "capacity_one_memory_cache_republishes_after_fault_and_backend_error",
    ),
    (
        "s3_37f_session_cache_pressure", "local_lifecycle", "od-host",
        "session::tests::cache_pressure_preserves_state_and_releases_the_arena",
    ),
    (
        "s3_23_memory_pair_cache_invalidation_republishes",
        "local_acceptance",
        "od-host",
        "tests::memory_pair_cache_invalidation_drops_mapping_before_republish",
    ),
    (
        "s3_23_tracked_v2_source_permission", "local_acceptance", "od-host",
        "tests::s3_23_tracked_v2_rejects_revoked_source_permission",
    ),
    (
        "s3_23_tracked_v3_source_permission", "local_acceptance", "od-host",
        "tests::s3_23_tracked_v3_rejects_revoked_source_permission",
    ),
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
    (
        "s3_34z_ir_dword_rbx_disp8_load_site_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rbx_disp8_load_binds_all_signed_source_bytes_to_its_site",
    ),
    (
        "s3_34z_host_disp8_dword_load_zero_extend_four_bytes", "local_acceptance", "od-host",
        "tests::s3_20cr_disp8_dword_load_zero_extends_and_reads_exactly_four_bytes",
    ),
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
    (
        "s3_34ac_published_dword_disp_load_maps_lifetime", "local_acceptance", "od-host",
        "published_dword_disp8_and_disp32_load_pc_maps_match_artifacts_survive_drop_and_republish",
    ),
    (
        "s3_34ad_ir_dword_rbx_disp32_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rbx_disp32_store_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34ad_ir_dword_rbx_disp32_store_pc_map_six_byte_boundary", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rbx_disp32_store_pc_map_uses_six_byte_guest_boundary",
    ),
    (
        "s3_34ad_host_dword_rbx_disp32_store_checked_semantics", "local_acceptance", "od-host",
        "tests::s3_20cu_disp32_dword_store_preserves_checked_semantics",
    ),
    (
        "s3_34ae_host_disp32_dword_store_canonical_end_range_overflow", "local_acceptance", "od-host",
        "tests::s3_27i_disp32_dword_store_checks_canonical_end_and_range_overflow",
    ),
    (
        "s3_34af_published_dword_disp8_disp32_store_maps_lifetime", "local_acceptance", "od-host",
        "published_dword_disp8_and_disp32_store_pc_maps_match_artifacts_survive_drop_and_republish",
    ),
    (
        "s3_34ag_ir_dword_rip_relative_disp32_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rip_relative_disp32_load_binds_all_source_bytes_to_next_rip",
    ),
    (
        "s3_34ag_ir_dword_rip_relative_disp32_load_pc_map_six_byte_boundary", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rip_relative_disp32_load_pc_map_uses_six_byte_guest_boundary",
    ),
    (
        "s3_34ag_host_rip_relative_disp32_dword_load_guest_next_rip", "local_acceptance", "od-host",
        "tests::s3_20cv_rip_relative_disp32_dword_load_uses_guest_next_rip",
    ),
    (
        "s3_34ah_ir_dword_rip_relative_disp32_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rip_relative_disp32_store_binds_all_source_bytes_to_next_rip",
    ),
    (
        "s3_34ah_ir_dword_rip_relative_disp32_store_pc_map_six_byte_boundary", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rip_relative_disp32_store_pc_map_uses_six_byte_guest_boundary",
    ),
    (
        "s3_34ah_host_rip_relative_disp32_dword_store_guest_next_rip", "local_acceptance", "od-host",
        "tests::s3_20cw_rip_relative_disp32_dword_store_uses_guest_next_rip",
    ),
    (
        "s3_34ai_ir_dword_addr32_load_exact_admission_provenance", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_load_admits_only_67_8b_03_and_preserves_its_provenance",
    ),
    (
        "s3_34ai_ir_dword_addr32_load_pc_map_three_byte_boundary", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_load_pc_map_uses_three_byte_guest_boundary",
    ),
    (
        "s3_34ai_host_addr32_dword_load_zero_extended_ebx", "local_acceptance", "od-host",
        "tests::s3_20cx_addr32_dword_load_uses_zero_extended_ebx",
    ),
    (
        "s3_34aj_ir_dword_addr32_store_exact_admission_provenance", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_store_admits_only_67_89_03_and_preserves_its_provenance",
    ),
    (
        "s3_34aj_host_addr32_dword_store_zero_extended_ebx", "local_acceptance", "od-host",
        "tests::s3_20cy_addr32_dword_store_uses_zero_extended_ebx",
    ),
    (
        "s3_34ak_ir_dword_addr32_disp8_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_disp8_load_binds_all_signed_source_bytes_to_its_site",
    ),
    (
        "s3_34ak_host_addr32_disp8_dword_load_wrapped_ebx", "local_acceptance", "od-host",
        "tests::s3_20cz_addr32_disp8_dword_load_uses_wrapped_ebx",
    ),
    (
        "s3_34al_ir_dword_addr32_disp32_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_disp32_load_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34al_host_addr32_disp32_dword_load_wrapped_ebx", "local_acceptance", "od-host",
        "tests::s3_20da_addr32_disp32_dword_load_uses_wrapped_ebx",
    ),
    (
        "s3_34am_ir_dword_addr32_disp8_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_disp8_store_binds_all_signed_source_bytes_to_its_site",
    ),
    (
        "s3_34am_host_addr32_disp8_dword_store_wrapped_ebx", "local_acceptance", "od-host",
        "tests::s3_20db_addr32_disp8_dword_store_uses_wrapped_ebx",
    ),
    (
        "s3_34an_ir_dword_addr32_disp32_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_disp32_store_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34an_host_addr32_disp32_dword_store_wrapped_ebx", "local_acceptance", "od-host",
        "tests::s3_20dc_addr32_disp32_dword_store_uses_wrapped_ebx",
    ),
    (
        "s3_34ao_ir_dword_rbx_rcx_sib_load_exact_admission", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rbx_rcx_sib_load_admits_only_exact_8b040b",
    ),
    (
        "s3_34ao_host_rbx_rcx_sib_dword_load_full_width_base_index", "local_acceptance", "od-host",
        "tests::s3_20dd_rbx_rcx_sib_dword_load_uses_full_width_base_and_index",
    ),
    (
        "s3_34ap_ir_dword_rbx_rcx_sib_store_exact_admission", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rbx_rcx_sib_store_admits_only_exact_89040b",
    ),
    (
        "s3_34ap_host_rbx_rcx_sib_dword_store_full_width_base_index", "local_acceptance", "od-host",
        "tests::s3_20de_rbx_rcx_sib_dword_store_uses_full_width_base_and_index",
    ),
    (
        "s3_34aq_ir_dword_rbx_rcx_sib_scaled_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rbx_rcx_sib_dword_load_binds_exact_scales_2_4_8",
    ),
    (
        "s3_34aq_host_rbx_rcx_sib_scaled_dword_load_full_width_base_index", "local_acceptance", "od-host",
        "tests::s3_20df_rbx_rcx_sib_scaled_dword_load_uses_full_width_base_and_index",
    ),
    (
        "s3_34ar_ir_dword_rbx_rcx_sib_scaled_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rbx_rcx_sib_store_binds_exact_scales_2_4_8",
    ),
    (
        "s3_34ar_host_rbx_rcx_scaled_sib_dword_store_full_width_base_index", "local_acceptance", "od-host",
        "tests::s3_20dg_rbx_rcx_scaled_sib_dword_store_uses_full_width_base_and_index",
    ),
    (
        "s3_34as_ir_dword_rbx_rcx_sib_disp8_scaled_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rbx_rcx_sib_disp8_load_binds_all_scales_and_signed_bytes",
    ),
    (
        "s3_34as_host_rbx_rcx_scaled_disp8_sib_dword_load_signed_displacement", "local_acceptance", "od-host",
        "tests::s3_20dh_rbx_rcx_scaled_disp8_sib_dword_load_uses_signed_displacement",
    ),
    (
        "s3_34at_ir_dword_rbx_rcx_sib_disp8_scaled_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rbx_rcx_sib_disp8_store_binds_all_scales_and_signed_bytes",
    ),
    (
        "s3_34at_host_rbx_rcx_scaled_disp8_sib_dword_store_checked_semantics", "local_acceptance", "od-host",
        "tests::s3_20di_rbx_rcx_scaled_disp8_sib_dword_store_preserves_checked_semantics",
    ),
    (
        "s3_34au_ir_dword_rbx_rcx_sib_disp32_scaled_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rbx_rcx_sib_disp32_load_binds_all_scales_and_source_bytes",
    ),
    (
        "s3_34au_host_rbx_rcx_scaled_disp32_sib_dword_load_checked_semantics", "local_acceptance", "od-host",
        "tests::s3_20dj_rbx_rcx_scaled_disp32_sib_dword_load_preserves_checked_semantics",
    ),
    (
        "s3_34av_ir_dword_rbx_rcx_sib_disp32_scaled_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rbx_rcx_sib_disp32_store_binds_all_scales_and_source_bytes",
    ),
    (
        "s3_34av_host_rbx_rcx_scaled_disp32_sib_dword_store_checked_semantics", "local_acceptance", "od-host",
        "tests::s3_20dk_rbx_rcx_scaled_disp32_sib_dword_store_preserves_checked_semantics",
    ),
    (
        "s3_34aw_ir_dword_addr32_rbx_rcx_sib_scaled_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rbx_rcx_sib_load_binds_exact_scales_and_prefix",
    ),
    (
        "s3_34aw_host_addr32_scaled_sib_dword_load_wraps_zero_extends", "local_acceptance", "od-host",
        "tests::s3_20dl_addr32_scaled_sib_dword_load_wraps_and_zero_extends",
    ),
    (
        "s3_34ax_ir_dword_addr32_rbx_rcx_sib_scaled_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rbx_rcx_sib_store_binds_exact_scales_and_prefix",
    ),
    (
        "s3_34ax_host_addr32_scaled_sib_dword_store_wraps_preserves_state", "local_acceptance", "od-host",
        "tests::s3_20dm_addr32_scaled_sib_dword_store_wraps_and_preserves_state",
    ),
    (
        "s3_34ay_ir_dword_addr32_rbx_rcx_sib_disp8_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rbx_rcx_sib_disp8_load_binds_all_scales_and_signed_bytes",
    ),
    (
        "s3_34ay_host_addr32_scaled_disp8_sib_dword_load_signed_displacement", "local_acceptance", "od-host",
        "tests::s3_20dn_addr32_scaled_disp8_sib_dword_load_uses_signed_displacement",
    ),
    (
        "s3_34az_ir_dword_addr32_rbx_rcx_sib_disp8_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rbx_rcx_sib_disp8_store_binds_all_scales_and_signed_bytes",
    ),
    (
        "s3_34az_host_addr32_scaled_disp8_sib_dword_store_wraps_preserves_state", "local_acceptance", "od-host",
        "tests::s3_20do_addr32_scaled_disp8_sib_dword_store_wraps_and_preserves_state",
    ),
    (
        "s3_34ba_ir_dword_addr32_rbx_rcx_sib_disp32_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rbx_rcx_sib_disp32_load_binds_scales_and_all_displacement_bytes",
    ),
    (
        "s3_34ba_host_addr32_scaled_disp32_sib_dword_load_wraps_preserves_state", "local_acceptance", "od-host",
        "tests::s3_20dp_addr32_scaled_disp32_sib_dword_load_wraps_and_preserves_state",
    ),
    (
        "s3_34bb_ir_dword_addr32_rbx_rcx_sib_disp32_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rbx_rcx_sib_disp32_store_binds_scales_and_all_displacement_bytes",
    ),
    (
        "s3_34bb_host_addr32_scaled_disp32_sib_dword_store_wraps_preserves_state", "local_acceptance", "od-host",
        "tests::s3_20dq_addr32_scaled_disp32_sib_dword_store_wraps_and_preserves_state",
    ),
    (
        "s3_34bc_ir_dword_rbp_disp8_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rbp_disp8_load_binds_all_signed_source_bytes_to_its_site",
    ),
    (
        "s3_34bc_host_rbp_disp8_dword_load_checked_semantics", "local_acceptance", "od-host",
        "tests::s3_20dr_rbp_disp8_dword_load_preserves_ss_fault_and_checked_state",
    ),
    (
        "s3_34bg_ir_dword_addr32_rbp_disp8_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rbp_disp8_load_binds_all_signed_source_bytes_to_its_site",
    ),
    (
        "s3_34bg_host_addr32_rbp_disp8_dword_load_checked_state", "local_acceptance", "od-host",
        "tests::s3_20dv_addr32_rbp_disp8_dword_load_preserves_checked_state",
    ),
    (
        "s3_34bd_ir_dword_rbp_disp8_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rbp_disp8_store_binds_all_signed_source_bytes_to_its_site",
    ),
    (
        "s3_34bd_host_rbp_disp8_dword_store_checked_semantics", "local_acceptance", "od-host",
        "tests::s3_20ds_rbp_disp8_dword_store_preserves_ss_fault_and_checked_state",
    ),
    (
        "s3_34be_ir_dword_rbp_disp32_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rbp_disp32_load_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34be_host_rbp_disp32_dword_load_checked_semantics", "local_acceptance", "od-host",
        "tests::s3_20dt_rbp_disp32_dword_load_preserves_ss_fault_and_checked_state",
    ),
    (
        "s3_34bf_ir_dword_rbp_disp32_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rbp_disp32_store_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34bf_host_rbp_disp32_dword_store_checked_semantics", "local_acceptance", "od-host",
        "tests::s3_20du_rbp_disp32_dword_store_preserves_ss_fault_and_checked_state",
    ),
    (
        "s3_34bh_ir_addr32_rbp_disp8_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rbp_disp8_store_binds_all_signed_source_bytes_to_its_site",
    ),
    (
        "s3_34bh_host_addr32_rbp_disp8_dword_store_wrapped_ebp", "local_acceptance", "od-host",
        "tests::s3_20dw_addr32_rbp_disp8_dword_store_uses_wrapped_ebp",
    ),
    (
        "s3_34bi_ir_addr32_rbp_disp32_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rbp_disp32_load_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34bi_host_addr32_rbp_disp32_dword_load_checked_state", "local_acceptance", "od-host",
        "tests::s3_20dx_addr32_rbp_disp32_dword_load_preserves_checked_state",
    ),
    (
        "s3_34bj_ir_addr32_rbp_disp32_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rbp_disp32_store_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34bj_host_addr32_rbp_disp32_dword_store_wrapped_ebp", "local_acceptance", "od-host",
        "tests::s3_20dy_addr32_rbp_disp32_dword_store_uses_wrapped_ebp",
    ),
    (
        "s3_34bk_ir_rsp_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rsp_sib_load_binds_exact_source_and_site",
    ),
    (
        "s3_34bk_host_rsp_dword_load_no_replay_retry_checked_state", "local_acceptance", "od-host",
        "tests::s3_20dz_rsp_dword_load_preserves_ss_fault_and_checked_state",
    ),
    (
        "s3_34bl_ir_rsp_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rsp_sib_store_binds_exact_source_and_site",
    ),
    (
        "s3_34bl_host_rsp_dword_store_checked_state", "local_acceptance", "od-host",
        "tests::s3_20ea_rsp_dword_store_preserves_ss_fault_and_checked_state",
    ),
    (
        "s3_34bm_ir_rsp_disp8_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rsp_sib_disp8_load_binds_all_signed_source_bytes_to_its_site",
    ),
    (
        "s3_34bm_host_rsp_disp8_dword_load_checked_state", "local_acceptance", "od-host",
        "tests::s3_20eb_rsp_disp8_dword_load_preserves_ss_fault_and_checked_state",
    ),
    (
        "s3_34bn_ir_rsp_disp8_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rsp_sib_disp8_store_binds_all_signed_source_bytes_to_its_site",
    ),
    (
        "s3_34bn_host_rsp_disp8_dword_store_checked_state", "local_acceptance", "od-host",
        "tests::s3_20ec_rsp_disp8_dword_store_preserves_ss_fault_and_checked_state",
    ),
    (
        "s3_34bo_ir_rsp_disp32_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rsp_sib_disp32_load_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34bo_host_rsp_disp32_dword_load_checked_state", "local_acceptance", "od-host",
        "tests::s3_20ed_rsp_disp32_dword_load_preserves_ss_fault_and_checked_state",
    ),
    (
        "s3_34bp_ir_rsp_disp32_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rsp_sib_disp32_store_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34bp_host_rsp_disp32_dword_store_checked_state", "local_acceptance", "od-host",
        "tests::s3_20ee_rsp_disp32_dword_store_preserves_ss_fault_and_checked_state",
    ),
    (
        "s3_34bq_ir_addr32_rsp_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rsp_sib_load_binds_exact_source_and_site",
    ),
    (
        "s3_34bq_host_addr32_rsp_dword_load_zero_extended_esp", "local_acceptance", "od-host",
        "tests::s3_20ef_addr32_rsp_dword_load_uses_zero_extended_esp",
    ),
    (
        "s3_34br_ir_addr32_rsp_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rsp_sib_store_binds_exact_source_and_site",
    ),
    (
        "s3_34br_host_addr32_rsp_dword_store_zero_extended_esp", "local_acceptance", "od-host",
        "tests::s3_20eg_addr32_rsp_dword_store_uses_zero_extended_esp",
    ),
    (
        "s3_34bs_ir_addr32_rsp_disp8_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rsp_sib_disp8_load_binds_all_signed_source_bytes_to_its_site",
    ),
    (
        "s3_34bs_host_addr32_rsp_disp8_dword_load_wrapped_esp", "local_acceptance", "od-host",
        "tests::s3_20eh_addr32_rsp_disp8_dword_load_uses_wrapped_esp",
    ),
    (
        "s3_34bt_ir_addr32_rsp_disp8_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rsp_sib_disp8_store_binds_all_signed_source_bytes_to_its_site",
    ),
    (
        "s3_34bt_host_addr32_rsp_disp8_dword_store_wrapped_esp", "local_acceptance", "od-host",
        "tests::s3_20ei_addr32_rsp_disp8_dword_store_uses_wrapped_esp",
    ),
    (
        "s3_34bu_ir_addr32_rsp_disp32_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rsp_sib_disp32_load_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34bu_host_addr32_rsp_disp32_dword_load_wrapped_esp", "local_acceptance", "od-host",
        "tests::s3_20ej_addr32_rsp_disp32_dword_load_uses_wrapped_esp",
    ),
    (
        "s3_34bv_ir_addr32_rsp_disp32_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rsp_sib_disp32_store_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34bv_host_addr32_rsp_disp32_dword_store_wrapped_esp", "local_acceptance", "od-host",
        "tests::s3_20ek_addr32_rsp_disp32_dword_store_uses_wrapped_esp",
    ),
    (
        "s3_34bw_ir_absolute_sib_disp32_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_absolute_sib_disp32_load_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34bw_host_absolute_sib_disp32_dword_load_signed_absolute_address", "local_acceptance", "od-host",
        "tests::s3_20el_absolute_sib_disp32_dword_load_uses_signed_absolute_address",
    ),
    (
        "s3_34bx_ir_absolute_sib_disp32_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_absolute_sib_disp32_store_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34bx_host_absolute_sib_disp32_dword_store_signed_absolute_address", "local_acceptance", "od-host",
        "tests::s3_20em_absolute_sib_disp32_dword_store_uses_signed_absolute_address",
    ),
    (
        "s3_34by_ir_addr32_absolute_sib_disp32_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_absolute_sib_disp32_load_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34by_host_addr32_absolute_sib_disp32_dword_load_zero_extended_absolute_address", "local_acceptance", "od-host",
        "tests::s3_20en_addr32_absolute_sib_disp32_dword_load_uses_zero_extended_absolute_address",
    ),
    (
        "s3_34bz_ir_addr32_absolute_sib_disp32_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_absolute_sib_disp32_store_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34bz_host_addr32_absolute_sib_disp32_dword_store_zero_extended_absolute_address", "local_acceptance", "od-host",
        "tests::s3_20eo_addr32_absolute_sib_disp32_dword_store_uses_zero_extended_absolute_address",
    ),
    (
        "s3_34ca_ir_rcx_index_no_base_sib_disp32_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rcx_index_no_base_sib_disp32_load_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34ca_host_rcx_index_no_base_sib_disp32_dword_load_signed_displacement", "local_acceptance", "od-host",
        "tests::s3_20ep_rcx_index_no_base_sib_disp32_dword_load_uses_signed_displacement",
    ),
    (
        "s3_34cb_ir_rcx_index_no_base_sib_disp32_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rcx_index_no_base_sib_disp32_store_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34cb_host_rcx_index_no_base_sib_disp32_dword_store_signed_displacement", "local_acceptance", "od-host",
        "tests::s3_20eq_rcx_index_no_base_sib_disp32_dword_store_uses_signed_displacement",
    ),
    (
        "s3_34cc_ir_rcx_index_no_base_sib_scaled_disp32_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rcx_index_no_base_sib_scaled_disp32_load_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34cc_host_rcx_index_no_base_sib_scaled_disp32_dword_load_full_width_scaled_index", "local_acceptance", "od-host",
        "tests::s3_20er_rcx_index_no_base_sib_scaled_disp32_dword_load_uses_full_width_scaled_index",
    ),
    (
        "s3_34cd_ir_rcx_index_no_base_sib_scaled_disp32_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rcx_index_no_base_sib_scaled_disp32_store_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34cd_host_rcx_index_no_base_sib_disp32_dword_store_full_width_scaled_index", "local_acceptance", "od-host",
        "tests::s3_20es_rcx_index_no_base_sib_scaled_disp32_dword_store_uses_full_width_scaled_index",
    ),
    (
        "s3_34ce_ir_addr32_rcx_index_no_base_sib_disp32_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rcx_index_no_base_sib_disp32_load_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34ce_host_addr32_rcx_index_no_base_sib_disp32_dword_load_wrapped_ecx", "local_acceptance", "od-host",
        "tests::s3_20et_addr32_rcx_index_no_base_sib_disp32_dword_load_uses_wrapped_ecx",
    ),
    (
        "s3_34cf_ir_addr32_rcx_index_no_base_sib_disp32_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rcx_index_no_base_sib_disp32_store_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34cf_host_addr32_rcx_index_no_base_sib_disp32_dword_store_wrapped_ecx", "local_acceptance", "od-host",
        "tests::s3_20eu_addr32_rcx_index_no_base_sib_disp32_dword_store_uses_wrapped_ecx",
    ),
    (
        "s3_34cg_ir_addr32_rcx_index_no_base_sib_scaled_disp32_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rcx_index_no_base_sib_scaled_disp32_load_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34cg_host_addr32_rcx_index_no_base_sib_scaled_disp32_dword_load_wrapped_scaled_ecx", "local_acceptance", "od-host",
        "tests::s3_20ev_addr32_rcx_index_no_base_sib_scaled_disp32_dword_load_uses_wrapped_scaled_ecx",
    ),
    (
        "s3_34ch_ir_addr32_rcx_index_no_base_sib_scaled_disp32_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rcx_index_no_base_sib_scaled_disp32_store_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34ch_host_addr32_rcx_index_no_base_sib_scaled_disp32_dword_store_wrapped_scaled_ecx", "local_acceptance", "od-host",
        "tests::s3_20ew_addr32_rcx_index_no_base_sib_scaled_disp32_dword_store_uses_wrapped_scaled_ecx",
    ),
    (
        "s3_34ci_ir_rax_index_no_base_sib_disp32_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rax_index_no_base_sib_disp32_load_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34ci_host_rax_index_no_base_sib_disp32_dword_load_original_rax", "local_acceptance", "od-host",
        "tests::s3_20ex_rax_index_no_base_sib_disp32_dword_load_uses_original_rax",
    ),
    (
        "s3_34cj_ir_rax_index_no_base_sib_disp32_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rax_index_no_base_sib_disp32_store_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34cj_host_rax_index_no_base_sib_disp32_dword_store_rax_address_and_data", "local_acceptance", "od-host",
        "tests::s3_20ey_rax_index_no_base_sib_disp32_dword_store_uses_full_rax_address_and_low_dword_data",
    ),
    (
        "s3_34ck_ir_rax_index_no_base_sib_scaled_disp32_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rax_index_no_base_sib_scaled_disp32_load_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34ck_host_rax_index_no_base_sib_scaled_disp32_dword_load_original_rax", "local_acceptance", "od-host",
        "tests::s3_20ez_rax_index_no_base_sib_scaled_disp32_dword_load_uses_original_scaled_rax",
    ),
    (
        "s3_34cl_ir_rax_index_no_base_sib_scaled_disp32_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rax_index_no_base_sib_scaled_disp32_store_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34cl_host_rax_index_no_base_sib_scaled_disp32_dword_store_rax_address_and_data", "local_acceptance", "od-host",
        "tests::s3_20fa_rax_index_no_base_sib_scaled_disp32_dword_store_uses_scaled_rax_address_and_low_dword_data",
    ),
    (
        "s3_34cm_ir_addr32_rax_index_no_base_sib_disp32_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rax_index_no_base_sib_disp32_load_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34cm_host_addr32_rax_index_no_base_sib_disp32_dword_load_original_eax", "local_acceptance", "od-host",
        "tests::s3_20fb_addr32_rax_index_no_base_sib_disp32_dword_load_uses_original_wrapped_eax",
    ),
    (
        "s3_34cn_ir_addr32_rax_index_no_base_sib_disp32_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rax_index_no_base_sib_disp32_store_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34cn_host_addr32_rax_index_no_base_sib_disp32_dword_store_eax_address_and_data", "local_acceptance", "od-host",
        "tests::s3_20fc_addr32_rax_index_no_base_sib_disp32_dword_store_uses_wrapped_eax_address_and_low_dword_data",
    ),
    (
        "s3_34co_ir_addr32_rax_index_no_base_sib_scaled_disp32_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rax_index_no_base_sib_scaled_disp32_load_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34co_host_addr32_rax_index_no_base_sib_scaled_disp32_dword_load_original_eax", "local_acceptance", "od-host",
        "tests::s3_20fd_addr32_rax_index_no_base_sib_scaled_disp32_dword_load_uses_original_wrapped_scaled_eax",
    ),
    (
        "s3_34cp_ir_addr32_rax_index_no_base_sib_scaled_disp32_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rax_index_no_base_sib_scaled_disp32_store_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34cp_host_addr32_rax_index_no_base_sib_scaled_disp32_dword_store_eax_address_and_data", "local_acceptance", "od-host",
        "tests::s3_20fe_addr32_rax_index_no_base_sib_scaled_disp32_dword_store_uses_wrapped_scaled_eax_address_and_low_dword_data",
    ),
    (
        "s3_34cq_ir_rax_disp8_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rax_disp8_load_binds_all_signed_source_bytes_to_its_site",
    ),
    (
        "s3_34cq_host_rax_disp8_dword_load_original_rax", "local_acceptance", "od-host",
        "tests::s3_20ff_rax_disp8_dword_load_uses_original_rax",
    ),
    (
        "s3_34cr_ir_rax_disp8_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rax_disp8_store_binds_all_signed_source_bytes_to_its_site",
    ),
    (
        "s3_34cr_host_rax_disp8_dword_store_rax_address_and_data", "local_acceptance", "od-host",
        "tests::s3_20fg_rax_disp8_dword_store_uses_full_rax_address_and_low_dword_data",
    ),
    (
        "s3_34cs_ir_rax_disp32_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rax_disp32_load_binds_signed_source_bytes_to_its_site",
    ),
    (
        "s3_34cs_host_rax_disp32_dword_load_original_rax", "local_acceptance", "od-host",
        "tests::s3_20fh_rax_disp32_dword_load_uses_original_full_rax",
    ),
    (
        "s3_34ct_ir_rax_disp32_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rax_disp32_store_binds_all_signed_source_bytes_to_its_site",
    ),
    (
        "s3_34ct_host_rax_disp32_dword_store_rax_address_and_data", "local_acceptance", "od-host",
        "tests::s3_20fi_rax_disp32_dword_store_uses_full_rax_address_and_low_dword_data",
    ),
    (
        "s3_34cu_ir_addr32_alias_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_alias_load_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34cu_host_addr32_alias_dword_load_original_eax", "local_acceptance", "od-host",
        "tests::s3_20fj_addr32_alias_dword_load_uses_original_eax",
    ),
    (
        "s3_34cv_ir_addr32_alias_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_alias_store_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34cv_host_addr32_alias_dword_store_original_eax_address_and_payload",
        "local_acceptance", "od-host",
        "tests::s3_20fk_addr32_alias_dword_store_uses_original_eax_address_and_payload",
    ),
    (
        "s3_34cw_ir_addr32_alias_disp8_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_alias_disp8_load_binds_all_signed_source_bytes_to_its_site",
    ),
    (
        "s3_34cw_host_addr32_alias_disp8_dword_load_wrapped_original_eax", "local_acceptance", "od-host",
        "tests::s3_20fl_addr32_alias_disp8_dword_load_uses_wrapped_original_eax",
    ),
    (
        "s3_34cx_ir_addr32_alias_disp8_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_alias_disp8_store_binds_all_signed_source_bytes_to_its_site",
    ),
    (
        "s3_34cx_host_addr32_alias_disp8_dword_store_wrapped_original_eax_address_and_payload",
        "local_acceptance", "od-host",
        "tests::s3_20fm_addr32_disp8_dword_store_uses_wrapped_original_eax_address_and_payload",
    ),
    (
        "s3_34cy_ir_addr32_alias_disp32_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_alias_disp32_load_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34cy_host_addr32_alias_disp32_dword_load_wrapped_original_eax", "local_acceptance", "od-host",
        "tests::s3_20fn_addr32_alias_disp32_dword_load_uses_wrapped_original_eax",
    ),
    (
        "s3_34cz_ir_addr32_alias_disp32_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_alias_disp32_store_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34cz_host_addr32_alias_disp32_dword_store_wrapped_original_eax_address_and_payload",
        "local_acceptance", "od-host",
        "tests::s3_20fo_addr32_alias_disp32_dword_store_uses_wrapped_original_eax_address_and_payload",
    ),
    (
        "s3_34da_ir_rcx_base_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rcx_base_load_binds_exact_source_and_site",
    ),
    (
        "s3_34da_host_rcx_base_dword_load_full_rcx_and_zero_extended_eax",
        "local_acceptance", "od-host",
        "tests::s3_20fp_rcx_base_dword_load_uses_full_rcx_and_zero_extends_eax",
    ),
    (
        "s3_34db_ir_rcx_base_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rcx_base_store_binds_exact_source_and_site",
    ),
    (
        "s3_34db_host_rcx_base_dword_store_full_rcx_and_low_eax_payload",
        "local_acceptance", "od-host",
        "tests::s3_20fq_rcx_base_dword_store_uses_full_rcx_and_low_eax_payload",
    ),
    (
        "s3_34dc_ir_rcx_disp8_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rcx_disp8_load_binds_all_signed_source_bytes_to_its_site",
    ),
    (
        "s3_34dc_host_rcx_disp8_dword_load_full_rcx_and_signed_displacement",
        "local_acceptance", "od-host",
        "tests::s3_20fr_rcx_disp8_dword_load_uses_full_rcx_and_signed_displacement",
    ),
    (
        "s3_34dd_ir_rcx_disp8_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rcx_disp8_store_binds_all_signed_source_bytes_to_its_site",
    ),
    (
        "s3_34dd_host_rcx_disp8_dword_store_full_rcx_and_low_eax_payload",
        "local_acceptance", "od-host",
        "tests::s3_20fs_rcx_disp8_dword_store_uses_full_rcx_and_low_eax_payload",
    ),
    (
        "s3_34de_ir_rcx_disp32_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rcx_disp32_load_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34de_host_rcx_disp32_dword_load_full_rcx_and_signed_displacement",
        "local_acceptance", "od-host",
        "tests::s3_20ft_rcx_disp32_dword_load_uses_full_rcx_and_signed_displacement",
    ),
    (
        "s3_34df_ir_rcx_disp32_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_rcx_disp32_store_binds_all_source_bytes_to_its_site",
    ),
    (
        "s3_34df_host_rcx_disp32_dword_store_full_rcx_and_low_eax_payload",
        "local_acceptance", "od-host",
        "tests::s3_20fu_rcx_disp32_dword_store_uses_full_rcx_and_low_eax_payload",
    ),
    (
        "s3_34dg_ir_addr32_rcx_base_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rcx_base_load_binds_exact_source_and_site",
    ),
    (
        "s3_34dg_host_addr32_rcx_base_dword_load_zero_extended_ecx",
        "local_acceptance", "od-host",
        "tests::s3_20fv_addr32_rcx_base_dword_load_uses_zero_extended_ecx",
    ),
    (
        "s3_34dh_ir_addr32_rcx_base_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rcx_base_store_binds_exact_source_and_site",
    ),
    (
        "s3_34dh_host_addr32_rcx_base_dword_store_zero_extended_ecx_and_low_eax_payload",
        "local_acceptance", "od-host",
        "tests::s3_20fw_addr32_rcx_base_dword_store_uses_zero_extended_ecx_and_low_eax_payload",
    ),
    (
        "s3_34di_ir_addr32_rcx_disp8_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rcx_disp8_load_binds_all_signed_source_bytes_to_its_site",
    ),
    (
        "s3_34di_host_addr32_rcx_disp8_dword_load_wrapped_ecx", "local_acceptance", "od-host",
        "tests::s3_20fx_addr32_rcx_disp8_dword_load_uses_wrapped_ecx",
    ),
    (
        "s3_34dj_ir_addr32_rcx_disp8_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rcx_disp8_store_binds_all_signed_source_bytes_to_its_site",
    ),
    (
        "s3_34dj_host_addr32_rcx_disp8_dword_store_wrapped_ecx_and_low_eax_payload",
        "local_acceptance", "od-host",
        "tests::s3_20fy_addr32_rcx_disp8_dword_store_uses_wrapped_ecx_and_low_eax_payload",
    ),
    (
        "s3_34dk_ir_addr32_rcx_disp32_dword_load_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rcx_disp32_load_binds_exact_source_and_site",
    ),
    (
        "s3_34dk_host_addr32_rcx_disp32_dword_load_wrapped_ecx", "local_acceptance", "od-host",
        "tests::s3_20fz_addr32_rcx_disp32_dword_load_uses_wrapped_ecx",
    ),
    (
        "s3_34dl_ir_addr32_rcx_disp32_dword_store_source_binding", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_dword_addr32_rcx_disp32_store_binds_exact_source_and_site",
    ),
    (
        "s3_34dl_host_addr32_rcx_disp32_dword_store_wrapped_ecx_and_low_eax_payload",
        "local_acceptance", "od-host",
        "tests::s3_20ga_addr32_rcx_disp32_dword_store_uses_wrapped_ecx_and_low_eax_payload",
    ),
    (
        "s3_34dm_addr32_rsp_dword_store_fixed_seed_corpus", "local_acceptance", "od-host",
        "fixed_seed_addr32_rsp_dword_store_corpus_matches_interpreter_twice_per_input",
    ),
    (
        "s3_34dn_replay_artifact_self_check", "local_acceptance", "od-host",
        "s3_34dn_replay_artifact_self_checks_roundtrip_and_fail_closed",
    ),
    (
        "s3_34do_value_only_reducer_self_check", "local_acceptance", "od-host",
        "s3_34do_value_only_reducer_self_checks_are_bounded_and_preserve_signature",
    ),
    (
        "s3_34do_captured_runtime_mismatch", "local_acceptance", "od-host",
        "s3_34do_captured_runtime_mismatch_replays_and_reduces",
    ),
    (
        "s3_26f_store_store_source_sites_and_pc_map", "local_acceptance", "od-jit",
        "memory_load_v2::tests::ir_store_then_store_binds_two_exact_ordered_sites",
    ),
    (
        "s3_26f_store_store_uncached_dispatch_continuation", "local_acceptance", "od-jit",
        "memory_dispatch::tests::uncached_dispatch_selects_store_store_pair_and_resumes_at_second_site",
    ),
    (
        "s3_26f_store_store_corpus_jit_interpreter", "local_acceptance", "od-host",
        "fixed_seed_s3_26f_store_store_corpus_matches_interpreter_twice_per_input",
    ),
    (
        "s3_26f_store_store_fixed_seed_generator", "local_acceptance", "od-testkit",
        "memory_corpus::tests::s3_26f_store_store_inputs_are_deterministic_unique_and_use_two_addresses",
    ),
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
    (
        "s3_16o_store_store_not_rcx_store_store_corpus", "local_acceptance", "od-host",
        "tests::s3_16o_s3_34_store_store_not_rcx_store_store_fixed_seed_boundaries",
    ),
    (
        "s3_16_publication_refusal", "local_acceptance", "od-host",
        "paged_host_loop::tests::s3_16_publication_refusal_interprets_once_and_resumes_native",
    ),
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
    (
        "s3_18g_load_load_dependency_generation_republishes", "local_acceptance", "od-host",
        "tests::s3_18g_load_load_dependency_generation_republishes",
    ),
)
RUST_TEST_TARGET_OVERRIDES = {
    (
        "od-host",
        "fixed_seed_s3_26f_store_store_corpus_matches_interpreter_twice_per_input",
    ): ("--test", "memory_corpus"),
    (
        "od-host",
        "published_single_site_pc_maps_match_artifacts_outlive_them_and_republish",
    ): ("--test", "memory_corpus"),
    (
        "od-host",
        "capacity_one_memory_cache_republishes_after_fault_and_backend_error",
    ): ("--test", "memory_corpus"),
    (
        "od-host",
        "published_dword_pc_maps_match_artifacts_survive_drop_and_republish_at_new_rips",
    ): ("--test", "memory_corpus"),
    (
        "od-host",
        "published_dword_disp8_and_disp32_load_pc_maps_match_artifacts_survive_drop_and_republish",
    ): ("--test", "memory_corpus"),
    (
        "od-host",
        "published_dword_disp8_and_disp32_store_pc_maps_match_artifacts_survive_drop_and_republish",
    ): ("--test", "memory_corpus"),
    (
        "od-host",
        "fixed_seed_addr32_rsp_dword_store_corpus_matches_interpreter_twice_per_input",
    ): ("--test", "stage3_dword_store_corpus"),
    (
        "od-host",
        "s3_34dn_replay_artifact_self_checks_roundtrip_and_fail_closed",
    ): ("--test", "stage3_dword_store_corpus"),
    (
        "od-host",
        "s3_34do_value_only_reducer_self_checks_are_bounded_and_preserve_signature",
    ): ("--test", "stage3_dword_store_corpus"),
    (
        "od-host",
        "s3_34do_captured_runtime_mismatch_replays_and_reduces",
    ): ("--test", "stage3_dword_store_corpus"),
}
RUST_TEST_ARGV_OVERRIDES = {
    (
        "od-host",
        "tests::s3_27i_disp32_dword_store_checks_canonical_end_and_range_overflow",
    ): (
        "cargo", "test", "-p", "od-host", "--lib",
        "tests::s3_27i_disp32_dword_store_checks_canonical_end_and_range_overflow",
        "--offline", "--", "--exact",
    ),
}
RUST_TEST_GATES = (
    *(
        (name, "local_lifecycle", "od-host", f"tests::{name}")
        for name in LIFECYCLE_TESTS
    ),
    *ADDITIONAL_TEST_GATES,
)
LOCAL_VMHOST_RESULT_SUITE = "local_vmhost_result_state"
LOCAL_VMHOST_RESULT_STEPS = (
    "local_s38d_validator_selfcheck",
    "local_s38d_codesign_path",
    "local_s38d_build_od_host",
    "local_s38d_compile_vmhost",
    "local_s38d_sign_vmhost",
    "local_s38d_verify_vmhost",
    "local_s38d_vmhost_signature",
    "local_s38d_vmhost_entitlements",
    "local_s38d_compile_launcher",
    "local_s38d_sign_launcher",
    "local_s38d_verify_launcher",
    "local_s38d_launcher_signature",
    "local_s38d_launcher_entitlements",
    "local_s38d_result_state",
)
LOCAL_VMHOST_RESULT_TIMEOUTS = {
    "local_s38d_validator_selfcheck": 120,
    "local_s38d_codesign_path": 30,
    "local_s38d_build_od_host": 1800,
    "local_s38d_compile_vmhost": 900,
    "local_s38d_sign_vmhost": 120,
    "local_s38d_verify_vmhost": 120,
    "local_s38d_vmhost_signature": 120,
    "local_s38d_vmhost_entitlements": 120,
    "local_s38d_compile_launcher": 900,
    "local_s38d_sign_launcher": 120,
    "local_s38d_verify_launcher": 120,
    "local_s38d_launcher_signature": 120,
    "local_s38d_launcher_entitlements": 120,
    "local_s38d_result_state": 900,
}
VMHOST_LINUX_DIRECT_SOURCE = "apps/vmhost/Sources/LinuxDirectRunner.swift"
LOCAL_VMHOST_RESULT_SOURCE_PATHS = (
    "Cargo.lock",
    "Cargo.toml",
    "crates/od-host/Cargo.toml",
    "crates/od-host/src/lib.rs",
    "include/open_desktop/host_abi.h",
    "apps/macos/Sources/Shared/ControlFrame.swift",
    "apps/vmhost/Sources/FrameTransport.swift",
    "apps/vmhost/Sources/InputChannel.swift",
    "apps/macos/Sources/Shared/InputProtocol.swift",
    "apps/macos/Sources/Shared/InputQueue.swift",
    "apps/vmhost/Sources/VMHostMain.swift",
    VMHOST_LINUX_DIRECT_SOURCE,
    "apps/macos/Sources/Shared/ControlV2.swift",
    "apps/vmhost/Sources/BundleLock.swift",
    "apps/vmhost/Sources/Lifecycle.swift",
    "apps/vmhost/Sources/ControlV2Host.swift",
    "apps/macos/Sources/Shared/VMHostLaunch.swift",
    "apps/macos/Sources/launcher.swift",
    "apps/vmhost/Configuration/vmhost-smoke.entitlements",
    "protocols/control/CONTROL_PROTOCOL_V0.md",
    "tests/system/vmhost_result_state_smoke.py",
    "tests/cpu/stage3_harness.py",
    "tests/cpu/test_stage3_harness.py",
)
LOCAL_STEPS = (
    "harness_self_tests",
    "stage1_dispatch",
    "memory_corpus",
    "memory_s3_34h_exact_gate",
    *(step for step, _, _, _ in RUST_TEST_GATES),
    *LOCAL_VMHOST_RESULT_STEPS,
)
UNSELECTED_SUITES = ("physical_x86", "mac_mini", "performance", "owner_backed", "stage2")
LOCAL_SUITE_NAMES = (
    "local_lifecycle", "local_acceptance", "stage1_dispatch", "memory_corpus",
    LOCAL_VMHOST_RESULT_SUITE,
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
    "process_compile_v1_admission_failure_vmhost",
    "process_sign_v1_admission_failure_vmhost",
    "process_verify_v1_admission_failure_vmhost",
    "process_v1_admission_failure_vmhost_signature",
    "process_v1_admission_failure_vmhost_entitlements",
    "process_control_race",
    "process_control_v1_sessions",
    "process_control_v1_code_mismatch",
    "process_control_v1_frames",
    "process_control_v1_admission_failures",
    "process_control_v1_stop_eof",
    "process_control_v1_run_stop_id",
    "process_control_v1_run_id_exhaustion",
    "process_control_v1_isolation",
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
PROCESS_SUCCESS_MARKERS = {
    "control_race": "VMHost control smoke passed:",
    "control_v1_sessions": (
        "VMHost control V1 smoke passed: same-code sequential RUNs with fresh state and budgets"
    ),
    "control_v1_code_mismatch": (
        "VMHost control V1 smoke passed: immutable-code mismatch rejection "
        "preserves binding and RUN ID high-watermark"
    ),
    "control_v1_frames": (
        "VMHost control V1 smoke passed: fragmented/coalesced frames, "
        "maximum RUN length, bounded EOF teardown, oversized length, backpressure, active completion"
    ),
    "control_v1_admission_failures": (
        "VMHost control V1 smoke passed: create/begin INVALID_STATE closes the frame batch"
    ),
    "control_v1_stop_eof": (
        "VMHost control V1 smoke passed: STOP isolation and bounded active-run EOF teardown"
    ),
    "control_v1_run_stop_id": (
        "VMHost control V1 smoke passed: active STOP and RUN-ID high-watermark"
    ),
    "control_v1_run_id_exhaustion": (
        "VMHost control V1 smoke passed: RUN-ID exhaustion does not wrap and "
        "fresh connection accepts ID 1"
    ),
    "control_v1_isolation": (
        "VMHost control V1 isolation smoke passed: SIGSEGV stayed within helper A; "
        "helper B completed exact STOP, fresh RUN, and idle EOF"
    ),
    "isolation": "VMHost isolation smoke passed:",
    "generated_fault": "VMHost terminated by SIGSEGV",
    "app_bundle": "vmhost_app_bundle_status=passed",
}
ALL_SUITES = (*LOCAL_SUITE_NAMES, PROCESS_SUITE_NAME, *UNSELECTED_SUITES)
ALL_STEPS = (*LOCAL_STEPS, *PROCESS_SUITE_STEPS)
LOCAL_VMHOST_RESULT_LOOP = "48b8010000000000000075f4"
LOCAL_VMHOST_RESULT_BUDGETS = (0, 1, 2, 3, 4)
LOCAL_VMHOST_RESULT_INITIAL_GPRS = (
    0xF00DFACE12345678,
    0x1122334455667788,
    0x8877665544332211,
    0xCAFEBABE0BADF00D,
)
LOCAL_VMHOST_RESULT_INITIAL_RFLAGS = 0x202
LOCAL_VMHOST_RESULT_LAUNCHER_RFLAGS = 0x2
LOCAL_VMHOST_RESULT_STOP_BUDGET = (1 << 64) - 1
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
    "process_compile_v1_admission_failure_vmhost": 900,
    "process_sign_v1_admission_failure_vmhost": 120,
    "process_verify_v1_admission_failure_vmhost": 120,
    "process_v1_admission_failure_vmhost_signature": 120,
    "process_v1_admission_failure_vmhost_entitlements": 120,
    "process_control_race": 900,
    "process_control_v1_sessions": 30,
    "process_control_v1_code_mismatch": 30,
    "process_control_v1_frames": 30,
    "process_control_v1_admission_failures": 30,
    "process_control_v1_stop_eof": 30,
    "process_control_v1_run_stop_id": 30,
    "process_control_v1_run_id_exhaustion": 30,
    "process_control_v1_isolation": 30,
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
    "crates/od-host/src/session.rs",
    "crates/od-host/src/session_abi_v1.rs",
    "include/open_desktop/host_abi.h",
    "include/open_desktop/session_abi_v1.h",
    "protocols/control/CONTROL_PROTOCOL_V1.md",
    "apps/macos/Sources/Shared/ControlFrame.swift",
    "apps/vmhost/Sources/FrameTransport.swift",
    "apps/vmhost/Sources/InputChannel.swift",
    "apps/macos/Sources/Shared/InputProtocol.swift",
    "apps/macos/Sources/Shared/InputQueue.swift",
    "apps/macos/Sources/ODAtomics/include/FrameTransportAtomics.h",
    "apps/vmhost/Sources/VMHostMain.swift",
    VMHOST_LINUX_DIRECT_SOURCE,
    "apps/macos/Sources/Shared/ControlV2.swift",
    "apps/vmhost/Sources/BundleLock.swift",
    "apps/vmhost/Sources/Lifecycle.swift",
    "apps/vmhost/Sources/ControlV2Host.swift",
    "apps/vmhost/Sources/ODHostABI/include/VMHostABI.h",
    "apps/macos/Sources/Shared/VMHostLaunch.swift",
    "apps/macos/Sources/launcher.swift",
    "apps/vmhost/Configuration/vmhost-smoke.entitlements",
    "tests/system/vmhost_control_race_smoke.py",
    "tests/fixtures/control/control_v0_scenarios.json",
    "tests/fixtures/control/v1_admission_failure_bridge.h",
    "tests/system/vmhost_control_v1_same_code_smoke.py",
    "tests/system/vmhost_control_v1_code_mismatch_smoke.py",
    "tests/system/vmhost_control_v1_frames_smoke.py",
    "tests/system/vmhost_control_v1_stop_eof_smoke.py",
    "tests/system/vmhost_control_v1_run_stop_id_smoke.py",
    "tests/system/vmhost_control_v1_run_id_exhaustion_smoke.py",
    "tests/system/vmhost_control_v1_isolation_smoke.py",
    "tests/system/vmhost_isolation_smoke.py",
    "tests/system/vmhost_generated_fault_smoke.py",
    "tests/system/vmhost_generated_fault_helper.swift",
    "tests/system/vmhost_app_bundle_smoke.sh",
    "tests/cpu/stage3_harness.py",
)
LOCAL_RUST_TEST_SOURCES = (
    "crates/od-host/tests/memory_corpus.rs",
    "crates/od-testkit/src/memory_corpus.rs",
    "crates/od-host/src/lib.rs",
    "crates/od-host/src/paged_host_loop.rs",
    "crates/od-host/src/session_abi_v1.rs",
    "crates/od-host/src/session.rs",
    "crates/od-host/src/tests/native_register_v1_corpus.rs",
    "tests/cpu/stage3_dword_store_corpus.rs",
    "crates/od-jit/src/memory_cache.rs",
    "crates/od-jit/src/memory_load_v2.rs",
    "crates/od-jit/src/block.rs",
    "crates/od-jit/src/block_emitter.rs",
    "crates/od-jit/src/block_compile.rs",
    "crates/od-jit/src/lowering.rs",
    "crates/od-jit/src/dispatch.rs",
    "crates/od-jit/src/memory_dispatch.rs",
    "crates/od-jit/src/statepoints.rs",
    "crates/od-jit/src/translation.rs",
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
    argv_override = RUST_TEST_ARGV_OVERRIDES.get((package, identity))
    if argv_override is not None:
        return list(argv_override)
    target = RUST_TEST_TARGET_OVERRIDES.get((package, identity), ("--lib",))
    return [
        "cargo", "test", "--offline", "-p", package, *target, identity,
        "--", "--exact", "--test-threads=1",
    ]


def native_register_v1_reducer_artifact_inventory(run_dir: Path) -> list[dict[str, str]]:
    reducer_dir = run_dir / "artifacts" / "native-register-v1" / "native-register-v1-reducer"
    if not reducer_dir.is_dir() or reducer_dir.is_symlink():
        raise HarnessError("S3-34o reducer artifact directory is missing or symlinked")
    entries = []
    for path in sorted(reducer_dir.rglob("*")):
        if path.is_symlink():
            raise HarnessError(f"S3-34o reducer artifact cannot be a symlink: {path}")
        if path.is_file():
            entries.append({
                "path": path.relative_to(ROOT).as_posix(),
                "sha256": sha256_file(path),
            })
    required = {
        "reduction-report.json",
        "source-saved-case.json",
        "local-fault-injection-selfcheck/reduction-report.json",
        "local-fault-injection-selfcheck/source-saved-case.json",
        "local-fault-injection-selfcheck/reduced-saved-case.json",
        "local-fault-injection-selfcheck/reduced-input-digest.txt",
        "local-fault-injection-selfcheck/limit/reduction-report.json",
        "local-fault-injection-selfcheck/limit/reduced-saved-case.json",
        "local-fault-injection-selfcheck/limit/reduced-input-digest.txt",
    }
    observed = {
        Path(entry["path"]).relative_to(reducer_dir.relative_to(ROOT)).as_posix()
        for entry in entries
    }
    if not required.issubset(observed):
        raise HarnessError("S3-34o reducer artifact inventory is incomplete")
    return entries


def native_register_v1_cache_trace_evidence(run_dir: Path) -> dict[str, object]:
    artifact_dir = run_dir / "artifacts" / "native-register-v1"
    trace_path = artifact_dir / "native-register-v1-cache-trace.json"
    observations_path = artifact_dir / "native-register-v1-observations.json"
    if any(path.is_symlink() for path in (run_dir, run_dir / "artifacts", artifact_dir, trace_path, observations_path)):
        raise HarnessError("S3-34p cache trace path cannot be a symlink")
    try:
        trace = json.loads(trace_path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise HarnessError(f"S3-34p cache trace is missing or malformed: {error}") from error
    try:
        observations = json.loads(observations_path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise HarnessError(f"S3-34p source observations are missing or malformed: {error}") from error
    if not isinstance(trace, dict):
        raise HarnessError("S3-34p cache trace must be an object")
    if not isinstance(observations, dict):
        raise HarnessError("S3-34p source observations must be an object")
    expected_observations_digest = native_register_v1_observations_digest(observations)
    segments = trace.get("segments")
    expected_segments = [
        ("generated_corpus", "eager", 1200),
        ("generated_corpus", "demand_candidate", 1200),
        ("retained_hit", "eager", 2),
        ("retained_hit", "demand_candidate", 2),
    ]
    if (
        trace.get("version") != 1
        or trace.get("profile") != "NativeRegisterV1"
        or not isinstance(trace.get("source_inputs_digest"), str)
        or not re.fullmatch(r"fnv1a64:[0-9a-f]{16}", trace["source_inputs_digest"])
        or not isinstance(trace.get("trace_digest"), str)
        or not re.fullmatch(r"fnv1a64:[0-9a-f]{16}", trace["trace_digest"])
        or not isinstance(segments, list)
        or len(segments) != len(expected_segments)
        or observations.get("version") != 1
        or observations.get("profile") != "NativeRegisterV1"
        or observations.get("inputs_digest") != trace.get("source_inputs_digest")
        or not isinstance(observations.get("envelope_digest"), str)
        or not re.fullmatch(r"fnv1a64:[0-9a-f]{16}", observations["envelope_digest"])
        or observations["envelope_digest"] != expected_observations_digest
        or not isinstance(observations.get("cases"), list)
        or len(observations["cases"]) != 1200
    ):
        raise HarnessError("S3-34p cache trace/source observations version, profile or inventory differs")
    for segment_index, (segment, expected) in enumerate(zip(segments, expected_segments)):
        if (
            not isinstance(segment, dict)
            or (segment.get("kind"), segment.get("strategy")) != expected[:2]
            or not isinstance(segment.get("steps"), list)
            or len(segment["steps"]) != expected[2]
        ):
            raise HarnessError(f"S3-34p cache trace segment differs: {segment_index}")
        for sequence, step in enumerate(segment["steps"]):
            if (
                not isinstance(step, dict)
                or step.get("sequence_index") != sequence
                or not isinstance(step.get("input"), dict)
                or step["input"].get("version") != 1
                or step["input"].get("profile") != "NativeRegisterV1"
                or not isinstance(step["input"].get("input_digest"), str)
                or not re.fullmatch(r"fnv1a64:[0-9a-f]{16}", step["input"]["input_digest"])
                or not isinstance(step.get("bindings"), dict)
                or not all(isinstance(step.get(field), dict) for field in ("interpreter", "uncached", "cached", "cache"))
            ):
                raise HarnessError(f"S3-34p cache trace step differs: {segment_index}:{sequence}")
    for segment_index in (2, 3):
        steps = segments[segment_index]["steps"]
        first_input = steps[0]["input"]
        second_input = steps[1]["input"]
        first_cache = steps[0]["cache"]
        second_cache = steps[1]["cache"]
        if (
            first_input.get("code") != second_input.get("code")
            or first_input.get("guest_base_rip") != second_input.get("guest_base_rip")
            or steps[0].get("bindings") != steps[1].get("bindings")
            or first_input.get("initial_gpr", [None])[0] == second_input.get("initial_gpr", [None])[0]
            or first_input.get("initial_rflags") == second_input.get("initial_rflags")
            or first_cache.get("misses") != 1
            or first_cache.get("successful_compiles") != 1
            or first_cache.get("successful_publications") != 1
            or second_cache.get("hits") != 1
            or second_cache.get("successful_compiles") != 1
            or second_cache.get("successful_publications") != 1
        ):
            raise HarnessError(f"S3-34p retained-block hit evidence differs: {segment_index}")
    payload = {
        "version": trace["version"],
        "profile": trace["profile"],
        "source_inputs_digest": trace["source_inputs_digest"],
        "segments": segments,
    }
    raw = json.dumps(payload, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    value = 0xCBF29CE484222325
    for byte in raw:
        value = ((value ^ byte) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    if trace["trace_digest"] != f"fnv1a64:{value:016x}":
        raise HarnessError("S3-34p cache trace digest does not bind its ordered steps")
    return {
        "path": trace_path.relative_to(ROOT).as_posix(),
        "sha256": sha256_file(trace_path),
        "observations_path": observations_path.relative_to(ROOT).as_posix(),
        "observations_sha256": sha256_file(observations_path),
        "observations_envelope_digest": observations["envelope_digest"],
        "version": trace["version"],
        "profile": trace["profile"],
        "source_inputs_digest": trace["source_inputs_digest"],
        "trace_digest": trace["trace_digest"],
        "segments": [
            {"kind": segment["kind"], "strategy": segment["strategy"], "steps": len(segment["steps"])}
            for segment in segments
        ],
    }


def native_register_v1_observations_digest(observations: dict) -> str:
    payload = {
        "version": observations.get("version"),
        "profile": observations.get("profile"),
        "inputs_digest": observations.get("inputs_digest"),
        "cases": observations.get("cases"),
    }
    raw = json.dumps(payload, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    value = 0xCBF29CE484222325
    for byte in raw:
        value = ((value ^ byte) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return f"fnv1a64:{value:016x}"


def attach_native_register_v1_cache_trace_evidence(step: dict, run_dir: Path) -> None:
    step["cache_trace_evidence"] = native_register_v1_cache_trace_evidence(run_dir)


def validate_native_register_v1_cache_trace_artifacts(steps: dict, run_dir: Path) -> None:
    gate_names = (
        "s3_34p_cached_native_register_v1_corpus",
        "s3_34p_cached_native_register_v1_replay",
        "s3_34p_cached_native_register_v1_failure_replay",
    )
    expected_dir = run_dir / "artifacts" / "native-register-v1"
    expected_relative = expected_dir.relative_to(ROOT).as_posix()
    evidence = None
    for name in gate_names:
        step = steps.get(name)
        if (
            not isinstance(step, dict)
            or step.get("status") != "passed"
            or step.get("artifact_directory") != expected_relative
        ):
            raise HarnessError(f"S3-34p cache trace artifact binding is missing: {name}")
        current = native_register_v1_cache_trace_evidence(run_dir)
        if step.get("cache_trace_evidence") != current:
            raise HarnessError(f"S3-34p cache trace evidence is missing or forged: {name}")
        if evidence is not None and current != evidence:
            raise HarnessError("S3-34p cache trace changed between corpus and replay gates")
        evidence = current


def validate_native_register_v1_reducer_inventory(steps: dict, run_dir: Path) -> None:
    expected = native_register_v1_reducer_artifact_inventory(run_dir)
    for name in (
        "s3_34o_native_register_v1_reducer",
        "s3_34o_native_register_v1_reducer_selfcheck",
    ):
        step = steps.get(name)
        if not isinstance(step, dict) or step.get("reducer_artifacts") != expected:
            raise HarnessError(f"S3-34o reducer artifact digest/path binding differs: {name}")


def attach_native_register_v1_evidence(step: dict, run_dir: Path, step_name: str) -> None:
    expected_dir = run_dir / "artifacts" / "native-register-v1"
    expected_relative = expected_dir.relative_to(ROOT).as_posix()
    if step.get("artifact_directory") != expected_relative or any(
        path.is_symlink()
        for path in (run_dir, run_dir / "artifacts", expected_dir)
    ):
        raise HarnessError("S3-34n artifact directory is missing or differs from this run")
    if step_name.startswith("s3_34o_native_register_v1_"):
        step["reducer_artifacts"] = native_register_v1_reducer_artifact_inventory(run_dir)
        return
    if step_name.startswith("s3_34p_cached_native_register_v1_"):
        attach_native_register_v1_cache_trace_evidence(step, run_dir)
        return
    if step_name != "s3_34n_native_register_v1_corpus":
        return
    summary_path = expected_dir / "native-register-v1-summary.json"
    case_path = expected_dir / "native-register-v1-saved-case.json"
    observations_path = expected_dir / "native-register-v1-observations.json"
    if any(path.is_symlink() for path in (summary_path, case_path, observations_path)):
        raise HarnessError("S3-34n artifact file cannot be a symlink")
    try:
        summary = json.loads(summary_path.read_text())
        case = json.loads(case_path.read_text())
        observations = json.loads(observations_path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise HarnessError(f"S3-34n corpus evidence is missing or malformed: {error}") from error
    if not isinstance(summary, dict) or not isinstance(case, dict) or not isinstance(observations, dict):
        raise HarnessError("S3-34n corpus artifacts must be JSON objects")
    expected_seeds = [0x5333346E00000001, 0x5333346E00000002, 0x5333346E00000003]
    forms = summary.get("form_counts") if isinstance(summary, dict) else None
    case_input = case.get("input") if isinstance(case, dict) else None
    if (
        summary.get("profile") != "NativeRegisterV1"
        or summary.get("version") != 1
        or summary.get("unique_inputs") != 1200
        or summary.get("seeds") != expected_seeds
        or summary.get("cases_per_seed") != 400
        or summary.get("repeats_per_backend") != 2
        or summary.get("full_partial_preserve_xchg_branch_anchors") != 3
        or summary.get("rip_relative_lea_after_variable_predecessor_cases") != 1197
        or summary.get("bounded_loop_inputs") != 147
        or not isinstance(summary.get("zero_budget_inputs"), int)
        or summary["zero_budget_inputs"] <= 0
        or summary.get("rip_predecessor_lengths") != [3, 4, 7]
        or any(
            not isinstance(summary.get(field), int) or summary[field] <= 0
            for field in (
                "anchor_branch_taken",
                "anchor_branch_fallthrough",
                "code_end_exits",
                "budget_exits",
                "stop_address_exits",
                "stop_requested_exits",
                "fallback_instructions_retired",
            )
        )
        or not isinstance(forms, list)
        or len(forms) != 13
        or any(not isinstance(count, int) or count <= 0 for count in forms)
        or not isinstance(summary.get("canonical_inputs_digest"), str)
        or not summary["canonical_inputs_digest"].startswith("fnv1a64:")
        or not isinstance(case_input, dict)
        or case_input.get("profile") != "NativeRegisterV1"
        or case_input.get("version") != 1
        or not isinstance(case_input.get("input_digest"), str)
        or not case_input["input_digest"].startswith("fnv1a64:")
        or not isinstance(case.get("backend_runs"), dict)
        or observations.get("version") != 1
        or observations.get("profile") != "NativeRegisterV1"
        or observations.get("inputs_digest") != summary.get("canonical_inputs_digest")
        or not isinstance(observations.get("cases"), list)
        or len(observations["cases"]) != 1200
        or observations.get("envelope_digest") != native_register_v1_observations_digest(observations)
    ):
        raise HarnessError("S3-34n corpus count, seed, form coverage or saved-case evidence differs")
    step["corpus_evidence"] = {
        "summary": summary,
        "summary_path": summary_path.relative_to(ROOT).as_posix(),
        "summary_sha256": sha256_file(summary_path),
        "saved_case_path": case_path.relative_to(ROOT).as_posix(),
        "saved_case_sha256": sha256_file(case_path),
        "observations_path": observations_path.relative_to(ROOT).as_posix(),
        "observations_sha256": sha256_file(observations_path),
        "observations_envelope_digest": observations["envelope_digest"],
    }


def validate_native_register_v1_artifacts(steps: dict, run_dir: Path) -> None:
    gate_names = (
        "s3_34n_native_register_v1_corpus",
        "s3_34n_native_register_v1_replay",
        "s3_34n_native_register_v1_selfcheck",
        "s3_34o_native_register_v1_reducer",
        "s3_34o_native_register_v1_reducer_selfcheck",
        "s3_34p_cached_native_register_v1_corpus",
        "s3_34p_cached_native_register_v1_replay",
        "s3_34p_cached_native_register_v1_failure_replay",
    )
    expected_dir = run_dir / "artifacts" / "native-register-v1"
    expected_relative = expected_dir.relative_to(ROOT).as_posix()
    for name in gate_names:
        step = steps.get(name)
        if not isinstance(step, dict) or step.get("status") != "passed" or step.get("artifact_directory") != expected_relative:
            raise HarnessError(f"S3-34n run artifact binding is missing or forged: {name}")
    corpus_step = steps[gate_names[0]]
    evidence = corpus_step.get("corpus_evidence")
    if not isinstance(evidence, dict):
        raise HarnessError("S3-34n corpus summary evidence is missing")
    summary_path = expected_dir / "native-register-v1-summary.json"
    case_path = expected_dir / "native-register-v1-saved-case.json"
    observations_path = expected_dir / "native-register-v1-observations.json"
    if (
        any(path.is_symlink() for path in (run_dir, run_dir / "artifacts", expected_dir, summary_path, case_path, observations_path))
        or
        evidence.get("summary_path") != summary_path.relative_to(ROOT).as_posix()
        or evidence.get("saved_case_path") != case_path.relative_to(ROOT).as_posix()
        or evidence.get("observations_path") != observations_path.relative_to(ROOT).as_posix()
        or evidence.get("summary_sha256") != sha256_file(summary_path)
        or evidence.get("saved_case_sha256") != sha256_file(case_path)
        or evidence.get("observations_sha256") != sha256_file(observations_path)
    ):
        raise HarnessError("S3-34n saved corpus artifact digest/path binding differs")
    try:
        stored_summary = json.loads(summary_path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise HarnessError(f"S3-34n corpus summary cannot be reread: {error}") from error
    if evidence.get("summary") != stored_summary:
        raise HarnessError("S3-34n corpus counts/digest differ from the saved summary")
    try:
        stored_observations = json.loads(observations_path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise HarnessError(f"S3-34n source observations cannot be reread: {error}") from error
    if (
        evidence.get("observations_envelope_digest") != stored_observations.get("envelope_digest")
        or stored_observations.get("envelope_digest") != native_register_v1_observations_digest(stored_observations)
    ):
        raise HarnessError("S3-34n source observation envelope digest differs")
    validate_native_register_v1_reducer_inventory(steps, run_dir)
    validate_native_register_v1_cache_trace_artifacts(steps, run_dir)
    reducer_dir = expected_dir / "native-register-v1-reducer"
    report_path = reducer_dir / "reduction-report.json"
    source_path = reducer_dir / "source-saved-case.json"
    if any(path.is_symlink() for path in (reducer_dir, report_path, source_path)):
        raise HarnessError("S3-34o reducer artifacts cannot be symlinks")
    try:
        reducer_report = json.loads(report_path.read_text())
        reducer_source = json.loads(source_path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise HarnessError(f"S3-34o reducer artifact is missing or malformed: {error}") from error
    def complete_repeats(value: object) -> bool:
        if not isinstance(value, dict) or value.get("errors") != []:
            return False
        return all(
            isinstance(value.get(backend), list)
            and len(value[backend]) == 2
            and all(isinstance(item, dict) for item in value[backend])
            for backend in ("interpreter", "eager_native", "demand_candidate")
        )

    if (
        not isinstance(reducer_report, dict)
        or reducer_report.get("kind") != "native-register-v1-value-reduction"
        or reducer_report.get("source_digest") != reducer_source.get("input", {}).get("input_digest")
        or not complete_repeats(reducer_report.get("source_backend_runs"))
        or reducer_report.get("source_signature") != []
        or reducer_report.get("source_replay_signature") != []
        or "saved_case_round_trip_and_replay" not in reducer_report.get("source_replay_command", "")
        or reducer_report.get("metric_definition")
        != "popcount(initial GPRs, initial RFLAGS & 0x0cd5, and eligible encoded immediate/displacement values)"
        or not isinstance(reducer_report.get("attempts"), list)
        or reducer_report.get("evaluated_count") != 0
        or reducer_report.get("accepted_count") != 0
        or reducer_report.get("stop_reason") != "not_reproduced"
        or reducer_report.get("reduced_digest") is not None
        or (reducer_dir / "reduced-saved-case.json").exists()
    ):
        raise HarnessError("S3-34o reducer did not fail closed for the non-mismatch corpus sample")
    injected_dir = reducer_dir / "local-fault-injection-selfcheck"
    injected_report_path = injected_dir / "reduction-report.json"
    injected_case_path = injected_dir / "reduced-saved-case.json"
    digest_path = injected_dir / "reduced-input-digest.txt"
    try:
        injected_report = json.loads(injected_report_path.read_text())
        injected_case = json.loads(injected_case_path.read_text())
        injected_digest = digest_path.read_text().strip()
    except (OSError, json.JSONDecodeError) as error:
        raise HarnessError(f"S3-34o local fault-injection selfcheck artifacts missing: {error}") from error
    if (
        injected_report.get("kind") != "LOCAL-FAULT-INJECTION SELF-CHECK ONLY; NOT REPLAY EVIDENCE"
        or injected_report.get("accepted_count", 0) <= 0
        or not isinstance(injected_report.get("evaluated_count"), int)
        or injected_report["evaluated_count"] > 256
        or injected_report.get("accepted_count", 0) + injected_report.get("rejected_count", 0)
        != injected_report.get("evaluated_count")
        or injected_report.get("source_signature") != injected_report.get("reduced_signature")
        or injected_report.get("metric_after", 0) >= injected_report.get("metric_before", 0)
        or injected_report.get("reduced_digest") != injected_digest
        or injected_report.get("metric_definition")
        != "popcount(initial GPRs, initial RFLAGS & 0x0cd5, and eligible encoded immediate/displacement values)"
        or "saved_case_round_trip_and_replay" not in injected_report.get("source_replay_command", "")
        or "saved_case_round_trip_and_replay" not in injected_report.get("reduced_replay_command", "")
        or injected_case.get("input", {}).get("input_digest") != injected_digest
        or injected_case.get("failure_signature") != injected_report.get("source_signature")
        or not complete_repeats(injected_report.get("source_backend_runs"))
        or not complete_repeats(injected_report.get("reduced_backend_runs"))
    ):
        raise HarnessError("S3-34o local fault-injection reducer evidence differs")
    limit_report_path = injected_dir / "limit" / "reduction-report.json"
    limit_case_path = injected_dir / "limit" / "reduced-saved-case.json"
    limit_digest_path = injected_dir / "limit" / "reduced-input-digest.txt"
    if any(path.is_symlink() for path in (limit_report_path, limit_case_path, limit_digest_path)):
        raise HarnessError("S3-34o limit selfcheck artifacts cannot be symlinks")
    try:
        limit_report = json.loads(limit_report_path.read_text())
        limit_case = json.loads(limit_case_path.read_text())
        limit_digest = limit_digest_path.read_text().strip()
    except (OSError, json.JSONDecodeError) as error:
        raise HarnessError(f"S3-34o limit selfcheck artifacts missing: {error}") from error
    if (
        limit_report.get("kind") != "LOCAL-FAULT-INJECTION LIMIT SELF-CHECK ONLY"
        or limit_report.get("stop_reason") != "evaluation_limit"
        or limit_report.get("evaluated_count") != 1
        or limit_report.get("reduced_digest") != limit_digest
        or limit_case.get("input", {}).get("input_digest") != limit_digest
    ):
        raise HarnessError("S3-34o evaluation-limit behavior differs")


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
        "v1_admission_failure_vmhost": artifacts / "od-vmhost-v1-admission-failure-smoke",
        "app_bundle": artifacts / "OpenDesktop-Stage0-Smoke.app",
        "entitlements": ROOT / "apps/vmhost/Configuration/vmhost-smoke.entitlements",
    }


def ensure_real_directory(path: Path, purpose: str) -> None:
    if path.is_symlink():
        raise HarnessError(f"{purpose} must be a real directory: {path}")
    try:
        path.mkdir(parents=True, exist_ok=True)
    except OSError as error:
        raise HarnessError(f"{purpose} is not a directory: {path}") from error
    if path.is_symlink() or not path.is_dir():
        raise HarnessError(f"{purpose} must be a real directory: {path}")


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
    swift_compile = [
        "xcrun", "swiftc", "-module-cache-path", str(paths["swift_modules"]),
        "-parse-as-library", "-O", "-import-objc-header",
    ]
    control_frame = "apps/macos/Sources/Shared/ControlFrame.swift"
    swift_common = [*swift_compile, "apps/vmhost/Sources/ODHostABI/include/VMHostABI.h", control_frame]
    link_args = ["-ldl", "-lpthread", "-lm", *frameworks]
    vmhost_inputs = [
        "apps/macos/Sources/Shared/InputProtocol.swift", "apps/macos/Sources/Shared/InputQueue.swift",
        "apps/macos/Sources/Shared/ControlV2.swift", "apps/vmhost/Sources/BundleLock.swift",
        "apps/vmhost/Sources/Lifecycle.swift", "apps/vmhost/Sources/ControlV2Host.swift",
        "apps/vmhost/Sources/InputChannel.swift", "apps/vmhost/Sources/FrameTransport.swift",
        "apps/vmhost/Sources/VMHostMain.swift", VMHOST_LINUX_DIRECT_SOURCE,
        str(paths["od_host_library"]), *link_args,
    ]
    admission_failure_vmhost = str(paths["v1_admission_failure_vmhost"])
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
            *swift_common, *vmhost_inputs, "-o", str(paths["vmhost"]),
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
            *swift_common, "apps/macos/Sources/Shared/VMHostLaunch.swift", "apps/macos/Sources/launcher.swift",
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
        "process_compile_v1_admission_failure_vmhost": [
            *swift_compile, "tests/fixtures/control/v1_admission_failure_bridge.h",
            control_frame, *vmhost_inputs, "-o", admission_failure_vmhost,
        ],
        "process_sign_v1_admission_failure_vmhost": [
            codesign_path, "--force", "--sign", "-", "--options", "runtime",
            "--entitlements", str(paths["entitlements"]), admission_failure_vmhost,
        ],
        "process_verify_v1_admission_failure_vmhost": [
            codesign_path, "--verify", "--strict", "--verbose=2", admission_failure_vmhost,
        ],
        "process_v1_admission_failure_vmhost_signature": [
            codesign_path, "-d", "--verbose=4", admission_failure_vmhost,
        ],
        "process_v1_admission_failure_vmhost_entitlements": [
            codesign_path, "-d", "--entitlements", ":-", admission_failure_vmhost,
        ],
        "process_control_race": [
            sys.executable, "tests/system/vmhost_control_race_smoke.py",
            str(paths["launcher"]), str(paths["vmhost"]), "--diagnostics-dir",
            str(run_root / "logs" / "process_control_race_children"),
        ],
        "process_control_v1_sessions": [
            sys.executable, "tests/system/vmhost_control_v1_same_code_smoke.py",
            str(paths["vmhost"]),
        ],
        "process_control_v1_code_mismatch": [
            sys.executable, "tests/system/vmhost_control_v1_code_mismatch_smoke.py",
            str(paths["vmhost"]),
        ],
        "process_control_v1_frames": [
            sys.executable, "tests/system/vmhost_control_v1_frames_smoke.py",
            str(paths["vmhost"]),
        ],
        "process_control_v1_admission_failures": [
            sys.executable, "tests/system/vmhost_control_v1_frames_smoke.py",
            admission_failure_vmhost, "--admission-failures",
        ],
        "process_control_v1_stop_eof": [
            sys.executable, "tests/system/vmhost_control_v1_stop_eof_smoke.py",
            str(paths["vmhost"]),
        ],
        "process_control_v1_run_stop_id": [
            sys.executable, "tests/system/vmhost_control_v1_run_stop_id_smoke.py",
            str(paths["vmhost"]),
        ],
        "process_control_v1_run_id_exhaustion": [
            sys.executable, "tests/system/vmhost_control_v1_run_id_exhaustion_smoke.py",
            str(paths["vmhost"]),
        ],
        "process_control_v1_isolation": [
            sys.executable, "tests/system/vmhost_control_v1_isolation_smoke.py",
            str(paths["vmhost"]),
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


def local_vmhost_result_commands(
    run_root: Path, codesign_path: str = "${CODESIGN_PATH}"
) -> dict[str, list[str]]:
    process = vmhost_process_commands(run_root, codesign_path)
    paths = vmhost_process_paths(run_root)
    return {
        "local_s38d_validator_selfcheck": [
            sys.executable,
            "tests/cpu/test_stage3_harness.py",
            "Stage3HarnessValidationTests.test_s3_38d_result_state_validator_rejects_corruption",
        ],
        "local_s38d_codesign_path": ["xcrun", "--find", "codesign"],
        "local_s38d_build_od_host": process["process_build_od_host"],
        "local_s38d_compile_vmhost": process["process_compile_vmhost"],
        "local_s38d_sign_vmhost": process["process_sign_vmhost"],
        "local_s38d_verify_vmhost": process["process_verify_vmhost"],
        "local_s38d_vmhost_signature": process["process_vmhost_signature"],
        "local_s38d_vmhost_entitlements": process["process_vmhost_entitlements"],
        "local_s38d_compile_launcher": process["process_compile_launcher"],
        "local_s38d_sign_launcher": process["process_sign_launcher"],
        "local_s38d_verify_launcher": process["process_verify_launcher"],
        "local_s38d_launcher_signature": process["process_launcher_signature"],
        "local_s38d_launcher_entitlements": process["process_launcher_entitlements"],
        "local_s38d_result_state": [
            sys.executable,
            "tests/system/vmhost_result_state_smoke.py",
            str(paths["launcher"]),
            str(paths["vmhost"]),
            "--diagnostics-dir",
            str(run_root / "logs" / "local_s38d_result_state_children"),
        ],
    }


def local_vmhost_result_command_manifest() -> list[dict[str, object]]:
    run_placeholder = ROOT / "work" / "stage3-harness" / "<RUN_ID>"
    return [
        {
            "step": name,
            "argv": [
                value.replace(str(run_placeholder), "${RUN_ROOT}")
                for value in argv
            ],
            "timeout_seconds": LOCAL_VMHOST_RESULT_TIMEOUTS[name],
        }
        for name, argv in local_vmhost_result_commands(run_placeholder).items()
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
        environment = os.environ.copy()
        artifact_directory = None
        if name == "s3_34p_cached_native_register_v1_failure_replay":
            environment.pop("OD_JIT_NATIVE_REGISTER_V1_FAILURE_TRACE", None)
        if name in {
            "s3_34n_native_register_v1_corpus",
            "s3_34n_native_register_v1_replay",
            "s3_34n_native_register_v1_selfcheck",
            "s3_34o_native_register_v1_reducer",
            "s3_34o_native_register_v1_reducer_selfcheck",
            "s3_34p_cached_native_register_v1_corpus",
            "s3_34p_cached_native_register_v1_replay",
            "s3_34p_cached_native_register_v1_failure_replay",
        }:
            artifact_directory = run_dir / "artifacts" / "native-register-v1"
            environment["OD_JIT_NATIVE_REGISTER_V1_CORPUS_DIR"] = str(artifact_directory)
            environment["OD_JIT_NATIVE_REGISTER_V1_REPLAY_CASE"] = str(
                artifact_directory / "native-register-v1-saved-case.json"
            )
            environment["OD_JIT_NATIVE_REGISTER_V1_REDUCE_CASE"] = str(
                artifact_directory / "native-register-v1-saved-case.json"
            )
        process = subprocess.run(
            argv,
            cwd=ROOT,
            env=environment,
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
            failure = failure or "source_fingerprint_changed: source files changed during the command"
    except HarnessError as error:
        end_fingerprint = None
        failure = failure or f"source_fingerprint_changed: {error}"

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
        "output_tail": combined[-4000:],
        "output_sha256": output_sha256,
    }
    if artifact_directory is not None:
        result["artifact_directory"] = artifact_directory.relative_to(ROOT).as_posix()
    return result


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
            failure = failure or "source_fingerprint_changed: source files changed during the command"
    except HarnessError as error:
        end_fingerprint = None
        failure = failure or f"source_fingerprint_changed: {error}"

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
        "v1_admission_failure_vmhost": process_artifact_record(paths["v1_admission_failure_vmhost"]),
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
            if step_name == "process_control_v1_admission_failures":
                step["binary_inputs"] = process_binary_inputs(artifacts, "v1_admission_failure_vmhost")
                require_process_marker(step, PROCESS_SUCCESS_MARKERS["control_v1_admission_failures"], step_name)
            elif step_name in {"process_control_race", "process_control_v1_sessions",
                             "process_control_v1_code_mismatch",
                             "process_control_v1_frames", "process_control_v1_stop_eof",
                             "process_control_v1_run_stop_id",
                             "process_control_v1_run_id_exhaustion",
                             "process_control_v1_isolation",
                             "process_isolation", "process_generated_fault", "process_app_bundle"}:
                if step_name == "process_control_race":
                    artifacts = collect_vmhost_process_artifacts(run_root)
                    step["binary_inputs"] = process_binary_inputs(
                        artifacts, "launcher", "vmhost"
                    )
                    require_process_marker(step, "VMHost control smoke passed:", step_name)
                elif step_name == "process_control_v1_sessions":
                    step["binary_inputs"] = process_binary_inputs(artifacts, "vmhost")
                    require_process_marker(
                        step,
                        PROCESS_SUCCESS_MARKERS["control_v1_sessions"],
                        step_name,
                    )
                elif step_name == "process_control_v1_code_mismatch":
                    step["binary_inputs"] = process_binary_inputs(artifacts, "vmhost")
                    require_process_marker(
                        step,
                        PROCESS_SUCCESS_MARKERS["control_v1_code_mismatch"],
                        step_name,
                    )
                elif step_name in {"process_control_v1_frames", "process_control_v1_stop_eof",
                                   "process_control_v1_run_stop_id",
                                   "process_control_v1_run_id_exhaustion",
                                   "process_control_v1_isolation"}:
                    step["binary_inputs"] = process_binary_inputs(artifacts, "vmhost")
                    marker_key = (
                        "control_v1_frames"
                        if step_name == "process_control_v1_frames"
                        else "control_v1_stop_eof"
                        if step_name == "process_control_v1_stop_eof"
                        else "control_v1_run_stop_id"
                        if step_name == "process_control_v1_run_stop_id"
                        else "control_v1_isolation"
                        if step_name == "process_control_v1_isolation"
                        else "control_v1_run_id_exhaustion"
                    )
                    require_process_marker(
                        step,
                        PROCESS_SUCCESS_MARKERS[marker_key],
                        step_name,
                    )
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
                "process_v1_admission_failure_vmhost_signature",
                "process_bundle_signature", "process_bundle_vmhost_signature",
                "process_bundle_launcher_signature",
            }:
                process_signature_output(step, requires_jit=False, step_name=step_name)
            elif step_name in {
                "process_vmhost_entitlements", "process_fault_helper_entitlements",
                "process_v1_admission_failure_vmhost_entitlements",
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


def run_local_vmhost_result_state(
    summary: dict, run_root: Path, fingerprint: str
) -> None:
    host_target, _ = rust_host()
    require_local_arm64_backend(host_target)
    paths = vmhost_process_paths(run_root)
    ensure_real_directory(paths["artifacts"], "local VMHost artifacts directory")
    paths["swift_modules"].mkdir(parents=True, exist_ok=False)
    (run_root / "logs" / "local_s38d_result_state_children").mkdir()

    codesign_path = "${CODESIGN_PATH}"
    for step_name in LOCAL_VMHOST_RESULT_STEPS:
        commands = local_vmhost_result_commands(run_root, codesign_path)
        step = run_process_step(
            step_name,
            commands[step_name],
            run_root,
            LOCAL_VMHOST_RESULT_TIMEOUTS[step_name],
            fingerprint,
        )
        step["suite"] = LOCAL_VMHOST_RESULT_SUITE
        step["expected_returncode"] = 0

        if step_name == "local_s38d_codesign_path" and step["status"] == "passed":
            raw_path = step.get("output_tail", "").strip()
            tool_path = Path(raw_path)
            if not tool_path.is_absolute() or tool_path.is_symlink() or not tool_path.is_file():
                step["status"] = "failed"
                step["failure"] = "xcrun codesign path is invalid"
            else:
                codesign_path = str(tool_path)

        if step["status"] == "passed":
            try:
                validate_local_vmhost_result_step(
                    step, step_name, run_root, fingerprint, codesign_path
                )
                if step_name == "local_s38d_vmhost_signature":
                    process_signature_output(
                        step, requires_jit=False, step_name=step_name
                    )
                elif step_name == "local_s38d_vmhost_entitlements":
                    process_entitlements_output(
                        step, requires_jit=True, step_name=step_name
                    )
                elif step_name == "local_s38d_launcher_signature":
                    process_signature_output(
                        step, requires_jit=False, step_name=step_name
                    )
                elif step_name == "local_s38d_launcher_entitlements":
                    process_entitlements_output(
                        step, requires_jit=False, step_name=step_name
                    )
                elif step_name == "local_s38d_validator_selfcheck":
                    output = step.get("output_tail", "")
                    expected_test = (
                        "test_s3_38d_result_state_validator_rejects_corruption"
                    )
                    if (
                        not isinstance(output, str)
                        or f"{expected_test} (__main__.Stage3HarnessValidationTests.{expected_test}) ... ok"
                        not in output
                        or "Ran 1 test" not in output
                        or "OK" not in output
                    ):
                        raise HarnessError("S3-38d validator selfcheck did not run exactly once")
                elif step_name == "local_s38d_result_state":
                    step["child_diagnostics"] = child_log_evidence(
                        run_root, "local_s38d_result_state"
                    )
                    validate_local_vmhost_result_observation(step, run_root)
            except HarnessError as error:
                step["status"] = "failed"
                step["failure"] = str(error)

        summary["steps"][step_name] = step
        replace_json(run_root / "summary.json", summary)
        if step["status"] != "passed":
            raise HarnessError(
                f"required local VMHost RESULT step failed: {step_name}: {step.get('failure')}"
            )

    artifacts = local_vmhost_result_artifacts(run_root, Path(codesign_path))
    summary["local_vmhost_result_artifacts"] = artifacts
    summary["suites"][LOCAL_VMHOST_RESULT_SUITE] = {
        "status": "passed",
        "steps": list(LOCAL_VMHOST_RESULT_STEPS),
        "artifacts": artifacts,
        "automatic_retry": False,
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
            "expected_unique_inputs": MEMORY_CORPUS_UNIQUE_INPUTS,
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
            "success_markers": PROCESS_SUCCESS_MARKERS,
            "automatic_retry": False,
        },
        "local_vmhost_result_state": {
            "host_requirement": {
                "system": "darwin",
                "machine": "arm64",
                "rust_target_prefix": "aarch64-apple-darwin",
            },
            "commands": local_vmhost_result_command_manifest(),
            "inputs": [
                {"path": name, "sha256": sha256_file(ROOT / name)}
                for name in LOCAL_VMHOST_RESULT_SOURCE_PATHS
            ],
            "profile": {
                "loop_code_hex": LOCAL_VMHOST_RESULT_LOOP,
                "fixed_budgets": list(LOCAL_VMHOST_RESULT_BUDGETS),
                "initial_gprs": list(LOCAL_VMHOST_RESULT_INITIAL_GPRS),
                "initial_rflags": LOCAL_VMHOST_RESULT_INITIAL_RFLAGS,
                "launcher_initial_rflags": LOCAL_VMHOST_RESULT_LAUNCHER_RFLAGS,
                "result_format": "<6QIIQ",
                "result_bytes": 64,
                "stop_budget": LOCAL_VMHOST_RESULT_STOP_BUDGET,
                "stop_delays_ms": [0, 5],
                "vmhost_handshake_timeout_seconds": 30,
                "control_response_timeout_seconds": 5,
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
        or memory.get("expected_unique_inputs") != MEMORY_CORPUS_UNIQUE_INPUTS
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
    if process.get("success_markers") != PROCESS_SUCCESS_MARKERS:
        raise HarnessError("VMHost process success markers differ from the pinned suite")
    if process.get("build_profile", {}).get("commands") != vmhost_process_command_manifest():
        raise HarnessError("VMHost process command manifest differs from the exact run profile")
    inputs = process.get("inputs")
    expected_inputs = [
        {"path": name, "sha256": sha256_file(ROOT / name)}
        for name in PROCESS_SOURCE_PATHS
    ]
    if inputs != expected_inputs:
        raise HarnessError("VMHost process source inputs differ from the pinned manifest")
    local_result = value.get("local_vmhost_result_state")
    expected_local_result = {
        "host_requirement": {
            "system": "darwin",
            "machine": "arm64",
            "rust_target_prefix": "aarch64-apple-darwin",
        },
        "commands": local_vmhost_result_command_manifest(),
        "inputs": [
            {"path": name, "sha256": sha256_file(ROOT / name)}
            for name in LOCAL_VMHOST_RESULT_SOURCE_PATHS
        ],
        "profile": {
            "loop_code_hex": LOCAL_VMHOST_RESULT_LOOP,
            "fixed_budgets": list(LOCAL_VMHOST_RESULT_BUDGETS),
            "initial_gprs": list(LOCAL_VMHOST_RESULT_INITIAL_GPRS),
            "initial_rflags": LOCAL_VMHOST_RESULT_INITIAL_RFLAGS,
            "launcher_initial_rflags": LOCAL_VMHOST_RESULT_LAUNCHER_RFLAGS,
            "result_format": "<6QIIQ",
            "result_bytes": 64,
            "stop_budget": LOCAL_VMHOST_RESULT_STOP_BUDGET,
            "stop_delays_ms": [0, 5],
            "vmhost_handshake_timeout_seconds": 30,
            "control_response_timeout_seconds": 5,
        },
        "automatic_retry": False,
    }
    if local_result != expected_local_result:
        raise HarnessError("local VMHost RESULT command profile or inputs differ")
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


def validate_local_vmhost_result_observation(step: dict, run_root: Path) -> None:
    output = step.get("output_tail")
    marker = "vmhost_result_state_observation="
    rows = [line[len(marker):] for line in output.splitlines() if line.startswith(marker)] \
        if isinstance(output, str) else []
    if len(rows) != 1 or "VMHost RESULT state smoke passed:" not in output:
        raise HarnessError("S3-38d process step omitted its unique result-state observation")
    try:
        observed = json.loads(rows[0])
    except json.JSONDecodeError as error:
        raise HarnessError(f"S3-38d result-state observation is malformed: {error}") from error

    def expected_gprs(budget: int, retired: int) -> list[int]:
        return [
            LOCAL_VMHOST_RESULT_INITIAL_GPRS[0] if retired == 0 else 1,
            *LOCAL_VMHOST_RESULT_INITIAL_GPRS[1:],
        ]

    raw_budget = observed.get("raw_budget") if isinstance(observed, dict) else None
    launcher_budget = observed.get("launcher_budget") if isinstance(observed, dict) else None
    stops = observed.get("stop") if isinstance(observed, dict) else None
    if not all(isinstance(value, list) for value in (raw_budget, launcher_budget, stops)):
        raise HarnessError("S3-38d observation fields are missing")
    if len(raw_budget) != len(LOCAL_VMHOST_RESULT_BUDGETS) \
            or len(launcher_budget) != len(LOCAL_VMHOST_RESULT_BUDGETS) or len(stops) != 2:
        raise HarnessError("S3-38d observation case count differs from the pinned profile")

    for index, budget in enumerate(LOCAL_VMHOST_RESULT_BUDGETS, start=1):
        raw = raw_budget[index - 1]
        expected = {
            "gpr": expected_gprs(budget, budget),
            "rip": 0 if budget % 2 == 0 else 10,
            "rflags": LOCAL_VMHOST_RESULT_INITIAL_RFLAGS,
            "reason": 1,
            "reserved": 0,
            "instructions_executed": budget,
            "request_id": 0x380D0000 + index,
            "payload_length": 64,
        }
        if raw != expected:
            raise HarnessError(f"S3-38d raw RESULT budget binding differs: {budget}")
        launcher = launcher_budget[index - 1]
        projected = {
            "gpr": expected_gprs(budget, budget),
            "instructions_executed": budget,
            "reason": 1,
            "rflags": LOCAL_VMHOST_RESULT_LAUNCHER_RFLAGS,
            "rip": 0 if budget % 2 == 0 else 10,
        }
        if launcher != projected:
            raise HarnessError(f"S3-38d launcher RESULT projection differs: {budget}")

    expected_stop_ids = (0x380D1001, 0x380D1002)
    for index, (result, request_id) in enumerate(zip(stops, expected_stop_ids)):
        if not isinstance(result, dict):
            raise HarnessError(f"S3-38d STOP result is malformed: case {index}")
        retired = result.get("instructions_executed")
        if isinstance(retired, bool) or not isinstance(retired, int) \
                or not 0 <= retired <= LOCAL_VMHOST_RESULT_STOP_BUDGET:
            raise HarnessError(f"S3-38d STOP retirement count is invalid: case {index}")
        expected_stop = {
            "gpr": expected_gprs(LOCAL_VMHOST_RESULT_STOP_BUDGET, retired),
            "rip": 0 if retired % 2 == 0 else 10,
            "rflags": LOCAL_VMHOST_RESULT_INITIAL_RFLAGS,
            "reason": 2,
            "reserved": 0,
            "instructions_executed": retired,
            "request_id": request_id,
            "payload_length": 64,
            "ack_order": ["RUN_ACK", "STOP_ACK", "RESULT"],
            "stop_observation": (
                "early_stop_no_guest_progress" if retired == 0
                else "stop_after_observed_guest_progress"
            ),
        }
        if result != expected_stop:
            raise HarnessError(f"S3-38d STOP RESULT boundary or ordering differs: case {index}")

    labels = [
        *(f"raw-budget-{budget}" for budget in LOCAL_VMHOST_RESULT_BUDGETS),
        "raw-stop-immediate",
        "raw-stop-delayed",
        *(f"launcher-budget-{budget}" for budget in LOCAL_VMHOST_RESULT_BUDGETS),
    ]
    expected_paths = sorted(
        f"local_s38d_result_state_children/{label}.{stream}.log"
        for label in labels
        for stream in ("stdout", "stderr")
    )
    children = child_log_evidence(run_root, "local_s38d_result_state")
    observed_paths = local_vmhost_result_child_paths(run_root, children)
    if observed_paths != expected_paths:
        raise HarnessError("S3-38d child diagnostics do not cover each launcher and VMHost")
    if step.get("child_diagnostics") != children:
        raise HarnessError("S3-38d child log digests differ from saved files")


def local_vmhost_result_child_paths(run_root: Path, children: list[dict]) -> list[str]:
    logs_relative = (run_root / "logs").relative_to(ROOT)
    result = []
    for child in children:
        path = Path(child.get("path", ""))
        try:
            relative = path.relative_to(logs_relative)
        except ValueError as error:
            raise HarnessError("S3-38d child diagnostic escaped its run logs") from error
        result.append(relative.as_posix())
    return result


def local_vmhost_result_artifacts(run_root: Path, codesign_path: Path) -> dict[str, object]:
    paths = vmhost_process_paths(run_root)
    if not codesign_path.is_absolute() or codesign_path.is_symlink() or not codesign_path.is_file():
        raise HarnessError("S3-38d codesign binary path is invalid")
    return {
        "od_host_library": process_artifact_record(paths["od_host_library"]),
        "vmhost": process_artifact_record(paths["vmhost"]),
        "launcher": process_artifact_record(paths["launcher"]),
        "entitlements_source": process_artifact_record(paths["entitlements"]),
        "codesign_binary": {
            "path": str(codesign_path),
            "sha256": sha256_file(codesign_path),
            "size_bytes": codesign_path.stat().st_size,
        },
    }


def validate_local_vmhost_result_step(
    step: dict,
    step_name: str,
    run_root: Path,
    fingerprint: str,
    codesign_path: str,
) -> None:
    expected_argv = local_vmhost_result_commands(run_root, codesign_path).get(step_name)
    if expected_argv is None:
        raise HarnessError(f"unknown local VMHost RESULT step: {step_name}")
    if (
        step.get("name") != step_name
        or step.get("suite") != LOCAL_VMHOST_RESULT_SUITE
        or step.get("argv") != expected_argv
        or step.get("command") != shlex.join(expected_argv)
        or step.get("status") != "passed"
        or step.get("returncode") != 0
        or step.get("expected_returncode") != 0
        or step.get("failure") is not None
        or step.get("timeout_seconds") != LOCAL_VMHOST_RESULT_TIMEOUTS[step_name]
        or step.get("process_group_cleanup") != "empty"
        or step.get("source_fingerprint_after") != fingerprint
    ):
        raise HarnessError(f"local VMHost RESULT step evidence is missing, forged, or failed: {step_name}")
    duration = step.get("duration_seconds")
    if isinstance(duration, bool) or not isinstance(duration, (int, float)) or duration < 0:
        raise HarnessError(f"local VMHost RESULT step has invalid duration: {step_name}")

    contents: dict[str, bytes] = {}
    for field, basename in {
        "log": f"{step_name}.log",
        "stdout_log": f"{step_name}.stdout.log",
        "stderr_log": f"{step_name}.stderr.log",
    }.items():
        expected_path = run_root / "logs" / basename
        if step.get(field) != expected_path.relative_to(ROOT).as_posix() \
                or expected_path.is_symlink() or not expected_path.is_file():
            raise HarnessError(f"local VMHost RESULT log path is invalid: {step_name}/{field}")
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
        raise HarnessError(f"local VMHost RESULT log digest or stream evidence differs: {step_name}")


def validate_local_vmhost_result_state(summary: dict, manifest: dict, run_root: Path) -> None:
    suite = summary.get("suites", {}).get(LOCAL_VMHOST_RESULT_SUITE)
    if not isinstance(suite, dict) or suite.get("status") != "passed" \
            or suite.get("steps") != list(LOCAL_VMHOST_RESULT_STEPS):
        raise HarnessError("local VMHost RESULT suite membership is missing or forged")
    steps = summary.get("steps", {})
    codesign = steps.get("local_s38d_codesign_path", {})
    codesign_path = codesign.get("output_tail", "").strip()
    if not Path(codesign_path).is_absolute():
        raise HarnessError("S3-38d codesign tool path is missing or not absolute")
    fingerprint = summary.get("source", {}).get("fingerprint_start")
    for step_name in LOCAL_VMHOST_RESULT_STEPS:
        step = steps.get(step_name)
        if not isinstance(step, dict):
            raise HarnessError(f"local VMHost RESULT step is missing: {step_name}")
        validate_local_vmhost_result_step(step, step_name, run_root, fingerprint, codesign_path)
    selfcheck = steps["local_s38d_validator_selfcheck"].get("output_tail", "")
    expected_selfcheck = (
        "test_s3_38d_result_state_validator_rejects_corruption"
    )
    if (
        not isinstance(selfcheck, str)
        or f"{expected_selfcheck} (__main__.Stage3HarnessValidationTests.{expected_selfcheck}) ... ok"
        not in selfcheck
        or "Ran 1 test" not in selfcheck
        or "OK" not in selfcheck
    ):
        raise HarnessError("S3-38d validator selfcheck evidence is missing or forged")
    process_signature_output(
        steps["local_s38d_vmhost_signature"], requires_jit=False,
        step_name="local_s38d_vmhost_signature",
    )
    process_entitlements_output(
        steps["local_s38d_vmhost_entitlements"], requires_jit=True,
        step_name="local_s38d_vmhost_entitlements",
    )
    process_signature_output(
        steps["local_s38d_launcher_signature"], requires_jit=False,
        step_name="local_s38d_launcher_signature",
    )
    process_entitlements_output(
        steps["local_s38d_launcher_entitlements"], requires_jit=False,
        step_name="local_s38d_launcher_entitlements",
    )
    validate_local_vmhost_result_observation(steps["local_s38d_result_state"], run_root)
    codesign_path_value = Path(codesign_path)
    artifacts = local_vmhost_result_artifacts(run_root, codesign_path_value)
    if summary.get("local_vmhost_result_artifacts") != artifacts \
            or suite.get("artifacts") != artifacts:
        raise HarnessError("local VMHost RESULT binary hashes differ from current run outputs")
    if suite.get("automatic_retry") is not False:
        raise HarnessError("local VMHost RESULT suite permits automatic retry")
    if not isinstance(manifest.get("local_vmhost_result_state"), dict) \
            or manifest["local_vmhost_result_state"].get("automatic_retry") is not False:
        raise HarnessError("local VMHost RESULT manifest permits automatic retry")


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
        "v1_admission_failure_vmhost",
        "app_bundle", "bundle_vmhost", "bundle_launcher",
    }:
        raise HarnessError("VMHost process artifact set is incomplete")

    for name in ("process_vmhost_signature", "process_launcher_signature",
                 "process_fault_helper_signature", "process_v1_admission_failure_vmhost_signature",
                 "process_bundle_signature",
                 "process_bundle_vmhost_signature", "process_bundle_launcher_signature"):
        process_signature_output(
            steps[name], requires_jit=False, step_name=name
        )
    for name in ("process_vmhost_entitlements", "process_fault_helper_entitlements",
                 "process_v1_admission_failure_vmhost_entitlements",
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
    require_process_marker(
        steps["process_control_v1_sessions"],
        PROCESS_SUCCESS_MARKERS["control_v1_sessions"],
        "process_control_v1_sessions",
    )
    require_process_marker(
        steps["process_control_v1_code_mismatch"],
        PROCESS_SUCCESS_MARKERS["control_v1_code_mismatch"],
        "process_control_v1_code_mismatch",
    )
    require_process_marker(
        steps["process_control_v1_frames"],
        PROCESS_SUCCESS_MARKERS["control_v1_frames"],
        "process_control_v1_frames",
    )
    require_process_marker(
        steps["process_control_v1_admission_failures"],
        PROCESS_SUCCESS_MARKERS["control_v1_admission_failures"],
        "process_control_v1_admission_failures",
    )
    require_process_marker(
        steps["process_control_v1_stop_eof"],
        PROCESS_SUCCESS_MARKERS["control_v1_stop_eof"],
        "process_control_v1_stop_eof",
    )
    require_process_marker(
        steps["process_control_v1_run_stop_id"],
        PROCESS_SUCCESS_MARKERS["control_v1_run_stop_id"],
        "process_control_v1_run_stop_id",
    )
    require_process_marker(
        steps["process_control_v1_run_id_exhaustion"],
        PROCESS_SUCCESS_MARKERS["control_v1_run_id_exhaustion"],
        "process_control_v1_run_id_exhaustion",
    )
    require_process_marker(
        steps["process_control_v1_isolation"],
        PROCESS_SUCCESS_MARKERS["control_v1_isolation"],
        "process_control_v1_isolation",
    )
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
        "process_control_v1_sessions": process_binary_inputs(expected_artifacts, "vmhost"),
        "process_control_v1_code_mismatch": process_binary_inputs(expected_artifacts, "vmhost"),
        "process_control_v1_frames": process_binary_inputs(expected_artifacts, "vmhost"),
        "process_control_v1_admission_failures": process_binary_inputs(
            expected_artifacts, "v1_admission_failure_vmhost"
        ),
        "process_control_v1_stop_eof": process_binary_inputs(expected_artifacts, "vmhost"),
        "process_control_v1_run_stop_id": process_binary_inputs(expected_artifacts, "vmhost"),
        "process_control_v1_run_id_exhaustion": process_binary_inputs(expected_artifacts, "vmhost"),
        "process_control_v1_isolation": process_binary_inputs(expected_artifacts, "vmhost"),
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
        validate_local_vmhost_result_state(summary, manifest, run_root)
        validate_native_register_v1_artifacts(steps, run_root)
        memory_suite = suites.get("memory_corpus", {})
        if (
            memory_suite.get("status") != "passed"
            or memory_suite.get("unique_inputs") != MEMORY_CORPUS_UNIQUE_INPUTS
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
    elif selection == "vmhost-result-state":
        if (
            summary.get("status") != "vmhost_result_state_only"
            or summary.get("result_scope") != "vmhost_result_state_only"
        ):
            raise HarnessError("S3-38d dedicated suite scope is missing or forged")
        validate_vmhost_result_state_selection(suites, steps)
        validate_local_vmhost_result_state(summary, manifest, run_root)
    else:
        raise HarnessError("Stage 3 suite selection is missing or unsupported")
    source = summary.get("source", {})
    validate_fingerprint(source.get("fingerprint_start", ""), source.get("fingerprint_end", ""))


def validate_vmhost_result_state_selection(suites: dict, steps: dict) -> None:
    validate_unselected_suites(suites, (PROCESS_SUITE_NAME, *UNSELECTED_SUITES))
    if suites.get(LOCAL_VMHOST_RESULT_SUITE, {}).get("status") != "passed":
        raise HarnessError("selected S3-38d suite must be passed")
    if suites.get(PROCESS_SUITE_NAME) != {"status": "not_run"}:
        raise HarnessError("VMHost process suite must remain not_run in S3-38d selection")
    if any(
        suites.get(name) != {"status": "not_run"}
        for name in LOCAL_SUITE_NAMES
        if name != LOCAL_VMHOST_RESULT_SUITE
    ):
        raise HarnessError("non-S3-38d local suites must remain not_run")
    if any(
        steps.get(name, {}).get("status") != "not_run"
        for name in ALL_STEPS
        if name not in LOCAL_VMHOST_RESULT_STEPS
    ):
        raise HarnessError("non-S3-38d steps must remain not_run")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--suite",
        choices=("local", "vmhost-process", "vmhost-result-state"),
        default="local",
    )
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
        "result_scope": {
            "local": "local_only",
            "vmhost-process": "vmhost_process_only",
            "vmhost-result-state": "vmhost_result_state_only",
        }[args.suite],
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

        if args.suite == "vmhost-result-state":
            run_local_vmhost_result_state(summary, run_dir, fingerprint)
            summary["source"]["fingerprint_end"] = source_fingerprint()
            validate_fingerprint(fingerprint, summary["source"]["fingerprint_end"])
            summary["status"] = "vmhost_result_state_only"
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
                600,
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
                summary["steps"][name]["unique_inputs"] = MEMORY_CORPUS_UNIQUE_INPUTS
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
                    if step_name.startswith(("s3_34n_native_register_v1_", "s3_34o_native_register_v1_", "s3_34p_cached_native_register_v1_")):
                        attach_native_register_v1_evidence(step, run_dir, step_name)
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
            "unique_inputs": MEMORY_CORPUS_UNIQUE_INPUTS,
            "replays_per_input": 2,
            "test_outcomes": summary["steps"]["memory_corpus"]["test_outcomes"],
            "s3_34h_exact_gate": summary["steps"]["memory_s3_34h_exact_gate"],
        }
        run_local_vmhost_result_state(summary, run_dir, fingerprint)
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
