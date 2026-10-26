//! Real CLI/interpreter checks; expected values are hand-calculated from Intel SDM 093.
use od_testkit::HexU64;
use od_testkit::result::Observation;
use od_testkit::v2::{ExpectedExit, Fixture, LoadedFixture, Outcome, RunResult, Snapshot};
use std::fs;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

const LOAD_ADD_STORE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json");
const WIDTHS: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/mov_width_aliasing.case.json");
const REX_RIP: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/mov_rex_rip_relative.case.json");
const RET66: &[u8] = include_bytes!("../../../tests/fixtures/x86/stage2/s2_17t_ret66.case.json");
const RET66_IGNORED_REX: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_17t_ret66_ignored_rex.case.json");
const XCHG_QWORD_MEMORY_ALIAS: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_21a_xchg_qword_memory_alias.case.json");
const XADD_QWORD_REGISTER_ALIAS: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_21b_xadd_qword_register_alias.case.json");
const XADD_QWORD_MEMORY_ALIAS_SPLIT: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_21d_xadd_qword_memory_alias_split.case.json"
);
const LOCK_XADD_QWORD_MEMORY_ALIAS_SPLIT: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_21f_lock_xadd_qword_memory_alias_split.case.json"
);
const MFENCE_STORE_FENCE_LOAD: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_21g_mfence_store_fence_load.case.json");
const FSTP_INVALID_SOURCE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30w_fstp_invalid_source.case.json");
const FST_INVALID_SOURCE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30x_fst_invalid_source.case.json");
const FCHS_NORMAL: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30y_fchs_normal.case.json");
const FCHS_INVALID_SOURCE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30y_fchs_invalid_source.case.json");
const FRNDINT_TIE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31a_frndint_tie.case.json");
const FRNDINT_FNSTSW: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31a_frndint_fnstsw.case.json");
const FRNDINT_SEEDED_DE_FNSTSW: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31dj_frndint_seeded_de_fnstsw.case.json");
const FRNDINT_SEEDED_UNNORMAL_FNSTSW_FNCLEX: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31dq_frndint_seeded_unnormal_fnstsw_fnclex.case.json"
);
const FRNDINT_SEEDED_PSEUDO_INFINITY_FNSTSW_FNCLEX: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31dr_frndint_seeded_pseudo_infinity_fnstsw_fnclex.case.json"
);
const FRNDINT_SEEDED_PSEUDO_NAN_Q0_FNSTSW_FNCLEX: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31ds_frndint_seeded_pseudo_nan_q0_fnstsw_fnclex.case.json"
);
const FRNDINT_SEEDED_PSEUDO_NAN_Q1_FNSTSW_FNCLEX: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31ds_frndint_seeded_pseudo_nan_q1_fnstsw_fnclex.case.json"
);
const FRNDINT_SEEDED_UE_FNSTSW: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31dk_frndint_seeded_ue_fnstsw.case.json");
const FST_M32_UNDERFLOW_FRNDINT_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31dk_fst_m32_underflow_frndint_fnstsw.case.json"
);
const FRNDINT_SEEDED_OE_FNSTSW: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31dl_frndint_seeded_oe_fnstsw.case.json");
const FRNDINT_SEEDED_ZE_OE_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31dm_frndint_seeded_ze_oe_fnstsw.case.json"
);
const FRNDINT_SEEDED_SF_IE_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31dn_frndint_seeded_sf_ie_fnstsw.case.json"
);
const FST_M32_OVERFLOW_FRNDINT_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31dl_fst_m32_overflow_frndint_fnstsw.case.json"
);
const FRNDINT_INVALID_TAG: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31a_frndint_invalid_tag.case.json");
const FRNDINT_INFINITY: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31de_frndint_negative_infinity_fnstsw.case.json"
);
const FLDCW_FRNDINT_FNSTSW: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31i_fldcw_frndint_fnstsw.case.json");
const FSTP_M32FP: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31j_fstp_m32fp.case.json");
const FSTP_M32FP_FNSTSW: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31j_fstp_m32fp_fnstsw.case.json");
const FST_M32FP: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31k_fst_m32fp.case.json");
const FST_M32FP_FNSTSW: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31k_fst_m32fp_fnstsw.case.json");
const FST_M32FP_POSITIVE_ZERO: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31al_fst_m32fp_positive_zero.case.json");
const FST_M32FP_NEGATIVE_ZERO_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31al_fst_m32fp_negative_zero_fnstsw.case.json"
);
const FSTP_M32FP_POSITIVE_ZERO: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31al_fstp_m32fp_positive_zero.case.json");
const FSTP_M32FP_NEGATIVE_ZERO_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31al_fstp_m32fp_negative_zero_fnstsw.case.json"
);
const FST_M64FP_POSITIVE_ZERO: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31am_fst_m64fp_positive_zero.case.json");
const FST_M64FP_NEGATIVE_ZERO_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31am_fst_m64fp_negative_zero_fnstsw.case.json"
);
const FSTP_M64FP_POSITIVE_ZERO: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31am_fstp_m64fp_positive_zero.case.json");
const FSTP_M64FP_NEGATIVE_ZERO_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31am_fstp_m64fp_negative_zero_fnstsw.case.json"
);
const FILD_M64INT: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31p_fild_m64int_success.case.json");
const FILD_M64INT_FNSTSW: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31p_fild_m64int_fnstsw.case.json");
const FLD_M32_SUBNORMAL: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31y_fld_m32fp_min_subnormal.case.json");
const FRNDINT_FLD_M32_SUBNORMAL: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31dj_frndint_fld_m32_subnormal_fnstsw.case.json"
);
const FLD_M32_SUBNORMAL_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31y_fld_m32fp_negative_min_subnormal_fnstsw.case.json"
);
const FLD_M32_SUBNORMAL_FST_M64: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31az_fld_m32_subnormal_fst_m64fp.case.json"
);
const FLD_M32_NEGATIVE_LARGEST_SUBNORMAL_FSTP_M64_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31az_fld_m32_largest_negative_subnormal_fstp_m64fp_fnstsw.case.json"
);
const FLD_M32_INFINITY: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31af_fld_m32fp_infinity.case.json");
const FLD_M32_INFINITY_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31af_fld_m32fp_negative_infinity_fnstsw.case.json"
);
const FLD_M32_QNAN: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ah_fld_m32fp_qnan.case.json");
const FLD_M32_NEGATIVE_QNAN_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31ah_fld_m32fp_negative_qnan_payload_fnstsw.case.json"
);
const FLD_M32_SNAN: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31aj_fld_m32fp_snan.case.json");
const FLD_M32_NEGATIVE_SNAN_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31aj_fld_m32fp_negative_snan_payload_fnstsw.case.json"
);
const FLD_M32_INVALID_STATUS: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ak_fld_m32fp_invalid_status.case.json");
const FLD_M64_INVALID_STATUS: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ak_fld_m64fp_invalid_status.case.json");
const FLD_M32_SEEDED_UE_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31dt_fld_m32fp_seeded_ue_fnstsw.case.json"
);
const FLD_M64_SEEDED_UE_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31dt_fld_m64fp_seeded_ue_fnstsw.case.json"
);
const FLD_M64_SUBNORMAL: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31z_fld_m64fp_min_subnormal.case.json");
const FRNDINT_FLD_M64_SUBNORMAL: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31dj_frndint_fld_m64_subnormal_fnstsw.case.json"
);
const FLD_M64_SUBNORMAL_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31z_fld_m64fp_negative_min_subnormal_fnstsw.case.json"
);
const FLD_M64_INFINITY: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ae_fld_m64fp_infinity.case.json");
const FLD_M64_QNAN: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ag_fld_m64fp_qnan.case.json");
const FLD_M64_NEGATIVE_QNAN_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31ag_fld_m64fp_negative_qnan_payload_fnstsw.case.json"
);
const FLD_M64_SNAN: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ai_fld_m64fp_snan.case.json");
const FLD_M64_NEGATIVE_SNAN_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31ai_fld_m64fp_negative_snan_payload_fnstsw.case.json"
);
const FLD_M80_INFINITY: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31aa_fld_m80fp_infinity.case.json");
const FLD_M80_QNAN: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ab_fld_m80fp_qnan.case.json");
const FLD_M80_SNAN: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ac_fld_m80fp_snan.case.json");
const FLD_M80_PSEUDO_DENORMAL: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31ad_fld_m80fp_pseudo_denormal.case.json"
);
const FSTP_M80_QNAN: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ar_fstp_m80fp_qnan.case.json");
const FSTP_M80_NEGATIVE_INDEFINITE_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31ar_fstp_m80fp_negative_indefinite_fnstsw.case.json"
);
const FSTP_M80_SNAN: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31as_fstp_m80fp_snan.case.json");
const FSTP_M80_NEGATIVE_SNAN_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31as_fstp_m80fp_negative_snan_payload_fnstsw.case.json"
);
const FSTP_M80_SUBNORMAL: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31at_fstp_m80fp_subnormal.case.json");
const FSTP_M80_NEGATIVE_SUBNORMAL_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31at_fstp_m80fp_negative_subnormal_fnstsw.case.json"
);
const FSTP_M80_STICKY_STATUS: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ba_fstp_m80fp_sticky_status.case.json");
const FSTP_M80_NEGATIVE_SNAN_STICKY_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31ba_fstp_m80fp_negative_snan_sticky_fnstsw.case.json"
);
const FSTP_M80_NEGATIVE_SNAN_STICKY_UE_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bi_fstp_m80fp_negative_snan_sticky_ue_fnstsw.case.json"
);
const FSTP_M80_NEGATIVE_SNAN_STICKY_OE_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bs_fstp_m80fp_negative_snan_sticky_oe_fnstsw.case.json"
);
const FST_M64_QNAN: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31au_fst_m64fp_positive_quiet_nan.case.json"
);
const FSTP_M64_NEGATIVE_QNAN_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31au_fstp_m64fp_negative_quiet_nan_fnstsw.case.json"
);
const FST_M64_SNAN: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31aw_fst_m64fp_positive_signaling_nan.case.json"
);
const FSTP_M64_NEGATIVE_SNAN_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31aw_fstp_m64fp_negative_signaling_nan_fnstsw.case.json"
);
const FST_M32_QNAN: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31av_fst_m32fp_positive_qnan.case.json");
const FSTP_M32_NEGATIVE_QNAN_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31av_fstp_m32fp_negative_qnan_payload_fnstsw.case.json"
);
const FST_M32_SNAN: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ax_fst_m32fp_positive_snan.case.json");
const FSTP_M32_NEGATIVE_SNAN_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31ax_fstp_m32fp_negative_snan_payload_fnstsw.case.json"
);
const FST_M32_SNAN_STICKY_IE: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31ay_fst_m32fp_positive_snan_sticky_ie.case.json"
);
const FLD_M32_MAX_FINITE: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bb_fld_m32fp_positive_max_finite.case.json"
);
const FST_M32_MAX_FINITE: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bb_fst_m32fp_positive_max_finite.case.json"
);
const FSTP_M32_NEGATIVE_MAX_FINITE_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bb_fstp_m32fp_negative_max_finite_fnstsw.case.json"
);
const FLD_M64_MAX_FINITE: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bc_fld_m64fp_positive_max_finite.case.json"
);
const FST_M64_MAX_FINITE: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bc_fst_m64fp_positive_max_finite.case.json"
);
const FSTP_M64_NEGATIVE_MAX_FINITE_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bc_fstp_m64fp_negative_max_finite_fnstsw.case.json"
);
const FST_M32_POSITIVE_MIN_SUBNORMAL: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bd_fst_m32fp_positive_min_subnormal.case.json"
);
const FSTP_M32_NEGATIVE_MIN_SUBNORMAL_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bd_fstp_m32fp_negative_min_subnormal_fnstsw.case.json"
);
const FST_M32_POSITIVE_INEXACT_SUBNORMAL_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bf_fst_m32fp_positive_1_5_minsub_fnstsw.case.json"
);
const FST_M32_STICKY_UE_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bh_fst_m32fp_sticky_ue_fnstsw.case.json"
);
const FST_M32_EXTREME_UNDERFLOW_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bj_fst_m32fp_positive_0_75_minsub_fnstsw.case.json"
);
const FST_M32_BINARY80_SUBNORMAL_UNDERFLOW_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bl_fst_m32fp_negative_min_subnormal_fnstsw.case.json"
);
const FST_M32_BINARY80_MIN_NORMAL_TIE_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bn_fst_m32fp_positive_minimum_normal_nearest_tie_fnstsw.case.json"
);
const FSTP_M32_BINARY80_MIN_NORMAL_DIRECTED_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bn_fstp_m32fp_negative_minimum_normal_directed_fnstsw.case.json"
);
const FST_M64_BINARY80_MIN_NORMAL_TIE_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bo_fst_m64fp_positive_minimum_normal_nearest_tie_fnstsw.case.json"
);
const FSTP_M64_BINARY80_MIN_NORMAL_DIRECTED_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bo_fstp_m64fp_negative_minimum_normal_directed_fnstsw.case.json"
);
const FST_M32_BINARY80_INFINITY_OVERFLOW_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bp_fst_m32fp_positive_infinity_overflow_nearest_fnstsw.case.json"
);
const FSTP_M32_BINARY80_INFINITY_OVERFLOW_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bp_fstp_m32fp_negative_infinity_overflow_directed_fnstsw.case.json"
);
const FST_M64_BINARY80_INFINITY_OVERFLOW_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bq_fst_m64fp_positive_infinity_overflow_nearest_fnstsw.case.json"
);
const FSTP_M64_BINARY80_INFINITY_OVERFLOW_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bq_fstp_m64fp_negative_infinity_overflow_directed_fnstsw.case.json"
);
const M64_STICKY_OE_EXACT_STORE_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31br_m64fp_sticky_oe_exact_store_fnstsw.case.json"
);
const FSTP_M32_NEGATIVE_INEXACT_SUBNORMAL_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bf_fstp_m32fp_negative_1_5_minsub_fnstsw.case.json"
);
const FST_M64_POSITIVE_MIN_SUBNORMAL: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31be_fst_m64fp_positive_min_subnormal.case.json"
);
const FSTP_M64_NEGATIVE_MIN_SUBNORMAL_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31be_fstp_m64fp_negative_min_subnormal_fnstsw.case.json"
);
const FST_M64_POSITIVE_INEXACT_SUBNORMAL: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bg_fst_m64fp_positive_1_5_minsub.case.json"
);
const FST_M64_EXTREME_UNDERFLOW_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bk_fst_m64fp_positive_0_75_minsub_fnstsw.case.json"
);
const FSTP_M64_NEGATIVE_INEXACT_SUBNORMAL_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bg_fstp_m64fp_negative_1_5_minsub_fnstsw.case.json"
);
const FST_M64_BINARY80_SUBNORMAL: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bm_fst_m64fp_positive_min_subnormal_fnstsw.case.json"
);
const FSTP_M64_BINARY80_SUBNORMAL: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31bm_fstp_m64fp_negative_signed_zero_fnstsw.case.json"
);
const FSTP_M64_NEGATIVE_SNAN_STICKY_IE_FNSTSW: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_31ay_fstp_m64fp_negative_snan_sticky_ie_fnstsw.case.json"
);
const FABS_NORMAL: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30z_fabs_normal.case.json");
const FABS_INVALID_SOURCE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30z_fabs_invalid_source.case.json");
const FTST_POSITIVE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30aa_ftst_positive.case.json");
const FTST_NEGATIVE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30aa_ftst_negative.case.json");
const FTST_INVALID_SOURCE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30aa_ftst_invalid_source.case.json");
const FCOM_ST1_GREATER: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ab_fcom_st1_greater.case.json");
const FCOM_ST1_EQUAL: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ab_fcom_st1_equal.case.json");
const FCOM_ST1_LESS: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ab_fcom_st1_less.case.json");
const FCOM_ST1_INVALID_RHS_TAG: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ab_fcom_st1_invalid_rhs_tag.case.json");
const FCOM_ST7_EQUAL: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ac_fcom_st7_equal.case.json");
const FCOM_ST7_INVALID_RHS_TAG: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ac_fcom_st7_invalid_rhs_tag.case.json");
const FCOMP_ST1_EQUAL: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ad_fcomp_st1_equal.case.json");
const FCOMP_STI_EQUAL: [&[u8]; 8] = [
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ae_fcomp_st0_equal.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ae_fcomp_st1_equal.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ae_fcomp_st2_equal.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ae_fcomp_st3_equal.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ae_fcomp_st4_equal.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ae_fcomp_st5_equal.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ae_fcomp_st6_equal.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ae_fcomp_st7_equal.case.json"),
];
const FCOMP_ST1_INVALID_RHS_TAG: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30ad_fcomp_st1_invalid_rhs_tag.case.json"
);
const FCOMPP_EQUAL_FNSTSW: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30af_fcompp_equal_fnstsw.case.json");
const FCOMPP_INVALID_RHS_TAG: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30af_fcompp_invalid_rhs_tag.case.json");
const FUCOMPP_EQUAL_FNSTSW: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ai_fucompp_equal_fnstsw.case.json");
const FUCOMPP_INVALID_RHS_TAG: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ai_fucompp_invalid_rhs_tag.case.json");
const FCOMI_EQUAL_FNSTSW: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30aj_fcomi_equal_fnstsw.case.json");
const FCOMI_INVALID_RHS_TAG: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30aj_fcomi_invalid_rhs_tag.case.json");
const FCOMIP_EQUAL_FNSTSW: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ak_fcomip_equal_fnstsw.case.json");
const FCOMIP_INVALID_RHS_TAG: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ak_fcomip_invalid_rhs_tag.case.json");
const FUCOMI_EQUAL_FNSTSW: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30al_fucomi_equal_fnstsw.case.json");
const FUCOMI_INVALID_RHS_TAG: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30al_fucomi_invalid_rhs_tag.case.json");
const FUCOMIP_EQUAL_FNSTSW: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30am_fucomip_equal_fnstsw.case.json");
const FUCOMIP_INVALID_RHS_TAG: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30am_fucomip_invalid_rhs_tag.case.json");
const S2_30AN_SELF_FCOMI: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30an_fcomi_st0_equal_fnstsw.case.json");
const S2_30AN_SELF_FUCOMIP: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30an_fucomip_st0_wrap_fnstsw.case.json");
const S2_30AN_FUCOMI_ST7: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30an_fucomi_st7_fnstsw.case.json");
const S2_30AN_FCOMIP_ST6: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30an_fcomip_st6_wrap_fnstsw.case.json");
const S2_30AN_INVALID_SELF: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30an_fcomi_st0_invalid_class.case.json");
const S2_30AN_INVALID_ST7: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30an_fucomip_st7_invalid_actual_tag.case.json"
);
const GS_BASE_QWORD_LOAD: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_22b_gs_base_qword_load_alias.case.json");
const FS_SELECTOR_TO_RAX: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_22c_fs_selector_to_rax.case.json");
const GS_SELECTOR_TO_RAX: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_22d_gs_selector_to_rax.case.json");
const VERW_GDT_CASES: &[(&str, &[u8], bool)] = &[
    (
        "s2_22e_verw_writable_dpl3",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22e_verw_writable_dpl3.case.json"),
        true,
    ),
    (
        "s2_22e_verw_readonly_dpl3",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22e_verw_readonly_dpl3.case.json"),
        false,
    ),
    (
        "s2_22e_verw_code_dpl3",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22e_verw_code_dpl3.case.json"),
        false,
    ),
    (
        "s2_22e_verw_system",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22e_verw_system.case.json"),
        false,
    ),
    (
        "s2_22e_verw_privilege",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22e_verw_privilege.case.json"),
        false,
    ),
    (
        "s2_22e_verw_table_limit",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22e_verw_table_limit.case.json"),
        false,
    ),
    (
        "s2_22e_verw_null",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22e_verw_null.case.json"),
        false,
    ),
    (
        "s2_22e_verw_rpl0",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22e_verw_rpl0.case.json"),
        true,
    ),
];
const FS_LOAD_CASES: &[(&str, &[u8], Option<(u8, u32)>, u8, u32, u8, u32, bool)] = &[
    (
        "s2_22f_fs_load_writable_g0",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22f_fs_load_writable_g0.case.json"),
        None,
        0xf3,
        0,
        0,
        0x89ab_cdef,
        false,
    ),
    (
        "s2_22f_fs_load_readonly_g1",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22f_fs_load_readonly_g1.case.json"),
        None,
        0xf1,
        0x0fff,
        8,
        0,
        false,
    ),
    (
        "s2_22f_fs_load_readable_code",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22f_fs_load_readable_code.case.json"),
        None,
        0xfb,
        0,
        0,
        0,
        false,
    ),
    (
        "s2_22f_fs_load_conforming_code",
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_22f_fs_load_conforming_code.case.json"
        ),
        None,
        0xff,
        0,
        0,
        0,
        false,
    ),
    (
        "s2_22f_fs_load_system_gp",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22f_fs_load_system_gp.case.json"),
        Some((13, 8)),
        0,
        0,
        0,
        0,
        false,
    ),
    (
        "s2_22f_fs_load_execute_only_gp",
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_22f_fs_load_execute_only_gp.case.json"
        ),
        Some((13, 8)),
        0,
        0,
        0,
        0,
        false,
    ),
    (
        "s2_22f_fs_load_privilege_gp",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22f_fs_load_privilege_gp.case.json"),
        Some((13, 8)),
        0,
        0,
        0,
        0,
        false,
    ),
    (
        "s2_22f_fs_load_not_present_np",
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_22f_fs_load_not_present_np.case.json"
        ),
        Some((11, 8)),
        0,
        0,
        0,
        0,
        false,
    ),
    (
        "s2_22f_fs_load_table_limit_gp",
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_22f_fs_load_table_limit_gp.case.json"
        ),
        Some((13, 8)),
        0,
        0,
        0,
        0,
        false,
    ),
    (
        "s2_22g_fs_load_a0_writable",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22g_fs_load_a0_writable.case.json"),
        None,
        0xf3,
        0,
        0,
        0x89ab_cdef,
        true,
    ),
    (
        "s2_22g_fs_load_a0_readonly",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22g_fs_load_a0_readonly.case.json"),
        None,
        0xf1,
        0,
        0,
        0,
        true,
    ),
];
const GS_LOAD_CASES: &[(&str, &[u8], Option<(u8, u32)>, u8, u32, u8, u32, bool)] = &[
    (
        "s2_22h_gs_load_writable_g0",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22h_gs_load_writable_g0.case.json"),
        None,
        0xf3,
        0,
        0,
        0x89ab_cdef,
        false,
    ),
    (
        "s2_22h_gs_load_readonly_g1",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22h_gs_load_readonly_g1.case.json"),
        None,
        0xf1,
        0x0fff,
        8,
        0,
        false,
    ),
    (
        "s2_22h_gs_load_readable_code",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22h_gs_load_readable_code.case.json"),
        None,
        0xfb,
        0,
        0,
        0,
        false,
    ),
    (
        "s2_22h_gs_load_conforming_code",
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_22h_gs_load_conforming_code.case.json"
        ),
        None,
        0x9f,
        0,
        0,
        0,
        false,
    ),
    (
        "s2_22h_gs_load_system_gp",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22h_gs_load_system_gp.case.json"),
        Some((13, 8)),
        0,
        0,
        0,
        0,
        false,
    ),
    (
        "s2_22h_gs_load_execute_only_gp",
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_22h_gs_load_execute_only_gp.case.json"
        ),
        Some((13, 8)),
        0,
        0,
        0,
        0,
        false,
    ),
    (
        "s2_22h_gs_load_privilege_gp",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22h_gs_load_privilege_gp.case.json"),
        Some((13, 8)),
        0,
        0,
        0,
        0,
        false,
    ),
    (
        "s2_22h_gs_load_not_present_np",
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_22h_gs_load_not_present_np.case.json"
        ),
        Some((11, 8)),
        0,
        0,
        0,
        0,
        false,
    ),
    (
        "s2_22h_gs_load_table_limit_gp",
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_22h_gs_load_table_limit_gp.case.json"
        ),
        Some((13, 8)),
        0,
        0,
        0,
        0,
        false,
    ),
    (
        "s2_22h_gs_load_a0_writable",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22h_gs_load_a0_writable.case.json"),
        None,
        0xf3,
        0,
        0,
        0x89ab_cdef,
        true,
    ),
    (
        "s2_22h_gs_load_a0_readonly",
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22h_gs_load_a0_readonly.case.json"),
        None,
        0xf1,
        0,
        0,
        0,
        true,
    ),
];
const CMPXCHG_QWORD_REGISTER_EQUAL: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_21c_cmpxchg_qword_register_equal.case.json"
);
const CMPXCHG_QWORD_REGISTER_UNEQUAL_ALIAS: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_21c_cmpxchg_qword_register_unequal_alias.case.json"
);
const CMPXCHG_QWORD_MEMORY_EQUAL: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_21e_cmpxchg_qword_memory_equal.case.json"
);
const CMPXCHG_QWORD_MEMORY_UNEQUAL_ALIAS: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_21e_cmpxchg_qword_memory_unequal_alias.case.json"
);

fn cli(bytes: &[u8], engine: &str) -> Output {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "od-runner-v2-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).unwrap();
    let path = dir.join("case.json");
    fs::write(&path, bytes).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .arg("--fixture")
        .arg(&path)
        .args(["--engine", engine, "--json"])
        .output()
        .unwrap();
    fs::remove_dir_all(dir).unwrap();
    output
}

fn compare_cli(case: &[u8], result_bytes: &[u8]) -> Output {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "od-runner-v2-compare-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).unwrap();
    let case_path = dir.join("case.json");
    let left_path = dir.join("left.json");
    let right_path = dir.join("right.json");
    fs::write(&case_path, case).unwrap();
    fs::write(&left_path, result_bytes).unwrap();
    fs::write(&right_path, result_bytes).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .arg("--compare-results")
        .arg(&left_path)
        .arg(&right_path)
        .arg("--case")
        .arg(&case_path)
        .arg("--json")
        .output()
        .unwrap();
    fs::remove_dir_all(dir).unwrap();
    output
}

fn run(bytes: &[u8], engine: &str, exit: i32) -> (LoadedFixture, RunResult) {
    let loaded = Fixture::from_json(bytes).unwrap();
    let output = cli(bytes, engine);
    assert_eq!(
        output.status.code(),
        Some(exit),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result = RunResult::from_json(&output.stdout).unwrap();
    result.validate_for(&loaded).unwrap();
    (loaded, result)
}

fn snapshot(result: &RunResult) -> &Snapshot {
    match &result.snapshot {
        Observation::Available { value } => value,
        _ => panic!("CPU execution must capture its final state"),
    }
}

fn count(result: &RunResult) -> u64 {
    match result.instructions_executed {
        Observation::Available { value } => value.get(),
        _ => panic!("interpreter count must be observed"),
    }
}

#[test]
fn operand_size_prefixed_near_ret_keeps_qword_stack_semantics() {
    for (bytes, expected_code, expected_rip) in [
        (RET66, "66c3", 0x0010_0002),
        (RET66_IGNORED_REX, "4866c3", 0x0010_0003),
    ] {
        let (input, result) = run(bytes, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 1);
        let observed = snapshot(&result);
        let mut expected_gpr = input.fixture.initial_state.gpr.clone();
        expected_gpr[4] = HexU64(0x0030_0088);
        assert_eq!(observed.state.gpr, expected_gpr);
        assert_eq!(observed.state.rip, HexU64(expected_rip));
        assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);
        assert_eq!(observed.rflags_defined, HexU64(0xcd7));
        assert_eq!(observed.memory.len(), 2);
        assert_eq!(observed.memory[0].bytes_hex, expected_code);
        assert_eq!(
            observed.memory[1].bytes_hex,
            input.fixture.memory[1].initial_bytes_hex
        );
    }
}

#[test]
fn qword_memory_xchg_fixture_matches_literal_alias_and_split_memory_result() {
    let (input, result) = run(XCHG_QWORD_MEMORY_ALIAS, "interpreter", 0);
    assert_eq!(input.fixture.case_id, "s2_21a_xchg_qword_memory_alias");
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 1);

    let observed = snapshot(&result);
    let mut expected_gpr = input.fixture.initial_state.gpr;
    expected_gpr[0] = HexU64(0x1122_3344_5566_7788);
    assert_eq!(observed.state.gpr, expected_gpr);
    assert_eq!(observed.state.rip, HexU64(0x0010_0003));
    assert_eq!(observed.state.rflags, HexU64(0x0cd7));
    assert_eq!(observed.rflags_defined, HexU64(0x0cd7));
    assert_eq!(observed.memory.len(), 3);
    assert_eq!(observed.memory[0].region_id, "code");
    assert_eq!(observed.memory[0].bytes_hex, "488700");
    assert_eq!(observed.memory[1].region_id, "data_left");
    assert_eq!(observed.memory[1].bytes_hex, "a5a5a5a5a5a5fe0f");
    assert_eq!(observed.memory[2].region_id, "data_right");
    assert_eq!(observed.memory[2].bytes_hex, "200000000000a5a5");
    assert_eq!(observed.exception, Observation::Available { value: None });
}

#[test]
fn qword_register_xadd_fixture_matches_alias_sum_and_arithmetic_flags() {
    let (input, result) = run(XADD_QWORD_REGISTER_ALIAS, "interpreter", 0);
    assert_eq!(input.fixture.case_id, "s2_21b_xadd_qword_register_alias");
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 1);

    let observed = snapshot(&result);
    let mut expected_gpr = input.fixture.initial_state.gpr;
    expected_gpr[0] = HexU64(0xffff_ffff_ffff_fffe);
    assert_eq!(observed.state.gpr, expected_gpr);
    assert_eq!(observed.state.rip, HexU64(0x0010_0004));
    assert_eq!(observed.state.rflags, HexU64(0x0c92));
    assert_eq!(observed.rflags_defined, HexU64(0x0cd7));
    assert_eq!(observed.memory.len(), 1);
    assert_eq!(observed.memory[0].region_id, "code");
    assert_eq!(observed.memory[0].bytes_hex, "480fc1c0");
    assert_eq!(observed.exception, Observation::Available { value: None });
}

#[test]
fn qword_memory_xadd_fixture_matches_alias_sum_flags_and_split_canaries() {
    let (input, result) = run(XADD_QWORD_MEMORY_ALIAS_SPLIT, "interpreter", 0);
    assert_eq!(
        input.fixture.case_id,
        "s2_21d_xadd_qword_memory_alias_split"
    );
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 1);

    let observed = snapshot(&result);
    let mut expected_gpr = input.fixture.initial_state.gpr;
    expected_gpr[0] = HexU64(u64::MAX);
    assert_eq!(observed.state.gpr, expected_gpr);
    assert_eq!(observed.state.rip, HexU64(0x0010_0004));
    assert_eq!(observed.state.rflags, HexU64(0x0417));
    assert_eq!(observed.rflags_defined, HexU64(0x0cd7));
    assert_eq!(observed.memory.len(), 3);
    assert_eq!(observed.memory[0].region_id, "code");
    assert_eq!(observed.memory[0].bytes_hex, "480fc100");
    assert_eq!(observed.memory[1].region_id, "data_left");
    assert_eq!(observed.memory[1].bytes_hex, "a5a5a5fa00300000");
    assert_eq!(observed.memory[2].region_id, "data_right");
    assert_eq!(observed.memory[2].bytes_hex, "000000a5a5a5a5a5");
    assert_eq!(observed.exception, Observation::Available { value: None });
}

#[test]
fn locked_qword_memory_xadd_fixture_matches_alias_sum_flags_and_split_canaries() {
    let (input, result) = run(LOCK_XADD_QWORD_MEMORY_ALIAS_SPLIT, "interpreter", 0);
    assert_eq!(
        input.fixture.case_id,
        "s2_21f_lock_xadd_qword_memory_alias_split"
    );
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 1);

    let observed = snapshot(&result);
    let mut expected_gpr = input.fixture.initial_state.gpr;
    expected_gpr[0] = HexU64(u64::MAX);
    assert_eq!(observed.state.gpr, expected_gpr);
    assert_eq!(observed.state.rip, HexU64(0x0010_0005));
    assert_eq!(observed.state.rflags, HexU64(0x0417));
    assert_eq!(observed.rflags_defined, HexU64(0x0cd7));
    assert_eq!(observed.memory.len(), 3);
    assert_eq!(observed.memory[0].region_id, "code");
    assert_eq!(observed.memory[0].bytes_hex, "f0480fc100");
    assert_eq!(observed.memory[1].region_id, "data_left");
    assert_eq!(observed.memory[1].bytes_hex, "a5a5a5fa00300000");
    assert_eq!(observed.memory[2].region_id, "data_right");
    assert_eq!(observed.memory[2].bytes_hex, "000000a5a5a5a5a5");
    assert_eq!(observed.exception, Observation::Available { value: None });
}

#[test]
fn mfence_store_fence_load_fixture_preserves_state_and_observes_completed_store() {
    let (input, result) = run(MFENCE_STORE_FENCE_LOAD, "interpreter", 0);
    assert_eq!(input.fixture.case_id, "s2_21g_mfence_store_fence_load");
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 3);

    let observed = snapshot(&result);
    let mut expected_gpr = input.fixture.initial_state.gpr;
    expected_gpr[1] = expected_gpr[3];
    assert_eq!(observed.state.gpr, expected_gpr);
    assert_eq!(observed.state.rip, HexU64(0x0010_0009));
    assert_eq!(observed.state.rflags, HexU64(0x0cd7));
    assert_eq!(observed.rflags_defined, HexU64(0x0cd7));
    assert_eq!(observed.memory.len(), 2);
    assert_eq!(observed.memory[0].region_id, "code");
    assert_eq!(observed.memory[0].bytes_hex, "4889180faef0488b08");
    assert_eq!(observed.memory[1].region_id, "data");
    assert_eq!(observed.memory[1].bytes_hex, "8877665544332211");
    assert_eq!(observed.exception, Observation::Available { value: None });
}

#[test]
fn gs_base_qword_load_fixture_uses_base_and_rax_alias() {
    let (input, result) = run(GS_BASE_QWORD_LOAD, "interpreter", 0);
    assert_eq!(input.fixture.case_id, "s2_22b_gs_base_qword_load_alias");
    assert_eq!(input.fixture.profile, od_testkit::v2::Profile::LongGsBaseV1);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 1);

    let observed = snapshot(&result);
    assert_eq!(observed.state.gpr[0], HexU64(0x8877_6655_4433_2211));
    assert_eq!(observed.state.gs_base, Some(HexU64(0xffff_8000_0000_0000)));
    assert_eq!(observed.state.rip, HexU64(0x0010_0004));
    assert_eq!(observed.memory[0].bytes_hex, "65488b00");
    assert_eq!(observed.exception, Observation::Available { value: None });
}

#[test]
fn fs_selector_fixture_zero_extends_and_preserves_segment_state() {
    let (input, result) = run(FS_SELECTOR_TO_RAX, "interpreter", 0);
    assert_eq!(input.fixture.case_id, "s2_22c_fs_selector_to_rax");
    assert_eq!(
        input.fixture.profile,
        od_testkit::v2::Profile::LongFsSelectorV1
    );
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 1);

    let observed = snapshot(&result);
    assert_eq!(observed.state.gpr[0], HexU64(0xf123));
    assert_eq!(observed.state.fs_selector, Some(HexU64(0xf123)));
    assert_eq!(observed.state.fs_base, Some(HexU64(0xffff_8000_0000_1000)));
    assert_eq!(observed.state.rip, HexU64(0x0010_0003));
    assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);
    assert_eq!(observed.memory[0].bytes_hex, "488ce0");
    assert_eq!(observed.exception, Observation::Available { value: None });
}

#[test]
fn gs_selector_fixture_zero_extends_and_preserves_segment_state() {
    let (input, result) = run(GS_SELECTOR_TO_RAX, "interpreter", 0);
    assert_eq!(input.fixture.case_id, "s2_22d_gs_selector_to_rax");
    assert_eq!(
        input.fixture.profile,
        od_testkit::v2::Profile::LongGsSelectorV1
    );
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 1);

    let observed = snapshot(&result);
    assert_eq!(observed.state.gpr[0], HexU64(0xf123));
    let mut expected_gpr = input.fixture.initial_state.gpr;
    expected_gpr[0] = HexU64(0xf123);
    assert_eq!(observed.state.gpr, expected_gpr);
    assert_eq!(observed.state.gs_selector, Some(HexU64(0xf123)));
    assert_eq!(observed.state.gs_base, Some(HexU64(0xffff_8000_0000_1000)));
    assert_eq!(observed.state.rip, HexU64(0x0010_0003));
    assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);
    assert_eq!(observed.memory[0].bytes_hex, "488ce8");
    assert_eq!(observed.exception, Observation::Available { value: None });
}

#[test]
fn verw_gdt_fixtures_check_literal_rights_flags_state_and_descriptor_bytes() {
    for (case_id, bytes, writable) in VERW_GDT_CASES {
        let (input, result) = run(bytes, "interpreter", 0);
        assert_eq!(input.fixture.case_id, *case_id);
        assert_eq!(
            input.fixture.profile,
            od_testkit::v2::Profile::LongVerwGdtV1
        );
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 1);

        let observed = snapshot(&result);
        assert_eq!(observed.state.gpr, input.fixture.initial_state.gpr);
        assert_eq!(observed.state.rip, HexU64(0x0010_0003));
        let expected_flags = if *writable { 0x0cd7 } else { 0x0c97 };
        assert_eq!(observed.state.rflags, HexU64(expected_flags));
        assert_eq!(
            observed.state.gdtr_base,
            input.fixture.initial_state.gdtr_base
        );
        assert_eq!(
            observed.state.gdtr_limit,
            input.fixture.initial_state.gdtr_limit
        );
        assert_eq!(observed.memory.len(), 2);
        assert_eq!(observed.memory[0].bytes_hex, "0f00e8");
        assert_eq!(
            observed.memory[1].bytes_hex,
            input.fixture.memory[1].initial_bytes_hex
        );
        assert_eq!(observed.exception, Observation::Available { value: None });
    }
}

#[test]
fn fs_load_fixtures_check_literal_descriptor_results_fault_codes_and_full_state() {
    for (
        case_id,
        bytes,
        fault,
        expected_access,
        expected_limit,
        expected_flags,
        expected_base,
        accessed_update,
    ) in FS_LOAD_CASES
    {
        let (input, result) = run(bytes, "interpreter", 0);
        assert_eq!(input.fixture.case_id, *case_id);
        assert_eq!(input.fixture.profile, od_testkit::v2::Profile::LongFsLoadV1);
        let observed = snapshot(&result);
        assert_eq!(
            observed.state.gdtr_base,
            input.fixture.initial_state.gdtr_base
        );
        assert_eq!(
            observed.state.gdtr_limit,
            input.fixture.initial_state.gdtr_limit
        );
        assert_eq!(observed.memory.len(), 2);
        assert_eq!(observed.memory[0].bytes_hex, "8ee0");
        let mut expected_gdt = input.fixture.memory[1].initial_bytes_hex.clone();
        if *accessed_update {
            expected_gdt.replace_range(26..28, &format!("{expected_access:02x}"));
        }
        assert_eq!(observed.memory[1].bytes_hex, expected_gdt);
        assert_eq!(
            observed.exception,
            match fault {
                Some((vector, code)) => Observation::Available {
                    value: Some(od_testkit::result::ExceptionSnapshot {
                        vector: *vector,
                        error_code: Observation::Available {
                            value: HexU64(u64::from(*code))
                        },
                        faulting_rip: HexU64(0x0010_0000),
                        fault_address: Observation::Unavailable {
                            reason: "exception has no reported linear address".into(),
                        },
                    })
                },
                None => Observation::Available { value: None },
            }
        );
        let expected_exit = if fault.is_some() { 0 } else { 1 };
        assert_eq!(count(&result), expected_exit);

        let expected_gpr = input.fixture.initial_state.gpr;
        if fault.is_none() {
            assert!(matches!(result.outcome, Outcome::StopAddress {}));
            assert_eq!(observed.state.rip, HexU64(0x0010_0002));
            assert_eq!(
                observed.state.fs_base,
                Some(HexU64(u64::from(*expected_base)))
            );
            assert_eq!(
                observed.state.fs_selector,
                Some(HexU64(expected_gpr[0].get() & 0xffff))
            );
            assert_eq!(
                observed.state.fs_cache_access,
                Some(HexU64(u64::from(*expected_access)))
            );
            assert_eq!(
                observed.state.fs_cache_limit,
                Some(HexU64(u64::from(*expected_limit)))
            );
            assert_eq!(
                observed.state.fs_cache_flags,
                Some(HexU64(u64::from(*expected_flags)))
            );
        } else {
            assert!(matches!(result.outcome, Outcome::Exception {}));
            assert_eq!(observed.state.rip, HexU64(0x0010_0000));
            assert_eq!(observed.state.fs_base, input.fixture.initial_state.fs_base);
            assert_eq!(
                observed.state.fs_selector,
                input.fixture.initial_state.fs_selector
            );
            assert_eq!(
                observed.state.fs_cache_limit,
                input.fixture.initial_state.fs_cache_limit
            );
            assert_eq!(
                observed.state.fs_cache_access,
                input.fixture.initial_state.fs_cache_access
            );
            assert_eq!(
                observed.state.fs_cache_flags,
                input.fixture.initial_state.fs_cache_flags
            );
        }
        assert_eq!(
            observed.state.gpr, expected_gpr,
            "all GPRs remain unchanged: {case_id}"
        );
        assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);
    }
}

#[test]
fn gs_load_fixtures_check_literal_descriptor_results_fault_codes_and_full_state() {
    for (
        case_id,
        bytes,
        fault,
        expected_access,
        expected_limit,
        expected_flags,
        expected_base,
        accessed_update,
    ) in GS_LOAD_CASES
    {
        let (input, result) = run(bytes, "interpreter", 0);
        assert_eq!(input.fixture.case_id, *case_id);
        assert_eq!(input.fixture.profile, od_testkit::v2::Profile::LongGsLoadV1);
        let observed = snapshot(&result);
        assert_eq!(
            observed.state.gdtr_base,
            input.fixture.initial_state.gdtr_base
        );
        assert_eq!(
            observed.state.gdtr_limit,
            input.fixture.initial_state.gdtr_limit
        );
        assert_eq!(observed.memory.len(), 2);
        assert_eq!(observed.memory[0].bytes_hex, "8ee8");
        let mut expected_gdt = input.fixture.memory[1].initial_bytes_hex.clone();
        if *accessed_update {
            expected_gdt.replace_range(26..28, &format!("{expected_access:02x}"));
        }
        assert_eq!(observed.memory[1].bytes_hex, expected_gdt);
        assert_eq!(
            observed.exception,
            match fault {
                Some((vector, code)) => Observation::Available {
                    value: Some(od_testkit::result::ExceptionSnapshot {
                        vector: *vector,
                        error_code: Observation::Available {
                            value: HexU64(u64::from(*code))
                        },
                        faulting_rip: HexU64(0x0010_0000),
                        fault_address: Observation::Unavailable {
                            reason: "exception has no reported linear address".into(),
                        },
                    })
                },
                None => Observation::Available { value: None },
            }
        );
        let expected_exit = if fault.is_some() { 0 } else { 1 };
        assert_eq!(count(&result), expected_exit);

        let expected_gpr = input.fixture.initial_state.gpr;
        if fault.is_none() {
            assert!(matches!(result.outcome, Outcome::StopAddress {}));
            assert_eq!(observed.state.rip, HexU64(0x0010_0002));
            assert_eq!(
                observed.state.gs_base,
                Some(HexU64(u64::from(*expected_base)))
            );
            assert_eq!(
                observed.state.gs_selector,
                Some(HexU64(expected_gpr[0].get() & 0xffff))
            );
            assert_eq!(
                observed.state.gs_cache_access,
                Some(HexU64(u64::from(*expected_access)))
            );
            assert_eq!(
                observed.state.gs_cache_limit,
                Some(HexU64(u64::from(*expected_limit)))
            );
            assert_eq!(
                observed.state.gs_cache_flags,
                Some(HexU64(u64::from(*expected_flags)))
            );
        } else {
            assert!(matches!(result.outcome, Outcome::Exception {}));
            assert_eq!(observed.state.rip, HexU64(0x0010_0000));
            assert_eq!(observed.state.gs_base, input.fixture.initial_state.gs_base);
            assert_eq!(
                observed.state.gs_selector,
                input.fixture.initial_state.gs_selector
            );
            assert_eq!(
                observed.state.gs_cache_limit,
                input.fixture.initial_state.gs_cache_limit
            );
            assert_eq!(
                observed.state.gs_cache_access,
                input.fixture.initial_state.gs_cache_access
            );
            assert_eq!(
                observed.state.gs_cache_flags,
                input.fixture.initial_state.gs_cache_flags
            );
        }
        assert_eq!(
            observed.state.gpr, expected_gpr,
            "all GPRs remain unchanged: {case_id}"
        );
        assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);
    }
}

#[test]
fn s2_22i_gs_base_composition_runs_the_literal_two_instruction_profile() {
    let bytes =
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22i_gs_base_composition.case.json");
    let (input, result) = run(bytes, "interpreter", 0);
    assert_eq!(input.fixture.case_id, "s2_22i_gs_base_composition");
    assert_eq!(input.fixture.profile, od_testkit::v2::Profile::LongGsLoadV1);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 2);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, HexU64(0x0010_0006));
    assert_eq!(observed.state.gpr[0], HexU64(0x1122_3344_5566_7788));
    assert_eq!(observed.state.gs_base, Some(HexU64(0x8000_0000)));
    assert_eq!(observed.state.gs_selector, Some(HexU64(0x0b)));
    assert_eq!(observed.state.gs_cache_limit, Some(HexU64(0)));
    assert_eq!(observed.state.gs_cache_access, Some(HexU64(0xf3)));
    assert_eq!(observed.state.gs_cache_flags, Some(HexU64(0)));
    assert_eq!(
        observed.memory[1].bytes_hex,
        "00000000000000000000000000f30080"
    );
    assert_eq!(
        observed.memory[2].bytes_hex,
        "0000000000000000000000deadc0decafef00d"
    );
    assert_eq!(
        observed.memory[3].bytes_hex,
        "00000000000000000000008877665544332211"
    );

    let mut unsupported = input.fixture.clone();
    unsupported.memory[0]
        .initial_bytes_hex
        .replace_range(10..12, "01");
    assert!(unsupported.validate().is_err());
    let mut widened_budget = input.fixture;
    widened_budget.instruction_budget = 3;
    assert!(widened_budget.validate().is_err());
}

#[test]
fn s2_22j_fs_base_composition_runs_the_literal_two_instruction_profile() {
    let bytes =
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22j_fs_base_composition.case.json");
    let (input, result) = run(bytes, "interpreter", 0);
    assert_eq!(input.fixture.case_id, "s2_22j_fs_base_composition");
    assert_eq!(input.fixture.profile, od_testkit::v2::Profile::LongFsLoadV1);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 2);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, HexU64(0x0010_0006));
    assert_eq!(observed.state.gpr[0], HexU64(0x1122_3344_5566_7788));
    assert_eq!(observed.state.fs_base, Some(HexU64(0x8000_0000)));
    assert_eq!(observed.state.fs_selector, Some(HexU64(0x0b)));
    assert_eq!(observed.state.fs_cache_limit, Some(HexU64(0)));
    assert_eq!(observed.state.fs_cache_access, Some(HexU64(0xf3)));
    assert_eq!(observed.state.fs_cache_flags, Some(HexU64(0)));
    assert_eq!(
        observed.memory[1].bytes_hex,
        "00000000000000000000000000f30080"
    );
    assert_eq!(
        observed.memory[2].bytes_hex,
        "0000000000000000000000deadc0decafef00d"
    );
    assert_eq!(
        observed.memory[3].bytes_hex,
        "00000000000000000000008877665544332211"
    );

    let mut unsupported = input.fixture.clone();
    unsupported.memory[0]
        .initial_bytes_hex
        .replace_range(10..12, "01");
    assert!(unsupported.validate().is_err());
    let mut widened_budget = input.fixture;
    widened_budget.instruction_budget = 3;
    assert!(widened_budget.validate().is_err());
}

#[test]
fn local_x87_state_profile_preserves_all_raw_state_across_one_nop() {
    let mut fixture = Fixture::from_json(MFENCE_STORE_FENCE_LOAD).unwrap().fixture;
    fixture.case_id = "local_x87_state_nop".into();
    fixture.profile = od_testkit::v2::Profile::LongX87StateV1;
    fixture.initial_state.rip = HexU64(0x0010_0000);
    let seed = od_isa::state::X87State::fixture_seed();
    fixture.initial_state.x87 = Some(od_testkit::v2::X87Snapshot::from(&seed));
    let code = fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.guest_address = HexU64(0x0010_0000);
    code.length = 1;
    code.initial_bytes_hex = "90".into();
    code.comparison_mask_hex = "ff".into();
    fixture.instruction_budget = 1;
    fixture.stop_rip = Some(HexU64(0x0010_0001));
    fixture.expected_exit = ExpectedExit::StopAddress;

    let bytes = fixture.to_json().unwrap();
    let (input, result) = run(&bytes, "interpreter", 0);
    assert_eq!(count(&result), 1);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, HexU64(0x0010_0001));
    assert_eq!(observed.state.x87, input.fixture.initial_state.x87);
    assert_eq!(
        observed.state.x87.as_ref().unwrap().registers[6],
        "deadbeefcafebabe0180"
    );
}

#[test]
fn local_x87_state_profile_runs_fninit_with_full_environment_reset() {
    let mut fixture = Fixture::from_json(MFENCE_STORE_FENCE_LOAD).unwrap().fixture;
    fixture.case_id = "local_x87_fninit".into();
    fixture.profile = od_testkit::v2::Profile::LongX87StateV1;
    fixture.initial_state.rip = HexU64(0x0010_0000);
    let mut seed = od_isa::state::X87State::fixture_seed();
    seed.registers = std::array::from_fn(|register| {
        std::array::from_fn(|byte| (register as u8).wrapping_mul(31).wrapping_add(byte as u8))
    });
    fixture.initial_state.x87 = Some(od_testkit::v2::X87Snapshot::from(&seed));
    let code = fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.guest_address = HexU64(0x0010_0000);
    code.length = 2;
    code.initial_bytes_hex = "dbe3".into();
    code.comparison_mask_hex = "ffff".into();
    fixture.instruction_budget = 1;
    fixture.stop_rip = Some(HexU64(0x0010_0002));
    fixture.expected_exit = ExpectedExit::StopAddress;

    let (input, result) = run(&fixture.to_json().unwrap(), "interpreter", 0);
    assert_eq!(count(&result), 1);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, HexU64(0x0010_0002));
    assert_eq!(observed.state.gpr, input.fixture.initial_state.gpr);
    assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);
    let expected = od_testkit::v2::X87Snapshot {
        registers: seed
            .registers
            .map(|register| register.iter().map(|byte| format!("{byte:02x}")).collect()),
        control_word: HexU64(0x037f),
        status_word: HexU64(0),
        tag_word: HexU64(0xffff),
        instruction_pointer: HexU64(0),
        data_pointer: HexU64(0),
        last_opcode: HexU64(0),
    };
    assert_eq!(observed.state.x87.as_ref(), Some(&expected));
}

#[test]
fn local_x87_state_profile_runs_fnstsw_ax_from_raw_status_word() {
    let mut fixture = Fixture::from_json(MFENCE_STORE_FENCE_LOAD).unwrap().fixture;
    fixture.case_id = "local_x87_fnstsw_ax".into();
    fixture.profile = od_testkit::v2::Profile::LongX87StateV1;
    fixture.initial_state.rip = HexU64(0x0010_0000);
    fixture.initial_state.gpr[0] = HexU64(0xfeed_cafe_89ab_7654);
    let mut seed = od_isa::state::X87State::fixture_seed();
    seed.status_word = 0xa5c3;
    fixture.initial_state.x87 = Some(od_testkit::v2::X87Snapshot::from(&seed));
    let code = fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.guest_address = HexU64(0x0010_0000);
    code.length = 2;
    code.initial_bytes_hex = "dfe0".into();
    code.comparison_mask_hex = "ffff".into();
    fixture.instruction_budget = 1;
    fixture.stop_rip = Some(HexU64(0x0010_0002));
    fixture.expected_exit = ExpectedExit::StopAddress;

    let (input, result) = run(&fixture.to_json().unwrap(), "interpreter", 0);
    assert_eq!(count(&result), 1);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, HexU64(0x0010_0002));
    assert_eq!(observed.state.gpr[0].get(), 0xfeed_cafe_89ab_a5c3);
    for index in 1..observed.state.gpr.len() {
        assert_eq!(
            observed.state.gpr[index],
            input.fixture.initial_state.gpr[index]
        );
    }
    assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);
    // The local model leaves C0/C1/C2/C3 deterministic; Intel defines them as
    // undefined after FNSTSW, so this is not an architectural preservation claim.
    assert_eq!(observed.state.x87, input.fixture.initial_state.x87);
}

#[test]
fn local_x87_state_profile_runs_fnclex_and_preserves_unaffected_fields() {
    let mut fixture = Fixture::from_json(MFENCE_STORE_FENCE_LOAD).unwrap().fixture;
    fixture.case_id = "local_x87_fnclex".into();
    fixture.profile = od_testkit::v2::Profile::LongX87StateV1;
    fixture.initial_state.rip = HexU64(0x0010_0000);
    let mut seed = od_isa::state::X87State::fixture_seed();
    seed.control_word = 0x0072;
    seed.status_word = 0xa5c3;
    seed.tag_word = 0x1234;
    seed.instruction_pointer = 0x1234_5678_9abc_def0;
    seed.data_pointer = 0xfedc_ba98_7654_3210;
    seed.last_opcode = 0x0567;
    fixture.initial_state.x87 = Some(od_testkit::v2::X87Snapshot::from(&seed));
    let code = fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.guest_address = HexU64(0x0010_0000);
    code.length = 2;
    code.initial_bytes_hex = "dbe2".into();
    code.comparison_mask_hex = "ffff".into();
    fixture.instruction_budget = 1;
    fixture.stop_rip = Some(HexU64(0x0010_0002));
    fixture.expected_exit = ExpectedExit::StopAddress;

    let (input, result) = run(&fixture.to_json().unwrap(), "interpreter", 0);
    assert_eq!(count(&result), 1);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, HexU64(0x0010_0002));
    assert_eq!(observed.state.gpr, input.fixture.initial_state.gpr);
    assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);
    let mut expected = seed;
    expected.status_word = 0x2500;
    assert_eq!(
        observed.state.x87.as_ref(),
        Some(&od_testkit::v2::X87Snapshot::from(&expected))
    );
}

#[test]
fn local_x87_fstp_invalid_source_fixture_is_atomic_in_the_cli_runner() {
    let (input, result) = run(FSTP_INVALID_SOURCE, "interpreter", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, input.fixture.initial_state.rip);
    assert_eq!(observed.state.gpr, input.fixture.initial_state.gpr);
    assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);
    assert_eq!(observed.state.x87, input.fixture.initial_state.x87);
}

#[test]
fn local_x87_fstp_fixture_runner_admits_every_destination_index() {
    for destination in 0u8..=7 {
        let mut fixture = Fixture::from_json(FSTP_INVALID_SOURCE).unwrap().fixture;
        fixture.case_id = format!("local_x87_fstp_i{destination}");
        fixture.stop_rip = Some(HexU64(0x0010_0002));
        fixture.expected_exit = ExpectedExit::StopAddress;
        let x87 = fixture.initial_state.x87.as_mut().unwrap();
        let top = ((x87.status_word.get() >> 11) & 7) as u8;
        let top_shift = u32::from(top) * 2;
        x87.tag_word = HexU64(x87.tag_word.get() & !(3 << top_shift));
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("dd{:02x}", 0xd8 | destination);

        let (input, result) = run(&fixture.to_json().unwrap(), "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 1);
        let observed = snapshot(&result);
        assert_eq!(observed.state.rip, HexU64(0x0010_0002));
        assert_eq!(observed.state.gpr, input.fixture.initial_state.gpr);
        assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);

        let initial = input.fixture.initial_state.x87.as_ref().unwrap();
        let mut expected = initial.clone();
        let top = ((initial.status_word.get() >> 11) & 7) as u8;
        let physical_destination = (top + destination) & 7;
        expected.registers[usize::from(physical_destination)] =
            initial.registers[usize::from(top)].clone();
        let dest_shift = u32::from(physical_destination) * 2;
        expected.tag_word =
            HexU64((expected.tag_word.get() & !(3 << dest_shift)) | (0 << dest_shift));
        let top_shift = u32::from(top) * 2;
        expected.tag_word =
            HexU64((expected.tag_word.get() & !(3 << top_shift)) | (3 << top_shift));
        let new_top = top.wrapping_add(1) & 7;
        expected.status_word =
            HexU64((initial.status_word.get() & !(7 << 11)) | (u64::from(new_top) << 11));
        expected.instruction_pointer = input.fixture.initial_state.rip;
        assert_eq!(observed.state.x87.as_ref(), Some(&expected));
    }
}

#[test]
fn local_x87_fst_invalid_source_fixture_is_atomic_in_the_cli_runner() {
    let (input, result) = run(FST_INVALID_SOURCE, "interpreter", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, input.fixture.initial_state.rip);
    assert_eq!(observed.state.gpr, input.fixture.initial_state.gpr);
    assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);
    assert_eq!(observed.state.x87, input.fixture.initial_state.x87);
}

#[test]
fn local_x87_fst_fixture_runner_admits_every_destination_index() {
    for destination in 0u8..=7 {
        let mut fixture = Fixture::from_json(FST_INVALID_SOURCE).unwrap().fixture;
        fixture.case_id = format!("local_x87_fst_i{destination}");
        fixture.stop_rip = Some(HexU64(0x0010_0002));
        fixture.expected_exit = ExpectedExit::StopAddress;
        let x87 = fixture.initial_state.x87.as_mut().unwrap();
        let top = ((x87.status_word.get() >> 11) & 7) as u8;
        let top_shift = u32::from(top) * 2;
        x87.tag_word = HexU64((x87.tag_word.get() & !(3 << top_shift)) | (0 << top_shift));
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("dd{:02x}", 0xd0 | destination);

        let (input, result) = run(&fixture.to_json().unwrap(), "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 1);
        let observed = snapshot(&result);
        assert_eq!(observed.state.rip, HexU64(0x0010_0002));
        assert_eq!(observed.state.gpr, input.fixture.initial_state.gpr);
        assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);

        let initial = input.fixture.initial_state.x87.as_ref().unwrap();
        let mut expected = initial.clone();
        let top = ((initial.status_word.get() >> 11) & 7) as u8;
        let physical_destination = (top + destination) & 7;
        expected.registers[usize::from(physical_destination)] =
            initial.registers[usize::from(top)].clone();
        let dest_shift = u32::from(physical_destination) * 2;
        expected.tag_word =
            HexU64((expected.tag_word.get() & !(3 << dest_shift)) | (0 << dest_shift));
        expected.instruction_pointer = input.fixture.initial_state.rip;
        assert_eq!(observed.state.x87.as_ref(), Some(&expected));
    }
}

#[test]
fn local_x87_fchs_fixture_cli_flips_only_the_st0_sign_and_clears_c1() {
    let (input, result) = run(FCHS_NORMAL, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 1);
    let observed = snapshot(&result);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0002);
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.registers[0] = "0000000000000080ffbf".into();
    expected_x87.status_word = HexU64(0x4500);
    expected_x87.instruction_pointer = input.fixture.initial_state.rip;
    assert_eq!(observed.state, expected);
}

#[test]
fn local_x87_fchs_invalid_source_fixture_is_atomic_in_the_cli_runner() {
    let (input, result) = run(FCHS_INVALID_SOURCE, "interpreter", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, input.fixture.initial_state.rip);
    assert_eq!(observed.state.gpr, input.fixture.initial_state.gpr);
    assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);
    assert_eq!(observed.state.x87, input.fixture.initial_state.x87);
}

#[test]
fn local_x87_frndint_tie_and_fnstsw_fixtures_observe_result_and_status() {
    for (fixture, expected_rax, expected_rip) in [
        (FRNDINT_TIE, 0, 0x0010_0002),
        (FRNDINT_FNSTSW, 0x4720, 0x0010_0004),
    ] {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), input.fixture.instruction_budget);
        let observed = snapshot(&result);
        assert_eq!(observed.state.rip, HexU64(expected_rip));
        assert_eq!(observed.state.gpr[0], HexU64(expected_rax));
        let x87 = observed.state.x87.as_ref().unwrap();
        assert_eq!(x87.registers[0], "00000000000000800040");
        assert_eq!(x87.status_word, HexU64(0x4720));
        assert_eq!(x87.instruction_pointer, input.fixture.initial_state.rip);
        assert_eq!(
            x87.tag_word,
            input.fixture.initial_state.x87.as_ref().unwrap().tag_word
        );
    }
}

#[test]
fn local_x87_frndint_preserves_de_from_fld_m32_and_m64_subnormals() {
    for fixture in [FRNDINT_FLD_M32_SUBNORMAL, FRNDINT_FLD_M64_SUBNORMAL] {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 3);
        let observed = snapshot(&result);
        assert_eq!(observed.state.rip, HexU64(0x0010_0006));
        assert_eq!(observed.state.gpr[0], HexU64(0x0000_0000_0030_7d22));
        let x87 = observed.state.x87.as_ref().unwrap();
        assert_eq!(x87.registers[7], "00000000000000000000");
        assert_eq!(x87.status_word, HexU64(0x7d22));
        assert_eq!(x87.tag_word, HexU64(0x7ffc));
        assert_eq!(x87.instruction_pointer, HexU64(0x0010_0002));
        assert_eq!(x87.data_pointer, input.fixture.initial_state.gpr[0]);
    }
}

#[test]
fn local_x87_frndint_seeded_de_fnstsw_uses_all_masked_fcw() {
    let (input, result) = run(FRNDINT_SEEDED_DE_FNSTSW, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 2);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, HexU64(0x0010_0004));
    assert_eq!(observed.state.gpr[0], HexU64(0x6d23));
    let x87 = observed.state.x87.as_ref().unwrap();
    assert_eq!(x87.status_word, HexU64(0x6d23));
    assert_eq!(x87.registers[5], "00000000000000000080");
    assert_eq!(x87.instruction_pointer, input.fixture.initial_state.rip);
}

#[test]
fn local_x87_frndint_seeded_unnormal_fnstsw_fnclex_fixture() {
    let (input, result) = run(FRNDINT_SEEDED_UNNORMAL_FNSTSW_FNCLEX, "interpreter", 0);
    assert!(od_jit::compile(&[0xd9, 0xfc]).is_err());
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 3);
    let observed_state = snapshot(&result);
    assert_eq!(observed_state.state.rip, HexU64(0x0010_0006));
    assert_eq!(observed_state.state.gpr[0].0 & 0xffff, 0x4501);
    let observed = observed_state.state.x87.as_ref().unwrap();
    let initial = &input.fixture.initial_state;
    assert_eq!(observed.registers[0], "00000000000000c0ffff");
    assert_eq!(observed.status_word, HexU64(0x4500));
    assert_eq!(observed.tag_word, initial.x87.as_ref().unwrap().tag_word);
    assert_eq!(
        observed.control_word,
        initial.x87.as_ref().unwrap().control_word
    );
    assert_eq!(observed.instruction_pointer, HexU64(0x0010_0000));
    assert_eq!(
        observed.data_pointer,
        initial.x87.as_ref().unwrap().data_pointer
    );
    assert_eq!(
        observed.last_opcode,
        initial.x87.as_ref().unwrap().last_opcode
    );
}

#[test]
fn local_x87_frndint_seeded_pseudo_infinity_fnstsw_fnclex_fixture() {
    let (input, result) = run(
        FRNDINT_SEEDED_PSEUDO_INFINITY_FNSTSW_FNCLEX,
        "interpreter",
        0,
    );
    assert!(od_jit::compile(&[0xd9, 0xfc]).is_err());
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 3);
    let observed_state = snapshot(&result);
    assert_eq!(observed_state.state.rip, HexU64(0x0010_0006));
    assert_eq!(observed_state.state.gpr[0].0 & 0xffff, 0x4501);
    let observed = observed_state.state.x87.as_ref().unwrap();
    let initial = &input.fixture.initial_state;
    assert_eq!(observed.registers[0], "00000000000000c0ffff");
    assert_eq!(observed.status_word, HexU64(0x4500));
    assert_eq!(observed.tag_word, initial.x87.as_ref().unwrap().tag_word);
    assert_eq!(
        observed.control_word,
        initial.x87.as_ref().unwrap().control_word
    );
    assert_eq!(observed.instruction_pointer, HexU64(0x0010_0000));
    assert_eq!(
        observed.data_pointer,
        initial.x87.as_ref().unwrap().data_pointer
    );
    assert_eq!(
        observed.last_opcode,
        initial.x87.as_ref().unwrap().last_opcode
    );
}

#[test]
fn local_x87_frndint_seeded_pseudo_nan_fnstsw_fnclex_fixtures() {
    assert!(od_jit::compile(&[0xd9, 0xfc]).is_err());
    for (fixture_bytes, seeded_source) in [
        (
            FRNDINT_SEEDED_PSEUDO_NAN_Q0_FNSTSW_FNCLEX,
            "0100000000000000ffff",
        ),
        (
            FRNDINT_SEEDED_PSEUDO_NAN_Q1_FNSTSW_FNCLEX,
            "0000000000000040ffff",
        ),
    ] {
        let (input, result) = run(fixture_bytes, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 3);
        let observed_state = snapshot(&result);
        assert_eq!(observed_state.state.rip, HexU64(0x0010_0006));
        assert_eq!(observed_state.state.gpr[0].0 & 0xffff, 0x4501);
        let observed = observed_state.state.x87.as_ref().unwrap();
        let initial = input.fixture.initial_state.x87.as_ref().unwrap();
        assert_eq!(initial.registers[0], seeded_source);
        assert_eq!(observed.registers[0], "00000000000000c0ffff");
        assert_eq!(observed.status_word, HexU64(0x4500));
        assert_eq!(observed.tag_word, initial.tag_word);
        assert_eq!(observed.control_word, initial.control_word);
        assert_eq!(observed.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(observed.data_pointer, initial.data_pointer);
        assert_eq!(observed.last_opcode, initial.last_opcode);
    }
}

#[test]
fn local_x87_frndint_admits_seeded_ue_and_underflow_composition() {
    let (input, seeded) = run(FRNDINT_SEEDED_UE_FNSTSW, "interpreter", 0);
    assert!(matches!(seeded.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&seeded), 2);
    let observed = snapshot(&seeded);
    assert_eq!(observed.state.gpr[0], HexU64(0x0000_0000_0000_0010));
    let x87 = observed.state.x87.as_ref().unwrap();
    assert_eq!(x87.status_word, HexU64(0x0010));
    assert_eq!(x87.instruction_pointer, input.fixture.initial_state.rip);

    let (input, composed) = run(FST_M32_UNDERFLOW_FRNDINT_FNSTSW, "interpreter", 0);
    assert!(matches!(composed.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&composed), 3);
    let observed = snapshot(&composed);
    assert_eq!(observed.state.rip, HexU64(0x0010_0006));
    assert_eq!(observed.state.gpr[0], HexU64(0x0000_0000_0030_0030));
    let x87 = observed.state.x87.as_ref().unwrap();
    assert_eq!(x87.registers[0], "00000000000000000000");
    assert_eq!(x87.tag_word.0 & 0x3, 0x1);
    assert_eq!(x87.status_word, HexU64(0x0030));
    assert_eq!(x87.instruction_pointer, HexU64(0x0010_0002));
    assert_eq!(x87.data_pointer, input.fixture.initial_state.gpr[0]);
}

#[test]
fn local_x87_frndint_admits_seeded_oe_and_overflow_composition() {
    let (input, seeded) = run(FRNDINT_SEEDED_OE_FNSTSW, "interpreter", 0);
    assert!(matches!(seeded.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&seeded), 2);
    let observed = snapshot(&seeded);
    assert_eq!(observed.state.gpr[0], HexU64(0x0000_0000_0000_0008));
    let x87 = observed.state.x87.as_ref().unwrap();
    assert_eq!(x87.status_word, HexU64(0x0008));
    assert_eq!(x87.instruction_pointer, input.fixture.initial_state.rip);

    let (input, composed) = run(FST_M32_OVERFLOW_FRNDINT_FNSTSW, "interpreter", 0);
    assert!(matches!(composed.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&composed), 3);
    let observed = snapshot(&composed);
    assert_eq!(observed.state.rip, HexU64(0x0010_0006));
    assert_eq!(observed.state.gpr[0], HexU64(0x0000_0000_0030_0028));
    let x87 = observed.state.x87.as_ref().unwrap();
    assert_eq!(x87.registers[0], "ffffffffffffffff7e40");
    assert_eq!(x87.status_word, HexU64(0x0028));
    assert_eq!(x87.instruction_pointer, HexU64(0x0010_0002));
    assert_eq!(x87.data_pointer, input.fixture.initial_state.gpr[0]);
}

#[test]
fn local_x87_frndint_admits_seeded_ze_and_preserves_it_in_fnstsw() {
    let (input, result) = run(FRNDINT_SEEDED_ZE_OE_FNSTSW, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 2);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, HexU64(0x0010_0004));
    assert_eq!(observed.state.gpr[0], HexU64(0x0000_0000_0000_000c));
    let x87 = observed.state.x87.as_ref().unwrap();
    assert_eq!(x87.status_word, HexU64(0x000c));
    assert_eq!(x87.registers[0], "0000000000000080ff3f");
    assert_eq!(
        x87.tag_word,
        input.fixture.initial_state.x87.as_ref().unwrap().tag_word
    );
    assert_eq!(x87.instruction_pointer, input.fixture.initial_state.rip);
}

#[test]
fn local_x87_frndint_admits_seeded_sf_with_ie_and_observes_recomputed_c1() {
    let (input, result) = run(FRNDINT_SEEDED_SF_IE_FNSTSW, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 2);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, HexU64(0x0010_0004));
    assert_eq!(observed.state.gpr[0], HexU64(0x0041));
    let x87 = observed.state.x87.as_ref().unwrap();
    assert_eq!(x87.status_word, HexU64(0x0041));
    assert_eq!(x87.registers[0], "0000000000000080ff3f");
    assert_eq!(
        x87.tag_word,
        input.fixture.initial_state.x87.as_ref().unwrap().tag_word
    );
}

#[test]
fn local_x87_fild_m64int_fixtures_load_signed_qword_and_observe_status() {
    for (fixture, expected_rax, expected_rip) in [
        (FILD_M64INT, 0x0000_0000_0030_0001, 0x0010_0002),
        (FILD_M64INT_FNSTSW, 0x0000_0000_0030_7d00, 0x0010_0004),
    ] {
        let (input, result) = run(fixture, "interpreter", 0);
        let code = input
            .fixture
            .memory
            .iter()
            .find(|region| region.region_id == "code")
            .unwrap();
        assert_eq!(
            code.initial_bytes_hex,
            if fixture == FILD_M64INT_FNSTSW {
                "df28dfe0"
            } else {
                "df28"
            }
        );
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), input.fixture.instruction_budget);
        let observed = snapshot(&result);
        assert_eq!(observed.state.rip, HexU64(expected_rip));
        assert_eq!(observed.state.gpr[0], HexU64(expected_rax));
        let x87 = observed.state.x87.as_ref().unwrap();
        assert_eq!(x87.registers[7], "0000000000000080ff3f");
        assert_eq!(x87.status_word, HexU64(0x7d00));
        assert_eq!(x87.instruction_pointer, input.fixture.initial_state.rip);
        assert_eq!(x87.data_pointer, input.fixture.initial_state.gpr[0]);
        assert_eq!(x87.tag_word, HexU64(0x3ffc));
    }
}

#[test]
fn local_x87_fld_m32fp_subnormal_fixtures_normalize_and_set_de() {
    for (fixture, expected_rax, expected_rip, payload, status) in [
        (
            FLD_M32_SUBNORMAL,
            0x0000_0000_0030_0004,
            0x0010_0002,
            "00000000000000806a3f",
            0x7d02,
        ),
        (
            FLD_M32_SUBNORMAL_FNSTSW,
            0x0000_0000_0030_7d02,
            0x0010_0004,
            "00000000000000806abf",
            0x7d02,
        ),
    ] {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), input.fixture.instruction_budget);
        let observed = snapshot(&result);
        assert_eq!(observed.state.rip, HexU64(expected_rip));
        assert_eq!(observed.state.gpr[0], HexU64(expected_rax));
        let x87 = observed.state.x87.as_ref().unwrap();
        assert_eq!(x87.registers[7], payload);
        assert_eq!(x87.status_word, HexU64(status));
        assert_eq!((x87.tag_word.get() >> 14) & 3, 0);
        assert_eq!(x87.instruction_pointer, input.fixture.initial_state.rip);
        assert_eq!(x87.data_pointer, input.fixture.initial_state.gpr[0]);
    }
}

#[test]
fn local_x87_fld_m32_subnormal_to_m64_stores_preserve_de() {
    for (
        fixture,
        expected_rax,
        expected_rip,
        payload,
        expected_status,
        expected_tag_word,
        output,
    ) in [
        (
            FLD_M32_SUBNORMAL_FST_M64,
            0x0000_0000_0030_0004,
            0x0010_0004,
            "00000000000000806a3f",
            0x7d02,
            0x3ffc,
            "000000000000a036",
        ),
        (
            FLD_M32_NEGATIVE_LARGEST_SUBNORMAL_FSTP_M64_FNSTSW,
            0x0000_0000_0030_4502,
            0x0010_0006,
            "0000000000feffff80bf",
            0x4502,
            0xfffc,
            "000000c0ffff0fb8",
        ),
    ] {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), input.fixture.instruction_budget);
        let observed = snapshot(&result);
        assert_eq!(observed.state.rip, HexU64(expected_rip));
        assert_eq!(observed.state.gpr[0], HexU64(expected_rax));
        let x87 = observed.state.x87.as_ref().unwrap();
        assert_eq!(x87.registers[7], payload);
        assert_eq!(x87.status_word, HexU64(expected_status));
        assert_eq!(x87.tag_word, HexU64(expected_tag_word));
        assert_eq!(x87.instruction_pointer, HexU64(0x0010_0002));
        assert_eq!(x87.data_pointer, HexU64(0x0030_0004));
        let stack = observed
            .memory
            .iter()
            .find(|region| region.region_id == "stack")
            .unwrap();
        assert_eq!(&stack.bytes_hex[..16], output);
    }
}

#[test]
fn local_x87_fld_m64fp_subnormal_fixtures_normalize_and_set_de() {
    for (fixture, expected_rax, expected_rip, payload) in [
        (
            FLD_M64_SUBNORMAL,
            0x0000_0000_0030_0004,
            0x0010_0002,
            "0000000000000080cd3b",
        ),
        (
            FLD_M64_SUBNORMAL_FNSTSW,
            0x0000_0000_0030_7d02,
            0x0010_0004,
            "0000000000000080cdbb",
        ),
    ] {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), input.fixture.instruction_budget);
        let observed = snapshot(&result);
        assert_eq!(observed.state.rip, HexU64(expected_rip));
        assert_eq!(observed.state.gpr[0], HexU64(expected_rax));
        let x87 = observed.state.x87.as_ref().unwrap();
        assert_eq!(x87.registers[7], payload);
        assert_eq!(x87.status_word, HexU64(0x7d02));
        assert_eq!((x87.tag_word.get() >> 14) & 3, 0);
        assert_eq!(x87.instruction_pointer, input.fixture.initial_state.rip);
        assert_eq!(x87.data_pointer, input.fixture.initial_state.gpr[0]);
    }
}

#[test]
fn v2_jit_denies_m64_infinity_qnan_and_snan_loads_without_fallback() {
    for fixture in [
        FLD_M64_INFINITY,
        FLD_M64_QNAN,
        FLD_M64_NEGATIVE_QNAN_FNSTSW,
        FLD_M64_SNAN,
        FLD_M64_NEGATIVE_SNAN_FNSTSW,
    ] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn local_x87_fld_m32_and_m64_invalid_status_fixtures_reject_before_read_without_commit() {
    for fixture in [FLD_M32_INVALID_STATUS, FLD_M64_INVALID_STATUS] {
        let (input, result) = run(fixture, "interpreter", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert_eq!(count(&result), 0);
        let observed = snapshot(&result);
        assert_eq!(observed.state, input.fixture.initial_state);
    }
}

#[test]
fn local_x87_fld_m32_and_m64_seeded_ue_fnstsw_fixtures_preserve_history() {
    for (fixture, opcode, expected_rax, expected_payload, expected_tag) in [
        (
            FLD_M32_SEEDED_UE_FNSTSW,
            "d900dfe0",
            0x0000_0000_0030_3810,
            "0000000000000080ff3f",
            0,
        ),
        (
            FLD_M64_SEEDED_UE_FNSTSW,
            "dd00dfe0",
            0x0000_0000_0030_3810,
            "0000000000000080ff3f",
            0,
        ),
    ] {
        let (input, result) = run(fixture, "interpreter", 0);
        let code = input
            .fixture
            .memory
            .iter()
            .find(|region| region.region_id == "code")
            .unwrap();
        assert_eq!(code.initial_bytes_hex, opcode);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 2);
        let observed = snapshot(&result);
        assert_eq!(observed.state.rip, HexU64(0x0010_0004));
        assert_eq!(observed.state.gpr[0], HexU64(expected_rax));
        let x87 = observed.state.x87.as_ref().unwrap();
        assert_eq!(x87.registers[7], expected_payload);
        assert_eq!(x87.status_word, HexU64(0x3810));
        assert_eq!(x87.tag_word, HexU64(0x3ffc));
        assert_eq!((x87.tag_word.get() >> 14) & 3, expected_tag);
        assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(x87.data_pointer, HexU64(0x0030_0004));
    }
}

#[test]
fn v2_jit_denies_m32_infinity_load_without_interpreter_fallback() {
    let (_, positive) = run(FLD_M32_INFINITY, "jit", 4);
    assert!(matches!(positive.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(positive.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        positive.instructions_executed,
        Observation::Unavailable { .. }
    ));
    let (_, negative) = run(FLD_M32_INFINITY_FNSTSW, "jit", 4);
    assert!(matches!(negative.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(negative.snapshot, Observation::Unavailable { .. }));
    for fixture in [
        FLD_M32_QNAN,
        FLD_M32_NEGATIVE_QNAN_FNSTSW,
        FLD_M32_SNAN,
        FLD_M32_NEGATIVE_SNAN_FNSTSW,
    ] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    }
}

#[test]
fn local_x87_frndint_invalid_tag_fixture_is_atomic() {
    let (input, result) = run(FRNDINT_INVALID_TAG, "interpreter", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, input.fixture.initial_state.rip);
    assert_eq!(observed.state.gpr, input.fixture.initial_state.gpr);
    assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);
    assert_eq!(observed.state.x87, input.fixture.initial_state.x87);
}

#[test]
fn local_x87_fstp_m32fp_fixtures_store_binary32_then_pop_and_observe_fsw() {
    for (fixture, expected_rax, expected_rip, expected_status, bytes) in [
        (
            FSTP_M32FP,
            0x0000_0000_0030_0004,
            0x0010_0002,
            0x4d00,
            "0000c03f",
        ),
        (
            FSTP_M32FP_FNSTSW,
            0x0000_0000_0030_4d00,
            0x0010_0004,
            0x4d00,
            "0000c03f",
        ),
    ] {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), input.fixture.instruction_budget);
        let observed = snapshot(&result);
        assert_eq!(observed.state.rip, HexU64(expected_rip));
        assert_eq!(observed.state.gpr[0], HexU64(expected_rax));
        let x87 = observed.state.x87.as_ref().unwrap();
        let initial = input.fixture.initial_state.x87.as_ref().unwrap();
        assert_eq!(x87.registers, initial.registers);
        assert_eq!(x87.control_word, initial.control_word);
        assert_eq!(x87.status_word, HexU64(expected_status));
        assert_eq!(x87.tag_word, HexU64(0xffff));
        assert_eq!(x87.instruction_pointer, input.fixture.initial_state.rip);
        assert_eq!(x87.data_pointer, HexU64(0x300004));
        let stack = observed
            .memory
            .iter()
            .find(|region| region.region_id == "stack")
            .unwrap();
        assert_eq!(&stack.bytes_hex[8..16], bytes);
    }
}

#[test]
fn local_x87_fst_m32fp_fixtures_store_binary32_without_pop() {
    for (fixture, expected_rax, expected_rip, code, budget) in [
        (FST_M32FP, 0x0000_0000_0030_0004, 0x0010_0002, "d910", 1),
        (
            FST_M32FP_FNSTSW,
            0x0000_0000_0030_4500,
            0x0010_0004,
            "d910dfe0",
            2,
        ),
    ] {
        let (input, result) = run(fixture, "interpreter", 0);
        assert_eq!(input.fixture.instruction_budget, budget);
        let code_region = input
            .fixture
            .memory
            .iter()
            .find(|region| region.region_id == "code")
            .unwrap();
        assert_eq!(code_region.initial_bytes_hex, code);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), budget);
        let observed = snapshot(&result);
        assert_eq!(observed.state.rip, HexU64(expected_rip));
        assert_eq!(observed.state.gpr[0], HexU64(expected_rax));
        let x87 = observed.state.x87.as_ref().unwrap();
        let initial = input.fixture.initial_state.x87.as_ref().unwrap();
        assert_eq!(x87.registers, initial.registers);
        assert_eq!(x87.tag_word, initial.tag_word);
        assert_eq!(x87.control_word, initial.control_word);
        assert_eq!(x87.status_word, HexU64(0x4500));
        assert_eq!(x87.instruction_pointer, input.fixture.initial_state.rip);
        assert_eq!(x87.data_pointer, HexU64(0x300004));
        let stack = observed
            .memory
            .iter()
            .find(|region| region.region_id == "stack")
            .unwrap();
        assert_eq!(&stack.bytes_hex[8..16], "0000c03f");
    }
}

#[test]
fn local_x87_fst_m32fp_signed_zero_fixtures_preserve_or_pop_after_store() {
    for (fixture, expected_rax, expected_rip, expected_status, expected_tag_word, bytes) in [
        (
            FST_M32FP_POSITIVE_ZERO,
            0x0030_0004,
            0x0010_0002,
            0x4500,
            0xfffd,
            "00000000",
        ),
        (
            FST_M32FP_NEGATIVE_ZERO_FNSTSW,
            0x0030_4500,
            0x0010_0004,
            0x4500,
            0xfffd,
            "00000080",
        ),
        (
            FSTP_M32FP_POSITIVE_ZERO,
            0x0030_0004,
            0x0010_0002,
            0x4d00,
            0xffff,
            "00000000",
        ),
        (
            FSTP_M32FP_NEGATIVE_ZERO_FNSTSW,
            0x0030_4d00,
            0x0010_0004,
            0x4d00,
            0xffff,
            "00000080",
        ),
    ] {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), input.fixture.instruction_budget);
        let observed = snapshot(&result);
        assert_eq!(observed.state.rip, HexU64(expected_rip));
        assert_eq!(observed.state.gpr[0], HexU64(expected_rax));
        let x87 = observed.state.x87.as_ref().unwrap();
        assert_eq!(x87.status_word, HexU64(expected_status));
        assert_eq!(x87.tag_word, HexU64(expected_tag_word));
        assert_eq!(
            x87.registers[0],
            if bytes == "00000080" {
                "00000000000000000080"
            } else {
                "00000000000000000000"
            }
        );
        assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(x87.data_pointer, HexU64(0x0030_0004));
        let stack = observed
            .memory
            .iter()
            .find(|region| region.region_id == "stack")
            .unwrap();
        assert_eq!(&stack.bytes_hex[8..16], bytes);
    }
}

#[test]
fn local_x87_fst_m64fp_signed_zero_fixtures_preserve_or_pop_after_store() {
    for (fixture, expected_rax, expected_rip, expected_status, expected_tag_word, bytes) in [
        (
            FST_M64FP_POSITIVE_ZERO,
            0x0030_0004,
            0x0010_0002,
            0x4500,
            0xfffd,
            "0000000000000000",
        ),
        (
            FST_M64FP_NEGATIVE_ZERO_FNSTSW,
            0x0030_4500,
            0x0010_0004,
            0x4500,
            0xfffd,
            "0000000000000080",
        ),
        (
            FSTP_M64FP_POSITIVE_ZERO,
            0x0030_0004,
            0x0010_0002,
            0x4d00,
            0xffff,
            "0000000000000000",
        ),
        (
            FSTP_M64FP_NEGATIVE_ZERO_FNSTSW,
            0x0030_4d00,
            0x0010_0004,
            0x4d00,
            0xffff,
            "0000000000000080",
        ),
    ] {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), input.fixture.instruction_budget);
        let observed = snapshot(&result);
        assert_eq!(observed.state.rip, HexU64(expected_rip));
        assert_eq!(observed.state.gpr[0], HexU64(expected_rax));
        let x87 = observed.state.x87.as_ref().unwrap();
        assert_eq!(x87.status_word, HexU64(expected_status));
        assert_eq!(x87.tag_word, HexU64(expected_tag_word));
        assert_eq!(
            x87.registers[0],
            if bytes.ends_with("80") {
                "00000000000000000080"
            } else {
                "00000000000000000000"
            }
        );
        assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(x87.data_pointer, HexU64(0x0030_0004));
        let stack = observed
            .memory
            .iter()
            .find(|region| region.region_id == "stack")
            .unwrap();
        assert_eq!(&stack.bytes_hex[8..24], bytes);
    }
}

#[test]
fn local_x87_fldcw_fixture_covers_all_old_and_loaded_control_word_pairs() {
    let words = [0x037f, 0x077f, 0x0b7f, 0x0f7f];
    for old in words {
        for loaded in words {
            let mut fixture = Fixture::from_json(FLDCW_FRNDINT_FNSTSW).unwrap().fixture;
            fixture.case_id = format!("local_x87_fldcw_{old:04x}_{loaded:04x}");
            fixture.initial_state.x87.as_mut().unwrap().control_word = HexU64(old);
            fixture.initial_state.x87.as_mut().unwrap().status_word = HexU64(0x4720);
            fixture.initial_state.gpr[0] = HexU64(0x0000_0000_0030_0001);
            let code = fixture
                .memory
                .iter_mut()
                .find(|region| region.region_id == "code")
                .unwrap();
            code.length = 2;
            code.initial_bytes_hex = "d928".into();
            code.comparison_mask_hex = "ffff".into();
            let stack = fixture
                .memory
                .iter_mut()
                .find(|region| region.region_id == "stack")
                .unwrap();
            let little_endian = loaded.to_le_bytes();
            stack.initial_bytes_hex.replace_range(
                2..6,
                &format!("{:02x}{:02x}", little_endian[0], little_endian[1]),
            );
            fixture.stop_rip = Some(HexU64(0x0010_0002));
            fixture.instruction_budget = 1;

            let (input, result) = run(&fixture.to_json().unwrap(), "interpreter", 0);
            assert!(matches!(result.outcome, Outcome::StopAddress {}));
            assert_eq!(count(&result), 1);
            let observed = snapshot(&result);
            assert_eq!(observed.state.rip, HexU64(0x0010_0002));
            assert_eq!(observed.state.gpr, input.fixture.initial_state.gpr);
            assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);
            let mut expected = input.fixture.initial_state.x87.clone().unwrap();
            expected.control_word = HexU64(loaded);
            assert_eq!(observed.state.x87.as_ref(), Some(&expected));
        }
    }
}

#[test]
fn local_x87_fldcw_frndint_fnstsw_composition_uses_loaded_rounding_mode() {
    let (input, result) = run(FLDCW_FRNDINT_FNSTSW, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 3);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, HexU64(0x0010_0006));
    assert_eq!(observed.state.gpr[0].get() & 0xffff, 0x4720);
    let mut expected = input.fixture.initial_state.x87.clone().unwrap();
    expected.control_word = HexU64(0x0b7f);
    expected.registers[0] = "00000000000000800040".into();
    expected.status_word = HexU64(0x4720);
    expected.instruction_pointer = HexU64(0x0010_0002);
    assert_eq!(observed.state.x87.as_ref(), Some(&expected));
}

#[test]
fn local_x87_fabs_fixture_cli_clears_only_the_st0_sign_and_clears_c1() {
    let (input, result) = run(FABS_NORMAL, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 1);
    let observed = snapshot(&result);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0002);
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.registers[0] = "0000000000000080ff3f".into();
    expected_x87.status_word = HexU64(0x4500);
    expected_x87.instruction_pointer = input.fixture.initial_state.rip;
    assert_eq!(observed.state, expected);
}

#[test]
fn local_x87_fabs_invalid_source_fixture_is_atomic_in_the_cli_runner() {
    let (input, result) = run(FABS_INVALID_SOURCE, "interpreter", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, input.fixture.initial_state.rip);
    assert_eq!(observed.state.gpr, input.fixture.initial_state.gpr);
    assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);
    assert_eq!(observed.state.x87, input.fixture.initial_state.x87);
}

#[test]
fn local_x87_ftst_fixtures_set_positive_and_negative_condition_codes() {
    for (fixture, raw, status) in [
        (FTST_POSITIVE, "0000000000000080ff3f", 0u64),
        (FTST_NEGATIVE, "0000000000000080ffbf", 0x0100),
    ] {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 1);
        let observed = snapshot(&result);
        let mut expected = input.fixture.initial_state.clone();
        expected.rip = HexU64(0x0010_0002);
        let expected_x87 = expected.x87.as_mut().unwrap();
        expected_x87.registers[0] = raw.into();
        expected_x87.status_word = HexU64(status);
        expected_x87.instruction_pointer = input.fixture.initial_state.rip;
        assert_eq!(observed.state, expected);
    }
}

#[test]
fn local_x87_ftst_empty_tag_fixture_is_atomic_in_the_cli_runner() {
    let (input, result) = run(FTST_INVALID_SOURCE, "interpreter", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, input.fixture.initial_state.rip);
    assert_eq!(observed.state.gpr, input.fixture.initial_state.gpr);
    assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);
    assert_eq!(observed.state.x87, input.fixture.initial_state.x87);
}

#[test]
fn local_x87_fcom_st1_literal_fixtures_set_greater_equal_and_less_conditions() {
    for (fixture, condition) in [
        (FCOM_ST1_GREATER, 0u64),
        (FCOM_ST1_EQUAL, 0x4000),
        (FCOM_ST1_LESS, 0x0100),
    ] {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 1);
        let observed = snapshot(&result);
        let mut expected = input.fixture.initial_state.clone();
        expected.rip = HexU64(0x0010_0002);
        let expected_x87 = expected.x87.as_mut().unwrap();
        expected_x87.status_word = HexU64(condition);
        expected_x87.instruction_pointer = input.fixture.initial_state.rip;
        assert_eq!(observed.state, expected);
    }
}

#[test]
fn local_x87_fcom_st1_invalid_rhs_tag_fixture_is_atomic_with_budget_one() {
    let (input, result) = run(FCOM_ST1_INVALID_RHS_TAG, "interpreter", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    let observed = snapshot(&result);
    assert_eq!(observed.state, input.fixture.initial_state);
}

#[test]
fn local_x87_fcom_st1_composes_with_fnstsw_ax() {
    let mut fixture = Fixture::from_json(FCOM_ST1_LESS).unwrap().fixture;
    fixture.case_id = "local_x87_fcom_st1_fnstsw_ax".into();
    fixture.instruction_budget = 2;
    fixture.stop_rip = Some(HexU64(0x0010_0004));
    let code = fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.length = 4;
    code.initial_bytes_hex = "d8d1dfe0".into();
    code.comparison_mask_hex = "ffffffff".into();
    let bytes = fixture.to_json().unwrap();
    let (input, result) = run(&bytes, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 2);
    let observed = snapshot(&result);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0004);
    expected.gpr[0] = HexU64((expected.gpr[0].get() & !0xffff) | 0x0100);
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.status_word = HexU64(0x0100);
    expected_x87.instruction_pointer = input.fixture.initial_state.rip;
    assert_eq!(observed.state, expected);
}

#[test]
fn local_x87_fcom_sti_cli_vectors_cover_every_source_index() {
    for source in 0u8..=7 {
        let mut fixture = Fixture::from_json(FCOM_ST1_EQUAL).unwrap().fixture;
        fixture.case_id = format!("local_x87_fcom_st{source}_cli");
        let x87 = fixture.initial_state.x87.as_mut().unwrap();
        x87.registers[usize::from(source)] = x87.registers[0].clone();
        x87.tag_word = HexU64(0);
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("d8d{source:x}");
        code.comparison_mask_hex = "ffff".into();
        let (input, result) = run(&fixture.to_json().unwrap(), "interpreter", 0);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "i={source}"
        );
        assert_eq!(count(&result), 1, "i={source}");
        let observed = snapshot(&result);
        let mut expected = input.fixture.initial_state.clone();
        expected.rip = HexU64(0x0010_0002);
        let expected_x87 = expected.x87.as_mut().unwrap();
        expected_x87.status_word = HexU64(0x4000);
        expected_x87.instruction_pointer = input.fixture.initial_state.rip;
        assert_eq!(observed.state, expected, "i={source}");
    }
}

#[test]
fn local_x87_fcom_st7_literal_fixture_and_invalid_rhs_tag_are_handled() {
    let (input, result) = run(FCOM_ST7_EQUAL, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 1);
    let observed = snapshot(&result);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0002);
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.status_word = HexU64(0x4000);
    expected_x87.instruction_pointer = input.fixture.initial_state.rip;
    assert_eq!(observed.state, expected);

    let (input, result) = run(FCOM_ST7_INVALID_RHS_TAG, "interpreter", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    assert_eq!(snapshot(&result).state, input.fixture.initial_state);
}

#[test]
fn local_x87_fcomp_sti_literal_fixtures_compare_then_pop_all_indices() {
    for (source, fixture) in FCOMP_STI_EQUAL.into_iter().enumerate() {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "i={source}"
        );
        assert_eq!(count(&result), 1, "i={source}");
        let observed = snapshot(&result);
        let mut expected = input.fixture.initial_state.clone();
        expected.rip = HexU64(0x0010_0002);
        let expected_x87 = expected.x87.as_mut().unwrap();
        expected_x87.status_word = HexU64(0x4800);
        expected_x87.tag_word = HexU64(
            (input
                .fixture
                .initial_state
                .x87
                .as_ref()
                .unwrap()
                .tag_word
                .get()
                & !3)
                | 3,
        );
        expected_x87.instruction_pointer = input.fixture.initial_state.rip;
        assert_eq!(observed.state, expected, "i={source}");
    }

    let (input, result) = run(FCOMP_ST1_INVALID_RHS_TAG, "interpreter", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    assert_eq!(snapshot(&result).state, input.fixture.initial_state);
}

#[test]
fn local_x87_fcomp_st1_cli_observation_sequence_preserves_popped_status() {
    let mut fixture = Fixture::from_json(FCOMP_ST1_EQUAL).unwrap().fixture;
    fixture.case_id = "local_x87_fcomp_st1_fnstsw_ax".into();
    fixture.instruction_budget = 2;
    fixture.stop_rip = Some(HexU64(0x0010_0004));
    let code = fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.length = 4;
    code.initial_bytes_hex = "d8d9dfe0".into();
    code.comparison_mask_hex = "ffffffff".into();
    let (input, result) = run(&fixture.to_json().unwrap(), "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 2);
    let observed = snapshot(&result);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0004);
    expected.gpr[0] = HexU64((expected.gpr[0].get() & !0xffff) | 0x4800);
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.status_word = HexU64(0x4800);
    expected_x87.tag_word = HexU64(0xfff3);
    expected_x87.instruction_pointer = input.fixture.initial_state.rip;
    assert_eq!(observed.state, expected);
}

#[test]
fn local_x87_fcompp_fixture_observes_two_entry_pop_and_rejects_bad_rhs_atomically() {
    let (input, result) = run(FCOMPP_EQUAL_FNSTSW, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 2);
    let observed = snapshot(&result);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0004);
    expected.gpr[0] = HexU64((expected.gpr[0].get() & !0xffff) | 0x5000);
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.status_word = HexU64(0x5000);
    expected_x87.tag_word = HexU64(0xffff);
    expected_x87.instruction_pointer = input.fixture.initial_state.rip;
    assert_eq!(observed.state, expected);

    let (input, result) = run(FCOMPP_INVALID_RHS_TAG, "interpreter", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    assert_eq!(snapshot(&result).state, input.fixture.initial_state);
}

#[test]
fn local_x87_fucompp_fixture_observes_two_entry_pop_and_rejects_bad_rhs_atomically() {
    let (input, result) = run(FUCOMPP_EQUAL_FNSTSW, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 2);
    let observed = snapshot(&result);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0004);
    expected.gpr[0] = HexU64((expected.gpr[0].get() & !0xffff) | 0x5000);
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.status_word = HexU64(0x5000);
    expected_x87.tag_word = HexU64(0xffff);
    expected_x87.instruction_pointer = input.fixture.initial_state.rip;
    assert_eq!(observed.state, expected);

    let (input, result) = run(FUCOMPP_INVALID_RHS_TAG, "interpreter", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    assert_eq!(snapshot(&result).state, input.fixture.initial_state);
}

#[test]
fn local_x87_fcomi_fixture_observes_flags_and_rejects_bad_rhs_atomically() {
    let (input, result) = run(FCOMI_EQUAL_FNSTSW, "interpreter", 0);
    assert_eq!(input.fixture.memory[0].initial_bytes_hex, "dbf1dfe0");
    let mut wrong_schema_program = input.fixture.clone();
    wrong_schema_program.memory[0].initial_bytes_hex = "dbf8dfe0".into();
    assert!(wrong_schema_program.to_json().is_err());
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 2);
    let observed = snapshot(&result);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0004);
    expected.gpr[0] = HexU64((expected.gpr[0].get() & !0xffff) | 0x4500);
    expected.rflags = HexU64((expected.rflags.get() & !0x8d5) | 0x40);
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.status_word = HexU64(0x4500);
    expected_x87.instruction_pointer = input.fixture.initial_state.rip;
    assert_eq!(observed.state, expected);

    let (input, result) = run(FCOMI_INVALID_RHS_TAG, "interpreter", 4);
    assert_eq!(input.fixture.memory[0].initial_bytes_hex, "dbf1");
    assert_eq!(input.fixture.instruction_budget, 1);
    assert_eq!(input.fixture.stop_rip, None);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    assert_eq!(snapshot(&result).state, input.fixture.initial_state);
}

#[test]
fn local_x87_fcomip_fixture_observes_flags_and_pop_and_rejects_bad_rhs_atomically() {
    let (input, result) = run(FCOMIP_EQUAL_FNSTSW, "interpreter", 0);
    assert_eq!(input.fixture.memory[0].initial_bytes_hex, "dff1dfe0");
    let mut wrong_schema_program = input.fixture.clone();
    wrong_schema_program.memory[0].initial_bytes_hex = "dff8dfe0".into();
    assert!(wrong_schema_program.to_json().is_err());
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 2);
    let observed = snapshot(&result);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0004);
    expected.gpr[0] = HexU64((expected.gpr[0].get() & !0xffff) | 0x4d00);
    expected.rflags = HexU64((expected.rflags.get() & !0x8d5) | 0x40);
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.status_word = HexU64(0x4d00);
    expected_x87.tag_word = HexU64(0xfff3);
    expected_x87.instruction_pointer = input.fixture.initial_state.rip;
    assert_eq!(observed.state, expected);

    let (input, result) = run(FCOMIP_INVALID_RHS_TAG, "interpreter", 4);
    assert_eq!(input.fixture.memory[0].initial_bytes_hex, "dff1");
    assert_eq!(input.fixture.instruction_budget, 1);
    assert_eq!(input.fixture.stop_rip, None);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    assert_eq!(snapshot(&result).state, input.fixture.initial_state);
}

#[test]
fn local_x87_fucomip_fixture_observes_flags_and_pop_and_rejects_bad_rhs_atomically() {
    let (input, result) = run(FUCOMIP_EQUAL_FNSTSW, "interpreter", 0);
    assert_eq!(input.fixture.memory[0].initial_bytes_hex, "dfe9dfe0");
    let mut wrong_schema_program = input.fixture.clone();
    wrong_schema_program.memory[0].initial_bytes_hex = "dff8dfe0".into();
    assert!(wrong_schema_program.to_json().is_err());
    for rejected in ["dff8dfe0", "66dfe9dfe0", "48dfe9dfe0", "9bdfe9dfe0"] {
        let mut wrong_schema_program = input.fixture.clone();
        wrong_schema_program.memory[0].initial_bytes_hex = rejected.into();
        assert!(wrong_schema_program.to_json().is_err(), "{rejected}");
    }
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 2);
    let observed = snapshot(&result);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0004);
    expected.gpr[0] = HexU64((expected.gpr[0].get() & !0xffff) | 0x4d00);
    expected.rflags = HexU64((expected.rflags.get() & !0x8d5) | 0x40);
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.status_word = HexU64(0x4d00);
    expected_x87.tag_word = HexU64(0xfff3);
    expected_x87.instruction_pointer = input.fixture.initial_state.rip;
    assert_eq!(observed.state, expected);

    let (input, result) = run(FUCOMIP_INVALID_RHS_TAG, "interpreter", 4);
    assert_eq!(input.fixture.memory[0].initial_bytes_hex, "dfe9");
    assert_eq!(input.fixture.instruction_budget, 1);
    assert_eq!(input.fixture.stop_rip, None);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    assert_eq!(snapshot(&result).state, input.fixture.initial_state);
}

#[test]
fn local_x87_fucomi_fixture_observes_flags_and_rejects_bad_rhs_atomically() {
    let (input, result) = run(FUCOMI_EQUAL_FNSTSW, "interpreter", 0);
    assert_eq!(input.fixture.memory[0].initial_bytes_hex, "dbe9dfe0");
    let mut wrong_schema_program = input.fixture.clone();
    wrong_schema_program.memory[0].initial_bytes_hex = "dbf8dfe0".into();
    assert!(wrong_schema_program.to_json().is_err());
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 2);
    let observed = snapshot(&result);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0004);
    expected.gpr[0] = HexU64((expected.gpr[0].get() & !0xffff) | 0x4500);
    expected.rflags = HexU64((expected.rflags.get() & !0x8d5) | 0x40);
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.status_word = HexU64(0x4500);
    expected_x87.instruction_pointer = input.fixture.initial_state.rip;
    assert_eq!(observed.state, expected);

    let (input, result) = run(FUCOMI_INVALID_RHS_TAG, "interpreter", 4);
    assert_eq!(input.fixture.memory[0].initial_bytes_hex, "dbe9");
    assert_eq!(input.fixture.instruction_budget, 1);
    assert_eq!(input.fixture.stop_rip, None);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    assert_eq!(snapshot(&result).state, input.fixture.initial_state);
}

#[test]
fn local_x87_fcomi_indexed_fixtures_cover_self_alias_wrap_and_actual_source_rejects() {
    for (bytes, top, expected_fsw, pop) in [
        ("dbf0dfe0", 3u8, 0x1800u16, false),
        ("dfe8dfe0", 7, 0, true),
        ("dbefdfe0", 7, 0x3800, false),
        ("dff6dfe0", 6, 0x3800, true),
    ] {
        let data = match bytes {
            "dbf0dfe0" => S2_30AN_SELF_FCOMI,
            "dfe8dfe0" => S2_30AN_SELF_FUCOMIP,
            "dbefdfe0" => S2_30AN_FUCOMI_ST7,
            "dff6dfe0" => S2_30AN_FCOMIP_ST6,
            _ => unreachable!(),
        };
        let (input, result) = run(data, "interpreter", 0);
        assert_eq!(input.fixture.memory[0].initial_bytes_hex, bytes);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 2);
        let observed = snapshot(&result);
        let mut expected = input.fixture.initial_state.clone();
        expected.rip = HexU64(input.fixture.initial_state.rip.get() + 4);
        expected.rflags = HexU64((expected.rflags.get() & !0x8d5) | 0x40);
        expected.gpr[0] = HexU64((expected.gpr[0].get() & !0xffff) | u64::from(expected_fsw));
        let expected_x87 = expected.x87.as_mut().unwrap();
        expected_x87.status_word = HexU64(u64::from(expected_fsw));
        expected_x87.instruction_pointer = input.fixture.initial_state.rip;
        if pop {
            expected_x87.tag_word = HexU64(
                (expected_x87.tag_word.get() & !(3u64 << (u32::from(top) * 2)))
                    | (3u64 << (u32::from(top) * 2)),
            );
        }
        assert_eq!(observed.state, expected, "{bytes}");
    }

    for data in [S2_30AN_INVALID_SELF, S2_30AN_INVALID_ST7] {
        let (input, result) = run(data, "interpreter", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert_eq!(count(&result), 0);
        assert_eq!(snapshot(&result).state, input.fixture.initial_state);
    }
}

#[test]
fn local_x87_state_profile_runs_only_exact_fadd_and_rejects_inexact_sum_without_commit() {
    let mut fixture = Fixture::from_json(MFENCE_STORE_FENCE_LOAD).unwrap().fixture;
    fixture.case_id = "local_x87_fadd_exact".into();
    fixture.profile = od_testkit::v2::Profile::LongX87StateV1;
    fixture.initial_state.rip = HexU64(0x0010_0000);
    let mut seed = od_isa::state::X87State::fixture_seed();
    seed.control_word = 0x037f;
    seed.status_word = 0x0200 | 0x0100 | 0x0400 | 0x4000;
    seed.tag_word = 0;
    seed.instruction_pointer = 0x1234;
    seed.data_pointer = 0x5678;
    seed.last_opcode = 0x05aa;
    seed.registers[0] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f]; // exact 1 + 1 -> 2
    seed.registers[1] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f];
    fixture.initial_state.x87 = Some(od_testkit::v2::X87Snapshot::from(&seed));
    let code = fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.guest_address = HexU64(0x0010_0000);
    code.length = 2;
    code.initial_bytes_hex = "d8c1".into();
    code.comparison_mask_hex = "ffff".into();
    fixture.instruction_budget = 1;
    fixture.stop_rip = Some(HexU64(0x0010_0002));
    fixture.expected_exit = ExpectedExit::StopAddress;

    let (input, result) = run(&fixture.to_json().unwrap(), "interpreter", 0);
    assert_eq!(count(&result), 1);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, HexU64(0x0010_0002));
    let mut expected = seed.clone();
    expected.registers[0] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0x00, 0x40];
    expected.status_word &= !(1 << 9);
    expected.instruction_pointer = 0x0010_0000;
    assert_eq!(
        observed.state.x87,
        Some(od_testkit::v2::X87Snapshot::from(&expected))
    );
    assert_eq!(observed.state.gpr, input.fixture.initial_state.gpr);
    assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);

    // An odd 65-bit coefficient would require dropping a real bit.
    let mut rejected = fixture;
    rejected.case_id = "local_x87_fadd_inexact_rejected".into();
    let mut inexact = seed;
    inexact.registers[1] = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x3f];
    rejected.initial_state.x87 = Some(od_testkit::v2::X87Snapshot::from(&inexact));
    let (input, result) = run(&rejected.to_json().unwrap(), "interpreter", 4);
    assert_eq!(count(&result), 0);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(snapshot(&result).state, input.fixture.initial_state);
}

#[test]
fn local_x87_state_profile_runs_exact_fsub_sources_and_rejects_i0_atomically() {
    for source_index in 1u8..=7 {
        let mut fixture = Fixture::from_json(MFENCE_STORE_FENCE_LOAD).unwrap().fixture;
        fixture.case_id = format!("local_x87_fsub_i{source_index}");
        fixture.profile = od_testkit::v2::Profile::LongX87StateV1;
        fixture.initial_state.rip = HexU64(0x0010_0000);
        let top = 3u8;
        let destination = usize::from(top);
        let source = usize::from((top + source_index) & 7);
        let mut seed = od_isa::state::X87State::fixture_seed();
        seed.control_word = 0x037f;
        seed.status_word = (u16::from(top) << 11) | 0x4700;
        seed.tag_word = u16::MAX & !(3 << (destination * 2)) & !(3 << (source * 2));
        seed.registers[destination] = [0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0x3f]; // 1.5
        seed.registers[source] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f]; // 1.0
        seed.instruction_pointer = 0x1234;
        seed.data_pointer = 0x5678;
        seed.last_opcode = 0x05aa;
        fixture.initial_state.x87 = Some(od_testkit::v2::X87Snapshot::from(&seed));
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.guest_address = HexU64(0x0010_0000);
        code.length = 2;
        code.initial_bytes_hex = format!("d8e{source_index:x}");
        code.comparison_mask_hex = "ffff".into();
        fixture.instruction_budget = 1;
        fixture.stop_rip = Some(HexU64(0x0010_0002));
        fixture.expected_exit = ExpectedExit::StopAddress;

        let (input, result) = run(&fixture.to_json().unwrap(), "interpreter", 0);
        assert_eq!(count(&result), 1);
        let observed = snapshot(&result);
        assert_eq!(observed.state.rip, HexU64(0x0010_0002));
        let mut expected = seed;
        expected.registers[destination] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xfe, 0x3f]; // 0.5
        expected.status_word &= !(1 << 9);
        expected.instruction_pointer = 0x0010_0000;
        assert_eq!(
            observed.state.x87,
            Some(od_testkit::v2::X87Snapshot::from(&expected))
        );
        assert_eq!(observed.state.gpr, input.fixture.initial_state.gpr);
        assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);
    }

    let mut rejected = Fixture::from_json(MFENCE_STORE_FENCE_LOAD).unwrap().fixture;
    rejected.case_id = "local_x87_fsub_i0_unsupported".into();
    rejected.profile = od_testkit::v2::Profile::LongX87StateV1;
    rejected.initial_state.rip = HexU64(0x0010_0000);
    let mut x87 = od_isa::state::X87State::fixture_seed();
    x87.control_word = 0x037f;
    x87.status_word = 0x0200;
    x87.tag_word = 0xfffc;
    x87.registers[0] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f];
    rejected.initial_state.x87 = Some(od_testkit::v2::X87Snapshot::from(&x87));
    let code = rejected
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.guest_address = HexU64(0x0010_0000);
    code.length = 2;
    code.initial_bytes_hex = "d8e0".into();
    code.comparison_mask_hex = "ffff".into();
    rejected.instruction_budget = 1;
    rejected.stop_rip = None;
    rejected.expected_exit = ExpectedExit::Unsupported;
    let (input, result) = run(&rejected.to_json().unwrap(), "interpreter", 4);
    assert_eq!(count(&result), 0);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(snapshot(&result).state, input.fixture.initial_state);
}

#[test]
fn local_x87_state_profile_runs_fmul_for_every_top_and_register_index() {
    for top in 0u8..8 {
        for source_index in 0u8..8 {
            let mut fixture = Fixture::from_json(MFENCE_STORE_FENCE_LOAD).unwrap().fixture;
            fixture.case_id = format!("local_x87_fmul_top{top}_i{source_index}");
            fixture.profile = od_testkit::v2::Profile::LongX87StateV1;
            fixture.initial_state.rip = HexU64(0x0010_0000);
            let physical_top = usize::from(top);
            let physical_source = usize::from((top + source_index) & 7);
            let mut seed = od_isa::state::X87State::fixture_seed();
            seed.control_word = 0x037f;
            seed.status_word = (u16::from(top) << 11) | 0x4700 | (1 << 9);
            seed.tag_word = u16::MAX;
            seed.registers[physical_top] = [0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0x3f]; // 1.5
            seed.set_tag(top, od_isa::state::X87Tag::Valid);
            if source_index != 0 {
                seed.registers[physical_source] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0x00, 0x40]; // 2.0
                seed.set_tag(physical_source as u8, od_isa::state::X87Tag::Valid);
            }
            seed.data_pointer = 0x5678;
            seed.last_opcode = 0x05aa;
            fixture.initial_state.x87 = Some(od_testkit::v2::X87Snapshot::from(&seed));
            let code = fixture
                .memory
                .iter_mut()
                .find(|region| region.region_id == "code")
                .unwrap();
            code.guest_address = HexU64(0x0010_0000);
            code.length = 2;
            code.initial_bytes_hex = format!("d8c{:x}", 8 + source_index);
            code.comparison_mask_hex = "ffff".into();
            fixture.instruction_budget = 1;
            fixture.stop_rip = Some(HexU64(0x0010_0002));
            fixture.expected_exit = ExpectedExit::StopAddress;

            let (input, result) = run(&fixture.to_json().unwrap(), "interpreter", 0);
            assert_eq!(count(&result), 1, "TOP={top}, i={source_index}");
            let observed = snapshot(&result);
            let mut expected = seed;
            expected.registers[physical_top] = if source_index == 0 {
                [0, 0, 0, 0, 0, 0, 0, 0x90, 0x00, 0x40] // 2.25
            } else {
                [0, 0, 0, 0, 0, 0, 0, 0xc0, 0x00, 0x40] // 3.0
            };
            expected.status_word &= !(1 << 9);
            expected.instruction_pointer = 0x0010_0000;
            assert_eq!(observed.state.rip, HexU64(0x0010_0002));
            assert_eq!(
                observed.state.x87,
                Some(od_testkit::v2::X87Snapshot::from(&expected))
            );
            assert_eq!(observed.state.gpr, input.fixture.initial_state.gpr);
            assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);
        }
    }
}

#[test]
fn local_x87_state_profile_runs_exact_fsub_destinations_for_all_top_and_indices() {
    for top in 0u8..8 {
        for destination_index in 1u8..8 {
            let mut fixture = Fixture::from_json(MFENCE_STORE_FENCE_LOAD).unwrap().fixture;
            fixture.case_id = format!("local_x87_fsub_dest_top{top}_i{destination_index}");
            fixture.profile = od_testkit::v2::Profile::LongX87StateV1;
            fixture.initial_state.rip = HexU64(0x0010_0000);
            let source = usize::from(top);
            let destination = usize::from((top + destination_index) & 7);
            let mut seed = od_isa::state::X87State::fixture_seed();
            seed.control_word = 0x037f;
            seed.status_word = (u16::from(top) << 11) | 0x4700;
            seed.tag_word = u16::MAX & !(3 << (source * 2)) & !(3 << (destination * 2));
            seed.registers[source] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f]; // ST(0)=1.0
            seed.registers[destination] = [0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0x3f]; // ST(i)=1.5
            seed.data_pointer = 0x5678;
            seed.last_opcode = 0x05aa;
            fixture.initial_state.x87 = Some(od_testkit::v2::X87Snapshot::from(&seed));
            let code = fixture
                .memory
                .iter_mut()
                .find(|region| region.region_id == "code")
                .unwrap();
            code.guest_address = HexU64(0x0010_0000);
            code.length = 2;
            code.initial_bytes_hex = format!("dce{:x}", 8 + destination_index);
            code.comparison_mask_hex = "ffff".into();
            fixture.instruction_budget = 1;
            fixture.stop_rip = Some(HexU64(0x0010_0002));
            fixture.expected_exit = ExpectedExit::StopAddress;

            let (input, result) = run(&fixture.to_json().unwrap(), "interpreter", 0);
            assert_eq!(count(&result), 1, "TOP={top}, i={destination_index}");
            let observed = snapshot(&result);
            let mut expected = seed;
            expected.registers[destination] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xfe, 0x3f]; // 0.5
            expected.status_word &= !(1 << 9);
            expected.instruction_pointer = 0x0010_0000;
            assert_eq!(observed.state.rip, HexU64(0x0010_0002));
            assert_eq!(
                observed.state.x87,
                Some(od_testkit::v2::X87Snapshot::from(&expected))
            );
            assert_eq!(observed.state.gpr, input.fixture.initial_state.gpr);
            assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);
        }
    }
}

#[test]
fn fs_load_commit_is_preserved_when_a_later_instruction_faults_and_self_compares() {
    let mut fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_22j_fs_base_composition.case.json"
    ))
    .unwrap()
    .fixture;
    fixture.initial_state.gpr[0] = HexU64(0x0000_7fff_8000_000b);
    fixture.expected_exit = ExpectedExit::Exception;
    let case = fixture.to_json().unwrap();
    let (loaded, result) = run(&case, "interpreter", 0);
    assert_eq!(loaded.fixture.instruction_budget, 2);
    assert!(matches!(result.outcome, Outcome::Exception {}));
    assert_eq!(count(&result), 1);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, HexU64(0x0010_0002));
    assert_eq!(observed.state.fs_base, Some(HexU64(0x8000_0000)));
    assert_eq!(observed.state.fs_selector, Some(HexU64(0x0b)));
    assert_eq!(observed.state.fs_cache_access, Some(HexU64(0xf3)));
    assert_eq!(observed.state.fs_cache_limit, Some(HexU64(0)));
    assert_eq!(observed.state.fs_cache_flags, Some(HexU64(0)));

    let raw_result = cli(&case, "interpreter");
    let compare = compare_cli(&case, &raw_result.stdout);
    assert!(
        compare.status.success(),
        "{}{}",
        String::from_utf8_lossy(&compare.stdout),
        String::from_utf8_lossy(&compare.stderr)
    );
}

#[test]
fn qword_register_cmpxchg_fixtures_match_both_compare_branches() {
    for (bytes, case_id, expected_rax, expected_rcx, expected_flags, expected_code) in [
        (
            CMPXCHG_QWORD_REGISTER_EQUAL,
            "s2_21c_cmpxchg_qword_register_equal",
            0x1122_3344_5566_7788,
            0xaabb_ccdd_eeff_0011,
            0x0446,
            "480fb1d9",
        ),
        (
            CMPXCHG_QWORD_REGISTER_UNEQUAL_ALIAS,
            "s2_21c_cmpxchg_qword_register_unequal_alias",
            7,
            7,
            0x0493,
            "480fb1c9",
        ),
    ] {
        let (input, result) = run(bytes, "interpreter", 0);
        assert_eq!(input.fixture.case_id, case_id);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 1);

        let observed = snapshot(&result);
        let mut expected_gpr = input.fixture.initial_state.gpr;
        expected_gpr[0] = HexU64(expected_rax);
        expected_gpr[1] = HexU64(expected_rcx);
        assert_eq!(observed.state.gpr, expected_gpr, "{case_id}");
        assert_eq!(observed.state.rip, HexU64(0x0010_0004), "{case_id}");
        assert_eq!(observed.state.rflags, HexU64(expected_flags), "{case_id}");
        assert_eq!(observed.rflags_defined, HexU64(0x0cd7), "{case_id}");
        assert_eq!(observed.memory.len(), 1, "{case_id}");
        assert_eq!(observed.memory[0].region_id, "code", "{case_id}");
        assert_eq!(observed.memory[0].bytes_hex, expected_code, "{case_id}");
        assert_eq!(observed.exception, Observation::Available { value: None });
    }
}

#[test]
fn qword_memory_cmpxchg_fixtures_match_both_compare_branches_and_split_canaries() {
    let (equal_input, equal_result) = run(CMPXCHG_QWORD_MEMORY_EQUAL, "interpreter", 0);
    assert_eq!(
        equal_input.fixture.case_id,
        "s2_21e_cmpxchg_qword_memory_equal"
    );
    assert!(matches!(equal_result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&equal_result), 1);
    let equal = snapshot(&equal_result);
    assert_eq!(equal.state.gpr, equal_input.fixture.initial_state.gpr);
    assert_eq!(equal.state.rip, HexU64(0x0010_0004));
    assert_eq!(equal.state.rflags, HexU64(0x0046));
    assert_eq!(equal.rflags_defined, HexU64(0x0cd7));
    assert_eq!(equal.memory.len(), 3);
    assert_eq!(equal.memory[0].region_id, "code");
    assert_eq!(equal.memory[0].bytes_hex, "480fb118");
    assert_eq!(equal.memory[1].region_id, "data_left");
    assert_eq!(equal.memory[1].bytes_hex, "a5a5a511223344");
    assert_eq!(equal.memory[2].region_id, "data_right");
    assert_eq!(equal.memory[2].bytes_hex, "55667788a5a5a5a5");
    assert_eq!(equal.exception, Observation::Available { value: None });

    let (unequal_input, unequal_result) = run(CMPXCHG_QWORD_MEMORY_UNEQUAL_ALIAS, "interpreter", 0);
    assert_eq!(
        unequal_input.fixture.case_id,
        "s2_21e_cmpxchg_qword_memory_unequal_alias"
    );
    assert!(matches!(unequal_result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&unequal_result), 1);
    let unequal = snapshot(&unequal_result);
    let mut expected_gpr = unequal_input.fixture.initial_state.gpr;
    expected_gpr[0] = HexU64(0x2004);
    assert_eq!(unequal.state.gpr, expected_gpr);
    assert_eq!(unequal.state.rip, HexU64(0x0010_0004));
    assert_eq!(unequal.state.rflags, HexU64(0x0097));
    assert_eq!(unequal.rflags_defined, HexU64(0x0cd7));
    assert_eq!(unequal.memory.len(), 3);
    assert_eq!(unequal.memory[0].region_id, "code");
    assert_eq!(unequal.memory[0].bytes_hex, "480fb100");
    assert_eq!(unequal.memory[1].region_id, "data_left");
    assert_eq!(unequal.memory[1].bytes_hex, "a5a5a504200000");
    assert_eq!(unequal.memory[2].region_id, "data_right");
    assert_eq!(unequal.memory[2].bytes_hex, "00000000a5a5a5a5");
    assert_eq!(unequal.exception, Observation::Available { value: None });
}

#[test]
fn hand_authored_mov_fixtures_execute_with_full_snapshot_and_exact_input_digest() {
    for (bytes, retired) in [(LOAD_ADD_STORE, 3), (WIDTHS, 8), (REX_RIP, 5)] {
        let (input, result) = run(bytes, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), retired);
        let observed = snapshot(&result);
        assert_eq!(observed.state.rip, input.fixture.stop_rip.unwrap());
        assert_eq!(observed.rflags_defined, HexU64(0xcd7));
        assert_eq!(
            observed.memory[0].bytes_hex,
            input.fixture.memory[0].initial_bytes_hex
        );
        assert_eq!(observed.exception, Observation::Available { value: None });
        let mut expected_gpr = input.fixture.initial_state.gpr;
        if bytes == LOAD_ADD_STORE {
            expected_gpr[0] = HexU64(12);
            assert_eq!(observed.state.rflags, HexU64(0x406));
            assert_eq!(observed.memory[1].bytes_hex, "0c00");
            assert_eq!(observed.memory[2].bytes_hex, "000000000000");
        } else if bytes == WIDTHS {
            expected_gpr[0] = HexU64(0x8877_6655);
            assert_eq!(observed.state.rflags, HexU64(0x402));
            assert_eq!(
                observed.memory[1].bytes_hex,
                "11223344556677881122334455667788"
            );
        } else {
            expected_gpr[4] = HexU64(0x7034);
            expected_gpr[8] = HexU64(0x4433_2211);
            expected_gpr[9] = HexU64(0x4433_2211);
            expected_gpr[10] = HexU64(0x1122_3344_5566_777f);
            assert_eq!(observed.state.rflags, HexU64(0x402));
            assert_eq!(observed.memory[1].bytes_hex, "11223344");
            assert_eq!(observed.memory[2].bytes_hex, "0011223344");
        }
        assert_eq!(observed.state.gpr, expected_gpr);
    }
}

#[test]
fn setcc_cli_fixtures_match_literal_full_state_and_preserve_flags() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&fs::read(directory.join("s2_16b_setcc_expected.json")).unwrap())
            .unwrap();
    assert_eq!(answers.len(), 79);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let (input, result) = run(&raw, "interpreter", 0);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{case_id}"
        );
        assert_eq!(count(&result), 1, "{case_id}");
        let state = snapshot(&result);
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        assert_eq!(state.state.gpr, expected_gpr, "{case_id}");
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        assert_eq!(state.state.rflags, expected_flags, "{case_id}");
        assert_eq!(state.rflags_defined, HexU64(0xcd7), "{case_id}");
        assert_eq!(
            state.state.rip,
            input.fixture.stop_rip.unwrap(),
            "{case_id}"
        );
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in &state.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}: {}",
                region.region_id
            );
        }
    }
}

#[test]
fn rep_stosb_cli_fixtures_preserve_full_state_and_one_instruction_retirement() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &fs::read(directory.join("s2_19a_rep_stosb_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 5);
    for (case_id, answer) in answers {
        if answer.get("fault").is_some() {
            continue;
        }
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let (input, result) = run(&raw, "interpreter", 0);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{case_id}"
        );
        assert_eq!(count(&result), 1, "{case_id}");
        let observed = snapshot(&result);
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gpr, "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.rflags_defined, HexU64(0xcd7), "{case_id}");
        assert_eq!(
            observed.state.rip,
            input.fixture.stop_rip.unwrap(),
            "{case_id}"
        );
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn rep_iteration_work_limit_has_a_distinct_cli_outcome_and_restart_snapshot() {
    let mut fixture: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_19a_rep_stosb_df0.case.json"
    ))
    .unwrap();
    let iterations = 1025;
    fixture["initial_state"]["gpr"][1] = serde_json::json!(format!("0x{iterations:016x}"));
    fixture["initial_state"]["gpr"][7] = serde_json::json!("0x0000000000200000");
    fixture["memory"][1]["length"] = serde_json::json!(iterations);
    fixture["memory"][1]["initial_bytes_hex"] = serde_json::json!("00".repeat(iterations));
    fixture["memory"][1]["comparison_mask_hex"] = serde_json::json!("ff".repeat(iterations));
    fixture["expected_exit"] = serde_json::json!("rep_iteration_budget_exhausted");
    let raw = serde_json::to_vec(&fixture).unwrap();
    let (input, result) = run(&raw, "interpreter", 0);
    assert_eq!(result.outcome, Outcome::RepIterationBudgetExhausted {});
    assert_eq!(count(&result), 0);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, input.fixture.initial_state.rip);
    assert_eq!(observed.state.gpr[1], HexU64(1));
    assert_eq!(observed.state.gpr[7], HexU64(0x0020_0400));
    let data = observed
        .memory
        .iter()
        .find(|region| region.region_id == "data")
        .unwrap();
    assert_eq!(&data.bytes_hex[..1024 * 2], "88".repeat(1024));
    assert_eq!(&data.bytes_hex[1024 * 2..], "00");
}

#[test]
fn popfq_cli_fixtures_match_literal_flags_stack_pointer_and_memory() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&fs::read(directory.join("s2_17o_popfq_expected.json")).unwrap())
            .unwrap();
    assert_eq!(answers.len(), 2);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let (input, result) = run(&raw, "interpreter", 0);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{case_id}"
        );
        assert_eq!(count(&result), 1, "{case_id}");
        let observed = snapshot(&result);
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gpr, "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        assert_eq!(
            observed.state.rip,
            input.fixture.stop_rip.unwrap(),
            "{case_id}"
        );
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn pushfq_cli_fixture_matches_literal_masked_rflags_image() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&fs::read(directory.join("s2_17p_pushfq_expected.json")).unwrap())
            .unwrap();
    assert_eq!(answers.len(), 1);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let (input, result) = run(&raw, "interpreter", 0);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{case_id}"
        );
        assert_eq!(count(&result), 1, "{case_id}");
        let observed = snapshot(&result);
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gpr, "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        assert_eq!(
            observed.state.rip,
            input.fixture.stop_rip.unwrap(),
            "{case_id}"
        );
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn pushfw_cli_fixture_matches_literal_low_flags_word() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&fs::read(directory.join("s2_17q_pushfw_expected.json")).unwrap())
            .unwrap();
    assert_eq!(answers.len(), 1);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let (input, result) = run(&raw, "interpreter", 0);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{case_id}"
        );
        assert_eq!(count(&result), 1, "{case_id}");
        let observed = snapshot(&result);
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gpr, "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        assert_eq!(
            observed.state.rip,
            input.fixture.stop_rip.unwrap(),
            "{case_id}"
        );
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn push_immediate_cli_fixtures_match_literal_signed_boundary_answers() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &fs::read(directory.join("s2_17b_push_immediate_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 8);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let (input, result) = run(&raw, "interpreter", 0);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{case_id}"
        );
        assert_eq!(count(&result), 1, "{case_id}");
        let observed = snapshot(&result);
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gpr, "{case_id}");
        assert_eq!(
            observed.state.rip,
            input.fixture.stop_rip.unwrap(),
            "{case_id}"
        );
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, answer["memory"][&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn v2_jit_has_explicit_unsupported_result_without_interpreter_fallback() {
    let (_, result) = run(LOAD_ADD_STORE, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_explicitly_denies_bounded_fldcw_without_interpreter_fallback() {
    let (_, result) = run(FLDCW_FRNDINT_FNSTSW, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_loaded_infinity_frndint_without_interpreter_fallback() {
    let (_, result) = run(FRNDINT_INFINITY, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_sticky_oe_and_sf_ie_frndint_without_interpreter_fallback() {
    for fixture in [
        FRNDINT_SEEDED_OE_FNSTSW,
        FRNDINT_SEEDED_SF_IE_FNSTSW,
        FST_M32_OVERFLOW_FRNDINT_FNSTSW,
    ] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_m80_infinity_load_without_interpreter_fallback() {
    let (_, result) = run(FLD_M80_INFINITY, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_m80_special_loads_without_interpreter_fallback() {
    for fixture in [FLD_M80_QNAN, FLD_M80_SNAN, FLD_M80_PSEUDO_DENORMAL] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_m80_qnan_stores_without_interpreter_fallback() {
    for fixture in [FSTP_M80_QNAN, FSTP_M80_NEGATIVE_INDEFINITE_FNSTSW] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_m80_snan_stores_without_interpreter_fallback() {
    for fixture in [FSTP_M80_SNAN, FSTP_M80_NEGATIVE_SNAN_FNSTSW] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_m80_subnormal_stores_without_interpreter_fallback() {
    for fixture in [FSTP_M80_SUBNORMAL, FSTP_M80_NEGATIVE_SUBNORMAL_FNSTSW] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_m80_sticky_status_stores_without_interpreter_fallback() {
    for fixture in [
        FSTP_M80_STICKY_STATUS,
        FSTP_M80_NEGATIVE_SNAN_STICKY_FNSTSW,
        FSTP_M80_NEGATIVE_SNAN_STICKY_UE_FNSTSW,
        FSTP_M80_NEGATIVE_SNAN_STICKY_OE_FNSTSW,
    ] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_m32_subnormal_to_m64_stores_without_interpreter_fallback() {
    for fixture in [
        FLD_M32_SUBNORMAL_FST_M64,
        FLD_M32_NEGATIVE_LARGEST_SUBNORMAL_FSTP_M64_FNSTSW,
    ] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_m64_qnan_stores_without_interpreter_fallback() {
    for fixture in [FST_M64_QNAN, FSTP_M64_NEGATIVE_QNAN_FNSTSW] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_m64_snan_stores_without_interpreter_fallback() {
    for fixture in [
        FST_M64_SNAN,
        FSTP_M64_NEGATIVE_SNAN_FNSTSW,
        FSTP_M64_NEGATIVE_SNAN_STICKY_IE_FNSTSW,
    ] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_m32_qnan_stores_without_interpreter_fallback() {
    for fixture in [FST_M32_QNAN, FSTP_M32_NEGATIVE_QNAN_FNSTSW] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_m32_snan_stores_without_interpreter_fallback() {
    for fixture in [
        FST_M32_SNAN,
        FSTP_M32_NEGATIVE_SNAN_FNSTSW,
        FST_M32_SNAN_STICKY_IE,
    ] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_m32_max_finite_fld_store_fixtures_without_fallback() {
    for fixture in [
        FLD_M32_MAX_FINITE,
        FST_M32_MAX_FINITE,
        FSTP_M32_NEGATIVE_MAX_FINITE_FNSTSW,
    ] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_m64_max_finite_fld_store_fixtures_without_fallback() {
    for fixture in [
        FLD_M64_MAX_FINITE,
        FST_M64_MAX_FINITE,
        FSTP_M64_NEGATIVE_MAX_FINITE_FNSTSW,
    ] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_exact_m32_subnormal_store_fixtures_without_fallback() {
    for fixture in [
        FST_M32_POSITIVE_MIN_SUBNORMAL,
        FSTP_M32_NEGATIVE_MIN_SUBNORMAL_FNSTSW,
    ] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_masked_inexact_m32_subnormal_store_fixtures_without_fallback() {
    for fixture in [
        FST_M32_POSITIVE_INEXACT_SUBNORMAL_FNSTSW,
        FST_M32_EXTREME_UNDERFLOW_FNSTSW,
        FST_M32_BINARY80_SUBNORMAL_UNDERFLOW_FNSTSW,
        FSTP_M32_NEGATIVE_INEXACT_SUBNORMAL_FNSTSW,
    ] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_binary80_to_minimum_normal_m32_store_fixtures_without_fallback() {
    for fixture in [
        FST_M32_BINARY80_MIN_NORMAL_TIE_FNSTSW,
        FSTP_M32_BINARY80_MIN_NORMAL_DIRECTED_FNSTSW,
    ] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_binary80_to_minimum_normal_m64_store_fixtures_without_fallback() {
    for fixture in [
        FST_M64_BINARY80_MIN_NORMAL_TIE_FNSTSW,
        FSTP_M64_BINARY80_MIN_NORMAL_DIRECTED_FNSTSW,
    ] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_binary80_to_infinity_overflow_m32_fixtures_without_fallback() {
    for fixture in [
        FST_M32_BINARY80_INFINITY_OVERFLOW_FNSTSW,
        FSTP_M32_BINARY80_INFINITY_OVERFLOW_FNSTSW,
    ] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_binary80_m64_overflow_and_sticky_oe_fixtures_without_fallback() {
    for fixture in [
        FST_M64_BINARY80_INFINITY_OVERFLOW_FNSTSW,
        FSTP_M64_BINARY80_INFINITY_OVERFLOW_FNSTSW,
        M64_STICKY_OE_EXACT_STORE_FNSTSW,
    ] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_m32_store_with_preexisting_sticky_ue_without_fallback() {
    let (_, result) = run(FST_M32_STICKY_UE_FNSTSW, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_signed_zero_fist_m32_fixtures_without_interpreter_fallback() {
    for fixture in [
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31bt_fist_m32int_negative_zero_fnstsw.case.json"
        )
        .as_slice(),
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31bt_fistp_m32int_positive_zero_fnstsw.case.json"
        )
        .as_slice(),
    ] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_signed_zero_fistp_m16_fixture_without_interpreter_fallback() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31bu_fistp_m16int_negative_zero_fnstsw.case.json"
    );
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_signed_zero_fistp_m64_fixture_without_interpreter_fallback() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31bv_fistp_m64int_negative_zero_fnstsw.case.json"
    );
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_finite_overflow_fistp_m64_fixture_without_interpreter_fallback() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31f_fistp_m64int_positive_overflow.case.json"
    );
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_fractional_fistp_m64_fixture_without_interpreter_fallback() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31by_fistp_m64int_negative_fraction_fnstsw.case.json"
    );
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_fractional_fistp_m32_fixture_without_interpreter_fallback() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31bw_fistp_m32int_negative_fraction_fnstsw.case.json"
    );
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_canonical_infinity_fistp_m32_fixture_without_fallback() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31bz_fistp_m32int_negative_infinity_fnstsw.case.json"
    );
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_canonical_quiet_nan_fistp_m32_fixture_without_fallback() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31cc_fistp_m32int_negative_qnan_fnstsw.case.json"
    );
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_canonical_signaling_nan_fistp_m32_fixture_without_fallback() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31cf_fistp_m32int_negative_snan_fnstsw.case.json"
    );
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn local_x87_fistp_m32_subnormal_fixture_observes_pe_with_fnstsw() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31cq_fistp_m32int_negative_subnormal_fnstsw.case.json"
    );
    let (input, result) = run(fixture, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), input.fixture.instruction_budget);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, HexU64(0x0010_0004));
    assert_eq!(observed.state.gpr[0].get() & 0xffff, 0x4e20);
    let x87 = observed.state.x87.as_ref().unwrap();
    assert_eq!(x87.status_word, HexU64(0x4e20));
    assert_eq!(x87.tag_word, HexU64(0xffff));
    assert_eq!(x87.registers[0], "02000000000000000080");
    assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000));
    assert_eq!(x87.data_pointer, HexU64(0x0030_0001));
    let data_left = observed
        .memory
        .iter()
        .find(|region| region.region_id == "data_left")
        .unwrap();
    let data_right = observed
        .memory
        .iter()
        .find(|region| region.region_id == "data_right")
        .unwrap();
    assert_eq!(data_left.bytes_hex, "ff");
    assert_eq!(&data_right.bytes_hex[..6], "ffffff");
}

#[test]
fn v2_jit_denies_canonical_subnormal_fistp_m32_fixture_without_fallback() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31cq_fistp_m32int_negative_subnormal_fnstsw.case.json"
    );
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn local_x87_fistp_m16_subnormal_fixture_observes_pe_with_fnstsw() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31cr_fistp_m16int_negative_subnormal_fnstsw.case.json"
    );
    let (input, result) = run(fixture, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), input.fixture.instruction_budget);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, HexU64(0x0010_0004));
    assert_eq!(observed.state.gpr[0].get() & 0xffff, 0x4e20);
    let x87 = observed.state.x87.as_ref().unwrap();
    assert_eq!(x87.status_word, HexU64(0x4e20));
    assert_eq!(x87.tag_word, HexU64(0xffff));
    assert_eq!(x87.registers[0], "02000000000000000080");
    assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000));
    assert_eq!(x87.data_pointer, HexU64(0x0030_0001));
    let data_left = observed
        .memory
        .iter()
        .find(|region| region.region_id == "data_left")
        .unwrap();
    let data_right = observed
        .memory
        .iter()
        .find(|region| region.region_id == "data_right")
        .unwrap();
    assert_eq!(data_left.bytes_hex, "ff");
    assert_eq!(&data_right.bytes_hex[..2], "ff");
    assert_eq!(&data_right.bytes_hex[2..], "ccddeeff11223344556677889900");
}

#[test]
fn v2_jit_denies_canonical_subnormal_fistp_m16_fixture_without_fallback() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31cr_fistp_m16int_negative_subnormal_fnstsw.case.json"
    );
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_canonical_infinity_fistp_m16_fixture_without_fallback() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31ca_fistp_m16int_negative_infinity_fnstsw.case.json"
    );
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_canonical_quiet_nan_fistp_m16_fixture_without_fallback() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31cd_fistp_m16int_negative_qnan_fnstsw.case.json"
    );
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_canonical_signaling_nan_fistp_m16_fixture_without_fallback() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31cg_fistp_m16int_negative_snan_fnstsw.case.json"
    );
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_canonical_infinity_fistp_m64_fixture_without_fallback() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31cb_fistp_m64int_negative_infinity_fnstsw.case.json"
    );
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_canonical_quiet_nan_fistp_m64_fixture_without_fallback() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31ce_fistp_m64int_negative_qnan_fnstsw.case.json"
    );
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_canonical_signaling_nan_fistp_m64_fixture_without_fallback() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31ch_fistp_m64int_negative_snan_fnstsw.case.json"
    );
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_pseudo_nan_fistp_m64_fixture_without_fallback() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31cy_fistp_m64int_negative_pseudo_nan_fnstsw.case.json"
    );
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn local_x87_fistp_m64_pseudo_nan_fixture_observes_and_clears_ie() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31cy_fistp_m64int_negative_pseudo_nan_fnstsw.case.json"
    );
    let (input, result) = run(fixture, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), input.fixture.instruction_budget);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, HexU64(0x0010_0006));
    assert_eq!(observed.state.gpr[0].get() & 0xffff, 0x4901);
    let x87 = observed.state.x87.as_ref().unwrap();
    assert_eq!(x87.status_word, HexU64(0x4900));
    assert_eq!(x87.tag_word, HexU64(0xffff));
    assert_eq!(x87.registers[0], "0100000000000040ffff");
    assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000));
    assert_eq!(x87.data_pointer, HexU64(0x0030_0002));
    let data_left = observed
        .memory
        .iter()
        .find(|region| region.region_id == "data_left")
        .unwrap();
    let data_right = observed
        .memory
        .iter()
        .find(|region| region.region_id == "data_right")
        .unwrap();
    assert_eq!(data_left.bytes_hex, "aa");
    assert_eq!(data_right.bytes_hex, "000000000000008044556677889900");
}

#[test]
fn v2_jit_denies_fist_snan_fnstsw_sticky_ie_fixture_without_fallback() {
    let fixture =
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ci_fist_snan_fnstsw.case.json");
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_fistp_exact_incoming_ie_fixture_without_fallback() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31ci_fistp_exact_ie_fnstsw.case.json"
    );
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_m32_fistp_finite_overflow_fixture_without_fallback() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31cj_fistp_m32int_positive_overflow_fnstsw.case.json"
    );
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_m32_fist_roundback_fixture_without_fallback() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31cj_fist_m32int_positive_roundback_fnstsw.case.json"
    );
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_m16_fist_overflow_and_roundback_fixtures_without_fallback() {
    for fixture in [
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ck_fistp_m16int_positive_overflow_fnstsw.case.json").as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ck_fist_m16int_positive_roundback_fnstsw.case.json").as_slice(),
    ] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_fld_subnormal_to_integer_store_fixtures_without_fallback() {
    for fixture in [
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_31cm_fistp_m32int_loaded_max_subnormal_fnstsw.case.json").as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_31cm_fistp_m64int_loaded_max_subnormal_fnstsw.case.json").as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_31cn_fistp_m32int_sticky_ue_fnstsw.case.json").as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_31cn_fistp_m64int_sticky_ue_fnstsw.case.json").as_slice(),
    ] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_sticky_oe_and_ze_integer_store_fixtures_without_fallback() {
    for fixture in [
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31co_fistp_m32int_sticky_oe_fnstsw.case.json"
        )
        .as_slice(),
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31co_fistp_m64int_sticky_oe_fnstsw.case.json"
        )
        .as_slice(),
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cp_fistp_m32int_sticky_ze_fnstsw.case.json"
        )
        .as_slice(),
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cp_fistp_m64int_sticky_ze_fnstsw.case.json"
        )
        .as_slice(),
    ] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_fractional_fistp_m16_fixture_without_interpreter_fallback() {
    let fixture = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_31bx_fistp_m16int_negative_fraction_fnstsw.case.json"
    );
    let (_, result) = run(fixture, "jit", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
    assert!(matches!(
        result.instructions_executed,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn v2_jit_denies_exact_m64_subnormal_store_fixtures_without_fallback() {
    for fixture in [
        FST_M64_POSITIVE_MIN_SUBNORMAL,
        FSTP_M64_NEGATIVE_MIN_SUBNORMAL_FNSTSW,
    ] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_masked_inexact_m64_subnormal_store_fixtures_without_fallback() {
    for fixture in [
        FST_M64_POSITIVE_INEXACT_SUBNORMAL,
        FST_M64_EXTREME_UNDERFLOW_FNSTSW,
        FSTP_M64_NEGATIVE_INEXACT_SUBNORMAL_FNSTSW,
    ] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn v2_jit_denies_binary80_subnormal_m64_store_fixtures_without_fallback() {
    for fixture in [FST_M64_BINARY80_SUBNORMAL, FSTP_M64_BINARY80_SUBNORMAL] {
        let (_, result) = run(fixture, "jit", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(matches!(result.snapshot, Observation::Unavailable { .. }));
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn budget_exit_records_committed_load_before_pending_add_and_store() {
    let mut fixture = Fixture::from_json(LOAD_ADD_STORE).unwrap().fixture;
    fixture.instruction_budget = 1;
    fixture.expected_exit = ExpectedExit::InstructionBudgetExhausted;
    let (_, result) = run(&fixture.to_json().unwrap(), "interpreter", 0);
    assert!(matches!(
        result.outcome,
        Outcome::InstructionBudgetExhausted {}
    ));
    assert_eq!(count(&result), 1);
    let state = snapshot(&result);
    assert_eq!(state.state.rip, HexU64(0x4004));
    assert_eq!(state.state.gpr[0], HexU64(7));
    for (actual, initial) in state.memory.iter().zip(&fixture.memory) {
        assert_eq!(actual.bytes_hex, initial.initial_bytes_hex);
    }
}

#[test]
fn noncanonical_operand_is_guest_fault_but_unmapped_fixture_ram_is_harness_error() {
    for (base, expected_exit) in [
        (0x0000_7fff_ffff_fffc, 0),
        (0x9000, 5),
        // Endpoint overflow remains a diagnostic memory error until its
        // architectural behavior is independently established.
        (u64::MAX - 3, 5),
    ] {
        let mut fixture = Fixture::from_json(LOAD_ADD_STORE).unwrap().fixture;
        fixture.initial_state.gpr[3] = HexU64(base);
        fixture.expected_exit = ExpectedExit::Exception;
        let (_, result) = run(&fixture.to_json().unwrap(), "interpreter", expected_exit);
        let observed = snapshot(&result);
        assert_eq!(count(&result), 0);
        assert_eq!(observed.state, fixture.initial_state);
        for (actual, initial) in observed.memory.iter().zip(&fixture.memory) {
            assert_eq!(actual.bytes_hex, initial.initial_bytes_hex);
        }
        if expected_exit == 0 {
            assert!(matches!(result.outcome, Outcome::Exception {}));
            let Observation::Available { value: Some(fault) } = &observed.exception else {
                panic!("expected architectural #GP");
            };
            assert_eq!(fault.vector, 13);
            assert_eq!(
                fault.error_code,
                Observation::Available { value: HexU64(0) }
            );
            assert_eq!(fault.faulting_rip, fixture.initial_state.rip);
        } else {
            assert!(matches!(result.outcome, Outcome::HarnessError { .. }));
            assert_eq!(observed.exception, Observation::Available { value: None });
        }
    }
}

#[test]
fn unsupported_encoding_and_cooperative_deadline_remain_distinct_from_cpu_faults() {
    let mut fixture = Fixture::from_json(LOAD_ADD_STORE).unwrap().fixture;
    fixture.memory[0].length = 2;
    fixture.memory[0].initial_bytes_hex = "0f05".into();
    fixture.memory[0].comparison_mask_hex = "ffff".into();
    let (_, result) = run(&fixture.to_json().unwrap(), "interpreter", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    assert_eq!(snapshot(&result).state, fixture.initial_state);

    fixture.memory[0].length = 2;
    fixture.memory[0].initial_bytes_hex = "75fe".into();
    fixture.memory[0].comparison_mask_hex = "ffff".into();
    fixture.stop_rip = None;
    fixture.expected_exit = ExpectedExit::InstructionBudgetExhausted;
    fixture.instruction_budget = 10_000_000;
    fixture.wall_timeout_ms = 1;
    let (_, result) = run(&fixture.to_json().unwrap(), "interpreter", 5);
    assert!(matches!(result.outcome, Outcome::TimedOut { .. }));
    assert!(count(&result) < fixture.instruction_budget);
    assert_eq!(snapshot(&result).state, fixture.initial_state);
}

#[test]
fn fixture_schema_dispatch_rejects_unknown_versions_and_keeps_legacy_execution() {
    let legacy = cli(
        include_bytes!("../../../tests/fixtures/x86/s0_09_mov_imm64.case.json"),
        "interpreter",
    );
    assert!(legacy.status.success());
    assert_eq!(
        od_testkit::result::RunResult::from_json(&legacy.stdout)
            .unwrap()
            .schema_version,
        1
    );
    let mut malformed: serde_json::Value = serde_json::from_slice(LOAD_ADD_STORE).unwrap();
    for version in [
        serde_json::json!(3),
        serde_json::json!("2"),
        serde_json::Value::Null,
    ] {
        malformed["schema_version"] = version;
        let output = cli(&serde_json::to_vec(&malformed).unwrap(), "interpreter");
        assert_eq!(output.status.code(), Some(3));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn logic_cli_fixtures_emit_exact_observations_and_semantic_definedness() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let expected: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.join("s2_12_expected.json")).unwrap()).unwrap();
    let answers = expected.as_object().unwrap();
    assert_eq!(answers.len(), 197);
    for (case_id, answer) in answers {
        assert!(
            case_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        );
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let (input, result) = run(&raw, "interpreter", 0);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{case_id}"
        );
        assert_eq!(
            count(&result),
            input.fixture.instruction_budget,
            "{case_id}"
        );
        let observed = snapshot(&result);
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gpr, "{case_id}");
        assert_eq!(
            observed.state.rip,
            input.fixture.stop_rip.unwrap(),
            "{case_id}"
        );
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        assert_eq!(
            observed.state.rflags.get() & input.fixture.rflags_mask.get(),
            expected_flags.get() & input.fixture.rflags_mask.get(),
            "{case_id}"
        );
        assert_eq!(observed.exception, Observation::Available { value: None });
        for region in &observed.memory {
            let before = input
                .fixture
                .memory
                .iter()
                .find(|r| r.region_id == region.region_id)
                .unwrap();
            let expected_bytes = answer["memory"]
                .get(&region.region_id)
                .map(|v| v.as_str().unwrap())
                .unwrap_or(&before.initial_bytes_hex);
            assert_eq!(
                region.bytes_hex, expected_bytes,
                "{case_id}: {}",
                region.region_id
            );
        }
        assert!(
            od_testkit::v2::compare(&input, &result, &result)
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn direct_control_flow_cli_fixtures_match_literal_branch_targets_and_state() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let expected: serde_json::Value = serde_json::from_slice(
        &fs::read(directory.join("s2_16a_control_flow_expected.json")).unwrap(),
    )
    .unwrap();
    let answers = expected.as_object().unwrap();
    assert_eq!(answers.len(), 68);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let (input, result) = run(&raw, "interpreter", 0);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{case_id}: {:?}",
            result.outcome
        );
        assert_eq!(count(&result), 1, "{case_id}");
        let observed = snapshot(&result);
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gpr, "{case_id}");
        assert_eq!(
            observed.state.rip,
            input.fixture.stop_rip.unwrap(),
            "{case_id}"
        );
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, answer["memory"][&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn multiply_divide_cli_fixtures_match_literal_full_state_and_flag_definedness() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let expected: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.join("s2_14_expected.json")).unwrap()).unwrap();
    let answers = expected.as_object().unwrap();
    assert_eq!(answers.len(), 38);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let (input, result) = run(&raw, "interpreter", 0);
        assert_eq!(input.fixture.case_id, *case_id);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{case_id}"
        );
        assert_eq!(count(&result), 1, "{case_id}");
        let observed = snapshot(&result);
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gpr, "{case_id}");
        assert_eq!(
            observed.state.rip,
            input.fixture.stop_rip.unwrap(),
            "{case_id}"
        );
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        assert_eq!(
            observed.state.rflags.get() & input.fixture.rflags_mask.get(),
            expected_flags.get() & input.fixture.rflags_mask.get(),
            "{case_id}"
        );
        assert_eq!(
            observed.exception,
            Observation::Available { value: None },
            "{case_id}"
        );
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, answer["memory"][&region.region_id],
                "{case_id}: {}",
                region.region_id
            );
        }
        assert!(
            od_testkit::v2::compare(&input, &result, &result)
                .unwrap()
                .is_empty(),
            "{case_id}"
        );
    }
}

#[test]
fn divide_error_fixtures_preserve_state_and_report_exact_de_metadata() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &fs::read(directory.join("s2_20a_divide_errors_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 2);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let (input, result) = run(&raw, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::Exception {}), "{case_id}");
        assert_eq!(count(&result), 0, "{case_id}");
        let observed = snapshot(&result);
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gpr, "{case_id}");
        assert_eq!(
            observed.state.rip, input.fixture.initial_state.rip,
            "{case_id}"
        );
        assert_eq!(observed.state.rip, HexU64(0x0010_0000), "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}"
            );
        }
        let Observation::Available {
            value: Some(exception),
        } = &observed.exception
        else {
            panic!("{case_id}: expected #DE snapshot");
        };
        assert_eq!(exception.vector, answer["exception"]["vector"], "{case_id}");
        assert_eq!(
            answer["exception"]["error_code"], "unavailable",
            "{case_id}"
        );
        assert!(
            matches!(exception.error_code, Observation::Unavailable { .. }),
            "{case_id}"
        );
        let expected_faulting_rip: HexU64 =
            serde_json::from_value(answer["exception"]["faulting_rip"].clone()).unwrap();
        assert_eq!(exception.faulting_rip, expected_faulting_rip, "{case_id}");
        assert_eq!(
            exception.faulting_rip, input.fixture.initial_state.rip,
            "{case_id}"
        );
        assert_eq!(
            answer["exception"]["fault_address"], "unavailable",
            "{case_id}"
        );
        assert!(
            matches!(exception.fault_address, Observation::Unavailable { .. }),
            "{case_id}"
        );
        assert_eq!(answer["instructions_executed"], count(&result), "{case_id}");
    }
}

#[test]
fn complete_ud2_cli_fixture_reports_ud_without_state_commit() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let case_id = "s2_20b_ud2";
    let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
    let answer: serde_json::Value = serde_json::from_slice(
        &fs::read(directory.join(format!("{case_id}_expected.json"))).unwrap(),
    )
    .unwrap();
    let (input, result) = run(&raw, "interpreter", 0);
    assert_eq!(input.fixture.case_id, case_id);
    assert!(matches!(result.outcome, Outcome::Exception {}));
    assert_eq!(count(&result), answer["instructions_executed"]);
    let observed = snapshot(&result);
    let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
    let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
    let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
    let expected_defined: HexU64 =
        serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
    assert_eq!(observed.state.gpr, expected_gpr);
    assert_eq!(observed.state.rip, expected_rip);
    assert_eq!(observed.state.rip, input.fixture.initial_state.rip);
    assert_eq!(observed.state.rflags, expected_flags);
    assert_eq!(observed.rflags_defined, expected_defined);
    let expected_memory: std::collections::BTreeMap<String, String> =
        serde_json::from_value(answer["memory"].clone()).unwrap();
    for region in &observed.memory {
        assert_eq!(region.bytes_hex, expected_memory[&region.region_id]);
    }
    let Observation::Available {
        value: Some(exception),
    } = &observed.exception
    else {
        panic!("complete UD2 must report an architectural #UD snapshot");
    };
    assert_eq!(exception.vector, answer["exception"]["vector"]);
    assert_eq!(answer["exception"]["error_code"], "unavailable");
    assert!(matches!(
        exception.error_code,
        Observation::Unavailable { .. }
    ));
    assert_eq!(exception.faulting_rip, expected_rip);
    assert_eq!(answer["exception"]["fault_address"], "unavailable");
    assert!(matches!(
        exception.fault_address,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn qword_idiv_overflow_cli_fixture_reports_de_without_state_commit() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let case_id = "s2_20c_idiv_qword_quotient_overflow";
    let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
    let answer: serde_json::Value = serde_json::from_slice(
        &fs::read(directory.join(format!("{case_id}_expected.json"))).unwrap(),
    )
    .unwrap();
    let (input, result) = run(&raw, "interpreter", 0);
    assert_eq!(input.fixture.case_id, case_id);
    assert!(matches!(result.outcome, Outcome::Exception {}));
    assert_eq!(count(&result), answer["instructions_executed"]);
    let observed = snapshot(&result);
    let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
    let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
    let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
    let expected_defined: HexU64 =
        serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
    assert_eq!(observed.state.gpr, expected_gpr);
    assert_eq!(observed.state.rip, expected_rip);
    assert_eq!(observed.state.rip, input.fixture.initial_state.rip);
    assert_eq!(observed.state.rflags, expected_flags);
    assert_eq!(observed.rflags_defined, expected_defined);
    let expected_memory: std::collections::BTreeMap<String, String> =
        serde_json::from_value(answer["memory"].clone()).unwrap();
    for region in &observed.memory {
        assert_eq!(region.bytes_hex, expected_memory[&region.region_id]);
    }
    let Observation::Available {
        value: Some(exception),
    } = &observed.exception
    else {
        panic!("qword IDIV overflow must report an architectural #DE snapshot");
    };
    assert_eq!(exception.vector, answer["exception"]["vector"]);
    assert_eq!(answer["exception"]["error_code"], "unavailable");
    assert!(matches!(
        exception.error_code,
        Observation::Unavailable { .. }
    ));
    assert_eq!(exception.faulting_rip, expected_rip);
    assert_eq!(answer["exception"]["fault_address"], "unavailable");
    assert!(matches!(
        exception.fault_address,
        Observation::Unavailable { .. }
    ));
}

#[test]
fn sign_extension_cli_fixtures_match_literal_state_and_preserve_flag_definedness() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let expected: serde_json::Value = serde_json::from_slice(
        &fs::read(directory.join("s2_15_sign_extension_expected.json")).unwrap(),
    )
    .unwrap();
    let answers = expected.as_object().unwrap();
    assert_eq!(answers.len(), 6);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let (input, result) = run(&raw, "interpreter", 0);
        assert_eq!(input.fixture.case_id, *case_id);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{case_id}"
        );
        assert_eq!(count(&result), 1, "{case_id}");
        let observed = snapshot(&result);
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gpr, "{case_id}");
        assert_eq!(
            observed.state.rip,
            input.fixture.stop_rip.unwrap(),
            "{case_id}"
        );
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        assert_eq!(
            observed.exception,
            Observation::Available { value: None },
            "{case_id}"
        );
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, answer["memory"][&region.region_id],
                "{case_id}: {}",
                region.region_id
            );
        }
    }
}

#[test]
fn bswap_cli_fixtures_match_literal_state_and_preserve_flag_definedness() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let expected: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.join("s2_15_bswap_expected.json")).unwrap())
            .unwrap();
    let answers = expected.as_object().unwrap();
    assert_eq!(answers.len(), 7);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let (input, result) = run(&raw, "interpreter", 0);
        assert_eq!(input.fixture.case_id, *case_id);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{case_id}"
        );
        assert_eq!(count(&result), 1, "{case_id}");
        let observed = snapshot(&result);
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gpr, "{case_id}");
        assert_eq!(
            observed.state.rip,
            input.fixture.stop_rip.unwrap(),
            "{case_id}"
        );
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, answer["memory"][&region.region_id],
                "{case_id}"
            );
        }
        assert!(
            od_testkit::v2::compare(&input, &result, &result)
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn bt_family_cli_fixtures_match_literal_state_and_flag_definedness() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let expected: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.join("s2_15_bitops_expected.json")).unwrap())
            .unwrap();
    let answers = expected.as_object().unwrap();
    assert_eq!(answers.len(), 12);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let (input, result) = run(&raw, "interpreter", 0);
        assert_eq!(input.fixture.case_id, *case_id);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{case_id}"
        );
        assert_eq!(count(&result), 1, "{case_id}");
        let observed = snapshot(&result);
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gpr, "{case_id}");
        assert_eq!(
            observed.state.rip,
            input.fixture.stop_rip.unwrap(),
            "{case_id}"
        );
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        assert_eq!(
            observed.exception,
            Observation::Available { value: None },
            "{case_id}"
        );
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, answer["memory"][&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn bt_family_immediate_cli_fixtures_match_literal_state_and_flag_definedness() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let expected: serde_json::Value = serde_json::from_slice(
        &fs::read(directory.join("s2_15_bitops_immediate_expected.json")).unwrap(),
    )
    .unwrap();
    let answers = expected.as_object().unwrap();
    assert_eq!(answers.len(), 48);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let (input, result) = run(&raw, "interpreter", 0);
        assert_eq!(input.fixture.case_id, *case_id);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{case_id}: {:?}",
            result.outcome
        );
        assert_eq!(count(&result), 1, "{case_id}");
        let observed = snapshot(&result);
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gpr, "{case_id}");
        assert_eq!(
            observed.state.rip,
            input.fixture.stop_rip.unwrap(),
            "{case_id}"
        );
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        assert_eq!(
            observed.exception,
            Observation::Available { value: None },
            "{case_id}"
        );
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, answer["memory"][&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn bt_family_immediate_memory_cli_fixtures_match_literal_state_and_flag_definedness() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let expected: serde_json::Value = serde_json::from_slice(
        &fs::read(directory.join("s2_15_bitops_immediate_memory_expected.json")).unwrap(),
    )
    .unwrap();
    let answers = expected.as_object().unwrap();
    assert_eq!(answers.len(), 48);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let (input, result) = run(&raw, "interpreter", 0);
        assert_eq!(input.fixture.case_id, *case_id);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{case_id}: {:?}",
            result.outcome
        );
        assert_eq!(count(&result), 1, "{case_id}");
        let observed = snapshot(&result);
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gpr, "{case_id}");
        assert_eq!(
            observed.state.rip,
            input.fixture.stop_rip.unwrap(),
            "{case_id}"
        );
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        assert_eq!(
            observed.exception,
            Observation::Available { value: None },
            "{case_id}"
        );
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, answer["memory"][&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn bt_family_register_memory_cli_fixtures_match_literal_state_and_flag_definedness() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let expected: serde_json::Value = serde_json::from_slice(
        &fs::read(directory.join("s2_15_bitops_register_memory_expected.json")).unwrap(),
    )
    .unwrap();
    let answers = expected.as_object().unwrap();
    assert_eq!(answers.len(), 72);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let (input, result) = run(&raw, "interpreter", 0);
        assert_eq!(input.fixture.case_id, *case_id);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{case_id}: {:?}",
            result.outcome
        );
        assert_eq!(count(&result), 1, "{case_id}");
        let observed = snapshot(&result);
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gpr, "{case_id}");
        assert_eq!(
            observed.state.rip,
            input.fixture.stop_rip.unwrap(),
            "{case_id}"
        );
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        assert_eq!(
            observed.exception,
            Observation::Available { value: None },
            "{case_id}"
        );
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, answer["memory"][&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn bsf_bsr_cli_fixtures_match_literal_state_and_flag_definedness() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let expected: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.join("s2_15_scans_expected.json")).unwrap())
            .unwrap();
    let answers = expected.as_object().unwrap();
    assert_eq!(answers.len(), 23);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let (input, result) = run(&raw, "interpreter", 0);
        assert_eq!(input.fixture.case_id, *case_id);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{case_id}"
        );
        assert_eq!(count(&result), 1, "{case_id}");
        let observed = snapshot(&result);
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gpr, "{case_id}");
        assert_eq!(
            observed.state.rip,
            input.fixture.stop_rip.unwrap(),
            "{case_id}"
        );
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        assert_eq!(
            observed.exception,
            Observation::Available { value: None },
            "{case_id}"
        );
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, answer["memory"][&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn faulting_logic_cli_does_not_commit_undefined_flags_effect() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let raw = fs::read(directory.join("s2_12_and_64_mem_reg.case.json")).unwrap();
    let mut fixture = Fixture::from_json(&raw).unwrap().fixture;
    fixture.initial_state.gpr[3] = HexU64(0x0000_8000_0000_0000);
    fixture.expected_exit = ExpectedExit::Exception;
    let (_, result) = run(&fixture.to_json().unwrap(), "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::Exception {}));
    assert_eq!(count(&result), 0);
    let observed = snapshot(&result);
    assert_eq!(observed.state, fixture.initial_state);
    assert_eq!(observed.rflags_defined, HexU64(0xcd7));
    for (actual, initial) in observed.memory.iter().zip(&fixture.memory) {
        assert_eq!(actual.bytes_hex, initial.initial_bytes_hex);
    }
}
