//! Real CLI/interpreter checks; expected values are hand-calculated from Intel SDM 093.
use od_testkit::HexU64;
use od_testkit::result::{MemoryRegionSnapshot, Observation};
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
const FINCSTP_FIXTURE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bm_fincstp_fnstsw.case.json");
const FDECSTP_FIXTURE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bm_fdecstp_fnstsw.case.json");
const FINCSTP_FDECSTP_FIXTURE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bm_fincstp_fdecstp_fnstsw.case.json");
const FDECSTP_FINCSTP_FIXTURE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bm_fdecstp_fincstp_fnstsw.case.json");
const FLD_ST0_NEGATIVE_ZERO_WRAP: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30bo_fld_st0_negative_zero_wrap.case.json"
);
const FLD_ST0_FNSTSW_WRAP: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bo_fld_st0_fnstsw_wrap.case.json");
const FLD1_FLD_ST0_FNSTSW: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bo_fld1_fld_st0_fnstsw.case.json");
const FLDZ_FLD_ST0_FNSTSW: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bo_fldz_fld_st0_fnstsw.case.json");
const FLD_ST0_EMPTY_SOURCE_REJECTS: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30bo_fld_st0_empty_source_rejects.case.json"
);
const FLD_ST0_OCCUPIED_DESTINATION_REJECTS: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30bo_fld_st0_occupied_destination_rejects.case.json"
);
const FLD_STI_FIXTURES: [&[u8]; 6] = [
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bp_fld_st1_normal.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bp_fld_st2_normal.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bp_fld_st3_normal.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bp_fld_st4_normal.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bp_fld_st5_normal.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bp_fld_st6_normal.case.json"),
];
const FLD_STI_SIGNED_ZERO_WRAP: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30bp_fld_st1_negative_zero_top_wrap.case.json"
);
const FLD_STI_REJECTS: [&[u8]; 2] = [
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bp_fld_st6_wrong_source_tag.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30bp_fld_st6_occupied_destination.case.json"
    ),
];
const FCMOVB_FIXTURES: [&[u8]; 14] = [
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30bu_fcmovb_taken_negative_zero.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30bu_fcmovb_untaken_distinct_tags.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bu_fcmovb_top_wrap.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bu_fcmovb_repeat.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30bu_fcmovb_producer_minus1_vs_plus1.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30bu_fcmovb_producer_plus1_vs_minus1.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bv_fcmovb_st0_top7_fnstsw.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bv_fcmovb_st1_top7_fnstsw.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bv_fcmovb_st2_top7_fnstsw.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bv_fcmovb_st3_top7_fnstsw.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bv_fcmovb_st4_top7_fnstsw.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bv_fcmovb_st5_top7_fnstsw.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bv_fcmovb_st6_top7_fnstsw.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bv_fcmovb_st7_top7_fnstsw.case.json"),
];
const FCMOVB_REJECTS: [&[u8]; 7] = [
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30bu_fcmovb_invalid_source_cf0.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30bu_fcmovb_invalid_destination.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bu_fcmovb_c1_reject.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bu_fcmovb_bad_fcw_reject.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30bv_fcmovb_st6_invalid_source_cf0.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bv_fcmovb_st7_c1_reject.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30bv_fcmovb_st0_bad_fcw_reject.case.json"
    ),
];
const FCMOVNB_FIXTURES: [&[u8]; 14] = [
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30bw_fcmovnb_taken_negative_zero.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30bw_fcmovnb_untaken_distinct_tags.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bw_fcmovnb_top7_wrap.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bw_fcmovnb_repeat.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30bw_fcmovnb_producer_minus1_below_plus1.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30bw_fcmovnb_producer_plus1_above_minus1.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bx_fcmovnb_st0_self_cf1.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bx_fcmovnb_st0_self_cf0.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bx_fcmovnb_st2_signed_zero.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bx_fcmovnb_st3_signed_zero.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bx_fcmovnb_st4_signed_zero.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bx_fcmovnb_st5_signed_zero.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bx_fcmovnb_st6_signed_zero.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bx_fcmovnb_st7_signed_zero.case.json"),
];
const FCMOVNB_REJECTS: [&[u8]; 4] = [
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30bw_fcmovnb_invalid_source_cf1.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30bw_fcmovnb_invalid_destination.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bw_fcmovnb_c1_reject.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bw_fcmovnb_bad_fcw_reject.case.json"),
];
const FCMOVNE_FIXTURES: [&[u8]; 13] = [
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30ca_fcmovne_taken_negative_zero.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30ca_fcmovne_untaken_distinct_tags.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cb_fcmovne_index_0_0.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cb_fcmovne_index_0_1.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cb_fcmovne_index_2_2.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cb_fcmovne_index_3_3.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cb_fcmovne_index_4_4.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cb_fcmovne_index_5_5.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cb_fcmovne_index_6_6.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cb_fcmovne_index_7_7.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cb_fcmovne_repeat.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cb_fcmovne_producer_equal_then_fchs.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cb_fcmovne_producer_unequal_after_fchs.case.json"
    ),
];
const FCMOVNE_REJECTS: [&[u8]; 3] = [
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30ca_fcmovne_invalid_source_zf1_reject.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cb_fcmovne_st7_invalid_source_zf1_reject.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cb_fcmovne_st7_bad_environment_reject.case.json"
    ),
];
const FCMOVBE_FIXTURES: [&[u8]; 26] = [
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cc_fcmovbe_untaken.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cc_fcmovbe_cf_negative_zero.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cc_fcmovbe_zf_wrap_fnstsw.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cc_fcmovbe_cf_zf_fnstsw.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cc_fcmovbe_repeat.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cc_fcmovbe_producer_greater.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cc_fcmovbe_producer_equal_then_fchs.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cc_fcmovbe_producer_less.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st0_indexed.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st0_cf.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st0_zf.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st0_cf_zf.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st2_indexed.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st3_indexed.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st4_indexed.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st5_indexed.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st6_indexed.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st7_indexed.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st0_fnstsw.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st1_fnstsw.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st2_fnstsw.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st3_fnstsw.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st4_fnstsw.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st5_fnstsw.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st6_fnstsw.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st7_fnstsw.case.json"),
];
const FCMOVBE_REJECTS: [&[u8]; 6] = [
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cc_fcmovbe_invalid_source_both_flags.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cc_fcmovbe_bad_environment.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st0_invalid_untaken.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st7_invalid_untaken.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st2_invalid_taken.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cd_fcmovbe_st6_bad_environment.case.json"
    ),
];
const FCMOVNBE_FIXTURES: [&[u8]; 30] = [
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st0_cf0_zf0.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st0_cf0_zf0_fnstsw.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st0_cf1_zf0.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st0_cf1_zf0_fnstsw.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st0_cf0_zf1.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st0_cf0_zf1_fnstsw.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st0_cf1_zf1.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st0_cf1_zf1_fnstsw.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st2_cf1_zf1.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st2_cf1_zf1_fnstsw.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st3_cf0_zf0.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st3_cf0_zf0_fnstsw.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st4_cf1_zf0.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st4_cf1_zf0_fnstsw.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st5_cf0_zf1.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st5_cf0_zf1_fnstsw.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st6_cf1_zf1.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st6_cf1_zf1_fnstsw.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st7_cf0_zf0.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st7_cf0_zf0_fnstsw.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st2_taken_normal_to_zero.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st5_taken_zero_to_normal.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30ce_fcmovnbe_taken_negative_zero.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ce_fcmovnbe_cf_untaken.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ce_fcmovnbe_zf_untaken.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ce_fcmovnbe_cf_zf_untaken.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ce_fcmovnbe_repeat.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30ce_fcmovnbe_producer_greater.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30ce_fcmovnbe_producer_equal_then_fchs.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ce_fcmovnbe_producer_less.case.json"),
];
const FCMOVNBE_REJECTS: [&[u8]; 5] = [
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30ce_fcmovnbe_invalid_source_untaken.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ce_fcmovnbe_bad_environment.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st3_invalid_taken.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st3_invalid_untaken.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cf_fcmovnbe_st6_bad_environment.case.json"
    ),
];
const FCMOVU_FIXTURES: [&[u8]; 2] = [
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cg_fcmovu_test_pf1_taken.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cg_fcmovu_test_pf0_untaken.case.json"),
];
const FCMOVU_INDEXED_FIXTURES: [&[u8]; 10] = [
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ch_fcmovu_st0_pf0.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ch_fcmovu_st0_pf1.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ch_fcmovu_i1.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ch_fcmovu_i2.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ch_fcmovu_zero.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ch_fcmovu_i4.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ch_fcmovu_i5.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ch_fcmovu_i6.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ch_fcmovu_i7.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ch_fcmovu_repeat.case.json"),
];
const FCMOVU_REJECTS: [&[u8]; 7] = [
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30ch_fcmovu_invalid_source_pf0.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30ch_fcmovu_invalid_source_pf1.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30ch_fcmovu_invalid_destination.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ch_fcmovu_c1_reject.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ch_fcmovu_es_reject.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ch_fcmovu_b_bit_reject.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ch_fcmovu_bad_fcw_reject.case.json"),
];
const FCMOVNU_INDEXED_FIXTURES: [&[u8]; 9] = [
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_i1.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_i7.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_self_pf0.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_self_pf1.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_wrap_top7_i1.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_zero_to_normal_i2.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ci_fcmovnu_repeat.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ci_fcmovnu_test_pf0_taken.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ci_fcmovnu_test_pf1_untaken.case.json"),
];
const FCMOVNU_REJECTS: [&[u8]; 7] = [
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_invalid_source_pf0.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30ci_fcmovnu_invalid_source_pf1.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_invalid_destination.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_c1.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_es.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_b.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_fcw.case.json"),
];
const FCMOVE_INDEXED_FIXTURES: [&[u8]; 8] = [
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bz_fcmove_st0_zf1.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bz_fcmove_st0_zf0.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bz_fcmove_st2_top6.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bz_fcmove_st3_top7.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bz_fcmove_st4_top0.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bz_fcmove_st5_top1.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bz_fcmove_st6_top2.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bz_fcmove_st7_top3.case.json"),
];
const FCMOVE_FIXTURES: [&[u8]; 14] = [
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30by_fcmove_taken_negative_zero.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30by_fcmove_untaken_distinct_tags.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30by_fcmove_top7_wrap.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30by_fcmove_repeat.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30by_fcmove_producer_equal_then_fchs.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30by_fcmove_producer_unequal_after_fchs.case.json"
    ),
    FCMOVE_INDEXED_FIXTURES[0],
    FCMOVE_INDEXED_FIXTURES[1],
    FCMOVE_INDEXED_FIXTURES[2],
    FCMOVE_INDEXED_FIXTURES[3],
    FCMOVE_INDEXED_FIXTURES[4],
    FCMOVE_INDEXED_FIXTURES[5],
    FCMOVE_INDEXED_FIXTURES[6],
    FCMOVE_INDEXED_FIXTURES[7],
];
const FCMOVE_REJECTS: [&[u8]; 7] = [
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30by_fcmove_invalid_source_zf0_reject.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30by_fcmove_invalid_destination_reject.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30by_fcmove_c1_reject.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30by_fcmove_bad_fcw_reject.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30by_fcmove_es_b_reject.case.json"),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30bz_fcmove_st7_invalid_source_zf0_reject.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30bz_fcmove_st7_bad_environment_reject.case.json"
    ),
];
const FFREE_ST0_ALREADY_EMPTY: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bq_ffree_already_empty.case.json");
const FFREE_ST0_SNAN_OPAQUE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bq_ffree_snan_opaque.case.json");
const FFREE_ST0_MALFORMED_OPAQUE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bq_ffree_malformed_opaque.case.json");
const FFREE_ST0_FINCP_FLD1: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bq_ffree_fincstp_fld1.case.json");
const FFREE_ST0_FLD_ST0_REJECT: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30bq_ffree_fld_st0_empty_reject.case.json"
);
const FFREE_ST0_ENV_REJECT: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bq_ffree_environment_reject.case.json");
const FFREE_STI_FIXTURES: [&[u8]; 7] = [
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30br_ffree_st1_opaque.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30br_ffree_st2_opaque.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30br_ffree_st3_opaque.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30br_ffree_st4_opaque.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30br_ffree_st5_opaque.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30br_ffree_st6_opaque.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30br_ffree_st7_fld1_reuse.case.json"),
];
const FNOP_FIXTURES: [&[u8]; 4] = [
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bs_fnop_top7_c1_opaque.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bs_fnop_all_empty_fnstsw.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bs_repeated_fnop.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bs_ffree_fnop_fld1.case.json"),
];
const FNOP_REJECT_FIXTURES: [&[u8]; 2] = [
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bs_fnop_forbidden_es_b.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bs_fnop_invalid_fcw.case.json"),
];
const FXAM_FIXTURES: [&[u8]; 7] = [
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30bt_fxam_negative_normal_wrap.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30bt_fxam_negative_normal_fnstsw.case.json"
    ),
    include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30bt_fxam_repeat_negative_normal.case.json"
    ),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bt_fxam_fld1_producer.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bt_fxam_fldz_producer.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bt_fxam_fld1_fchs_producer.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bt_fxam_fldz_fchs_producer.case.json"),
];
const FXAM_REJECT_FIXTURES: [&[u8]; 5] = [
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bt_fxam_wrong_tag_reject.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bt_fxam_empty_reject.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bt_fxam_special_reject.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bt_fxam_es_b_reject.case.json"),
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bt_fxam_bad_fcw_reject.case.json"),
];
const FFREE_STI_ENV_REJECT: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30br_ffree_st7_environment_reject.case.json"
);
const FCHS_SIGNED_ZERO: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ao_fchs_signed_zero.case.json");
const FCHS_ZERO_WRONG_TAG: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ao_fchs_zero_wrong_tag.case.json");
const FCHS_ZERO_COMPOSITION: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ao_fld_fchs_fst_fnstsw.case.json");
const FABS_ZERO_COMPOSITION: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ap_fld_fabs_fst_fnstsw.case.json");
const FCHS_INFINITY_COMPOSITION: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30av_fld_fchs_fst_fnstsw.case.json");
const FABS_INFINITY_COMPOSITION: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30av_fld_fabs_fst_fnstsw.case.json");
const FCHS_INFINITY_WRONG_TAG: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30av_fchs_infinity_wrong_tag.case.json");
const FCHS_QNAN_WRONG_TAG: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30aw_fchs_qnan_wrong_tag.case.json");
const FCHS_FABS_QNAN_FIXTURES: [(&[u8], bool, bool); 4] = [
    (
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30aw_fld_m32_fchs_fst_fnstsw_positive_qnan.case.json"
        ),
        true,
        false,
    ),
    (
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30aw_fld_m32_fchs_fst_fnstsw_negative_qnan.case.json"
        ),
        true,
        true,
    ),
    (
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30aw_fld_m32_fabs_fst_fnstsw_positive_qnan.case.json"
        ),
        false,
        false,
    ),
    (
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30aw_fld_m32_fabs_fst_fnstsw_negative_qnan.case.json"
        ),
        false,
        true,
    ),
];
const FCHS_FABS_SNAN_FIXTURES: [(&[u8], bool, bool); 4] = [
    (
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ax_fchs_positive_snan.case.json"),
        true,
        false,
    ),
    (
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ax_fchs_negative_snan.case.json"),
        true,
        true,
    ),
    (
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ax_fabs_positive_snan.case.json"),
        false,
        false,
    ),
    (
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ax_fabs_negative_snan.case.json"),
        false,
        true,
    ),
];
const FCHS_SNAN_WRONG_TAG: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ax_fchs_snan_wrong_tag.case.json");
const FCHS_FABS_SUBNORMAL_FIXTURES: [(&[u8], bool, bool); 4] = [
    (
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30ay_fchs_positive_subnormal.case.json"
        ),
        true,
        false,
    ),
    (
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30ay_fchs_negative_subnormal.case.json"
        ),
        true,
        true,
    ),
    (
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30ay_fabs_positive_subnormal.case.json"
        ),
        false,
        false,
    ),
    (
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30ay_fabs_negative_subnormal.case.json"
        ),
        false,
        true,
    ),
];
const FCHS_SUBNORMAL_WRONG_TAG: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ay_fchs_subnormal_wrong_tag.case.json");
const FCHS_FABS_SNAN_PRODUCER_FIXTURES: [(&[u8], bool, bool); 4] = [
    (
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30bc_fld_m32_fchs_fst_fnstsw_positive_snan.case.json"
        ),
        true,
        false,
    ),
    (
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30bc_fld_m32_fchs_fst_fnstsw_negative_snan.case.json"
        ),
        true,
        true,
    ),
    (
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30bc_fld_m32_fabs_fst_fnstsw_positive_snan.case.json"
        ),
        false,
        false,
    ),
    (
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30bc_fld_m32_fabs_fst_fnstsw_negative_snan.case.json"
        ),
        false,
        true,
    ),
];
const FCHS_FABS_SUBNORMAL_PRODUCER_FIXTURES: [(&[u8], bool, bool); 4] = [
    (
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30bd_fld_m32_fchs_fst_fnstsw_positive_subnormal.case.json"
        ),
        true,
        false,
    ),
    (
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30bd_fld_m32_fchs_fst_fnstsw_negative_subnormal.case.json"
        ),
        true,
        true,
    ),
    (
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30bd_fld_m32_fabs_fst_fnstsw_positive_subnormal.case.json"
        ),
        false,
        false,
    ),
    (
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30bd_fld_m32_fabs_fst_fnstsw_negative_subnormal.case.json"
        ),
        false,
        true,
    ),
];
const FCHS_DE_ONLY: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bd_fchs_normal_de_only.case.json");
const FCHS_DE_SF: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bd_fchs_normal_de_sf.case.json");
const FCHS_IE_DE_ES: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bd_fchs_normal_ie_de_es.case.json");
const FCHS_IE_ONLY: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bc_fchs_normal_ie_only.case.json");
const FCHS_IE_SF: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bi_fchs_normal_ie_sf_basic.case.json");
const FABS_IE_SF: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bi_fabs_normal_ie_sf_basic.case.json");
const FCHS_PE_ONLY: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30be_fchs_normal_pe_only.case.json");
const FCHS_IE_DE_PE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30be_fchs_normal_ie_de_pe.case.json");
const FCHS_PE_SF: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30be_fchs_normal_pe_sf.case.json");
const FCHS_IE_DE_PE_ES: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30be_fchs_normal_ie_de_pe_es.case.json");
const UE_FST_M32_FCHS: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bf_fst_m32_fchs_ue_only.case.json");
const UE_FST_M64_FABS: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bf_fst_m64_fabs_ie_de_ue_pe.case.json");
const UE_ES_FCHS: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bf_fchs_normal_ue_es.case.json");
const UE_HISTORY_ES_FCHS: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30bf_fchs_normal_ie_de_ue_pe_es.case.json"
);
const FRNDINT_FCHS_PE: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30bi_frndint_fchs_positive_tie_fnstsw.case.json"
);
const FRNDINT_FABS_PE: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30bi_frndint_fabs_negative_tie_fnstsw.case.json"
);
const FCHS_OE_ONLY: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bg_fchs_normal_oe_only.case.json");
const FCHS_ZE_ONLY: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bh_fchs_normal_ze_only.case.json");
const FABS_ALL_FLAGS: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bh_fabs_normal_all_flags.case.json");
const FCHS_ZE_SF: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bh_fchs_normal_ze_sf.case.json");
const FABS_ALL_FLAGS_ES: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bh_fabs_normal_all_flags_es.case.json");
const OE_FST_M32_FCHS: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30bg_fst_m32_fchs_overflow_midpoint.case.json"
);
const OE_FST_M64_FABS: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30bg_fst_m64_fabs_overflow_midpoint.case.json"
);
const FTST_ZERO_COMPOSITION: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30aq_fld_ftst_fnstsw.case.json");
const FST_STI_ZERO_COMPOSITION: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ar_fld_fst_sti_fnstsw.case.json");
const FSTP_STI_ZERO_COMPOSITION: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30as_fld_fstp_sti_fnstsw.case.json");
const FST_STI_INFINITY_POSITIVE: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30au_fld_fst_sti_positive_infinity_fnstsw.case.json"
);
const FST_STI_INFINITY_NEGATIVE: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30au_fld_fst_sti_negative_infinity_fnstsw.case.json"
);
const FSTP_STI_INFINITY_POSITIVE: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30au_fld_fstp_sti_positive_infinity_fnstsw.case.json"
);
const FSTP_STI_INFINITY_NEGATIVE: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30au_fld_fstp_sti_negative_infinity_fnstsw.case.json"
);
const FST_STI_QNAN_POSITIVE: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30az_fld_m32_fst_sti_positive_qnan_fnstsw.case.json"
);
const FST_STI_QNAN_NEGATIVE: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30az_fld_m32_fst_sti_negative_qnan_fnstsw.case.json"
);
const FSTP_STI_QNAN_POSITIVE: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30az_fld_m32_fstp_sti_positive_qnan_fnstsw.case.json"
);
const FSTP_STI_QNAN_NEGATIVE: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30az_fld_m32_fstp_sti_negative_qnan_fnstsw.case.json"
);
const FST_STI_SNAN_POSITIVE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ba_fst_sti_positive_snan.case.json");
const FST_STI_SNAN_NEGATIVE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ba_fst_sti_negative_snan.case.json");
const FSTP_STI_SNAN_POSITIVE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ba_fstp_sti_positive_snan.case.json");
const FSTP_STI_SNAN_NEGATIVE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ba_fstp_sti_negative_snan.case.json");
const FST_STI_SNAN_WRONG_TAG: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ba_fst_sti_snan_wrong_tag.case.json");
const FST_STI_SUBNORMAL_POSITIVE: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30bb_fst_sti_positive_subnormal.case.json"
);
const FST_STI_SUBNORMAL_NEGATIVE: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30bb_fst_sti_negative_subnormal.case.json"
);
const FSTP_STI_SUBNORMAL_POSITIVE: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30bb_fstp_sti_positive_subnormal.case.json"
);
const FSTP_STI_SUBNORMAL_NEGATIVE: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30bb_fstp_sti_negative_subnormal.case.json"
);
const FST_STI_SUBNORMAL_WRONG_TAG: &[u8] = include_bytes!(
    "../../../tests/fixtures/x86/stage2/s2_30bb_fst_sti_subnormal_wrong_tag.case.json"
);
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
        "case {}: {}",
        loaded.fixture.case_id,
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
fn local_x87_fchs_signed_zero_fixture_cli_preserves_zero_tag_and_payload() {
    let (input, result) = run(FCHS_SIGNED_ZERO, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 1);
    let observed = snapshot(&result);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0002);
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.registers[0] = "00000000000000000080".into();
    expected_x87.status_word = HexU64(0x4500);
    expected_x87.instruction_pointer = input.fixture.initial_state.rip;
    assert_eq!(observed.state, expected);
}

#[test]
fn local_x87_fchs_zero_wrong_tag_fixture_is_atomic_in_the_cli_runner() {
    let (input, result) = run(FCHS_ZERO_WRONG_TAG, "interpreter", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    let observed = snapshot(&result);
    assert_eq!(observed.state.rip, input.fixture.initial_state.rip);
    assert_eq!(observed.state.gpr, input.fixture.initial_state.gpr);
    assert_eq!(observed.state.rflags, input.fixture.initial_state.rflags);
    assert_eq!(observed.state.x87, input.fixture.initial_state.x87);
}

#[test]
fn local_x87_fld_fchs_fst_fnstsw_composition_fixture_checks_state_and_memory() {
    let (input, result) = run(FCHS_ZERO_COMPOSITION, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 4);
    let observed = snapshot(&result);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0008);
    expected.gpr[0] = HexU64(0x0000_0000_0030_3800);
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.registers[7] = "00000000000000000000".into();
    expected_x87.status_word = HexU64(0x3800);
    expected_x87.tag_word = HexU64(0x7ffc);
    expected_x87.instruction_pointer = HexU64(0x0010_0004);
    expected_x87.data_pointer = HexU64(0x0030_0004);
    assert_eq!(observed.state, expected);
    assert_eq!(observed.memory.len(), 3);
    assert_eq!(observed.memory[0].bytes_hex, "d900d9e0d910dfe0");
    assert_eq!(observed.memory[1].bytes_hex, "00000000");
    assert_eq!(observed.memory[2].bytes_hex, "a5");
}

#[test]
fn local_x87_fld_fabs_fst_fnstsw_zero_fixture_checks_state_and_memory() {
    let (input, result) = run(FABS_ZERO_COMPOSITION, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 4);
    let observed = snapshot(&result);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0008);
    expected.gpr[0] = HexU64(0x0000_0000_0030_3800);
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.registers[7] = "00000000000000000000".into();
    expected_x87.status_word = HexU64(0x3800);
    expected_x87.tag_word = HexU64(0x7ffc);
    expected_x87.instruction_pointer = HexU64(0x0010_0004);
    expected_x87.data_pointer = HexU64(0x0030_0004);
    assert_eq!(observed.state, expected);
    assert_eq!(observed.memory.len(), 3);
    assert_eq!(observed.memory[0].bytes_hex, "d900d9e1d910dfe0");
    assert_eq!(observed.memory[1].bytes_hex, "00000000");
    assert_eq!(observed.memory[2].bytes_hex, "a5");
}

#[test]
fn local_x87_fld_sign_operation_fst_fnstsw_infinity_fixtures_check_state_memory_and_jit_denial() {
    for (fixture, code, expected_top, operation) in [
        (
            FCHS_INFINITY_COMPOSITION,
            "d900d9e0d910dfe0",
            "0000000000000080ff7f",
            "FCHS",
        ),
        (
            FABS_INFINITY_COMPOSITION,
            "d900d9e1d910dfe0",
            "0000000000000080ff7f",
            "FABS",
        ),
    ] {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{operation}"
        );
        assert_eq!(count(&result), 4, "{operation}");
        let observed = snapshot(&result);
        let mut expected = input.fixture.initial_state.clone();
        expected.rip = HexU64(0x0010_0008);
        expected.gpr[0] = HexU64(0x0000_0000_0030_3800);
        let expected_x87 = expected.x87.as_mut().unwrap();
        expected_x87.registers[7] = expected_top.into();
        expected_x87.status_word = HexU64(0x3800);
        expected_x87.tag_word = HexU64(0xbffc);
        expected_x87.instruction_pointer = HexU64(0x0010_0004);
        expected_x87.data_pointer = HexU64(0x0030_0004);
        assert_eq!(observed.state, expected, "{operation}");
        assert_eq!(observed.memory.len(), 3, "{operation}");
        assert_eq!(observed.memory[0].bytes_hex, code, "{operation}");
        assert_eq!(observed.memory[1].bytes_hex, "0000807f", "{operation}");
        assert_eq!(observed.memory[2].bytes_hex, "a5", "{operation}");

        let (_, jit_result) = run(fixture, "jit", 4);
        assert!(
            matches!(jit_result.outcome, Outcome::Unsupported { .. }),
            "{operation}"
        );
    }
}

#[test]
fn local_x87_fld_qnan_sign_operations_preserve_payloads_and_deny_jit() {
    for (fixture, fchs, negative) in FCHS_FABS_QNAN_FIXTURES {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 4);
        let observed = snapshot(&result);
        let expected_negative = fchs && !negative;
        let code = if fchs {
            "d900d9e0d910dfe0"
        } else {
            "d900d9e1d910dfe0"
        };
        let expected_raw = "00000000004523c1ff7f";
        let expected_memory = if expected_negative {
            "4523c1ff"
        } else {
            "4523c17f"
        };
        let mut expected = input.fixture.initial_state.clone();
        expected.rip = HexU64(0x0010_0008);
        expected.gpr[0] = HexU64(0x0000_0000_0030_3800);
        let expected_x87 = expected.x87.as_mut().unwrap();
        expected_x87.registers[7] = if expected_negative {
            expected_raw.replace("ff7f", "ffff")
        } else {
            expected_raw.into()
        };
        expected_x87.status_word = HexU64(0x3800);
        expected_x87.tag_word = HexU64(0xbffc);
        expected_x87.instruction_pointer = HexU64(0x0010_0004);
        expected_x87.data_pointer = HexU64(0x0030_0004);
        assert_eq!(observed.state, expected, "FCHS={fchs}, negative={negative}");
        assert_eq!(observed.memory[0].bytes_hex, code);
        assert_eq!(observed.memory[1].bytes_hex, expected_memory);
        assert_eq!(observed.memory[2].bytes_hex, "a5");

        let (_, jit_result) = run(fixture, "jit", 4);
        assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
    }
}

#[test]
fn local_x87_m32_snan_producers_preserve_historical_ie_through_sign_operations() {
    for (fixture, fchs, negative) in FCHS_FABS_SNAN_PRODUCER_FIXTURES {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 4);
        let observed = snapshot(&result);
        let expected_negative = if fchs { !negative } else { false };
        let expected_raw = if expected_negative {
            "00000000004523c1ffff"
        } else {
            "00000000004523c1ff7f"
        };
        let expected_memory = if expected_negative {
            "4523c1ff"
        } else {
            "4523c17f"
        };
        let mut expected = input.fixture.initial_state.clone();
        expected.rip = HexU64(0x0010_0008);
        expected.gpr[0] = HexU64(0x0000_0000_0030_3801);
        let expected_x87 = expected.x87.as_mut().unwrap();
        expected_x87.registers[7] = expected_raw.into();
        expected_x87.status_word = HexU64(0x3801);
        expected_x87.tag_word = HexU64(0xbffc);
        expected_x87.instruction_pointer = HexU64(0x0010_0004);
        expected_x87.data_pointer = HexU64(0x0030_0004);
        assert_eq!(observed.state, expected, "FCHS={fchs}, negative={negative}");
        assert_eq!(observed.memory[1].bytes_hex, expected_memory);
        assert_eq!(observed.memory[2].bytes_hex, "a5");
        let (_, jit_result) = run(fixture, "jit", 4);
        assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
    }
}

#[test]
fn local_x87_m32_subnormal_producers_preserve_de_through_sign_operations() {
    for (fixture, fchs, negative) in FCHS_FABS_SUBNORMAL_PRODUCER_FIXTURES {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 4);
        let observed = snapshot(&result);
        let mut expected = input.fixture.initial_state.clone();
        expected.rip = HexU64(0x0010_0008);
        expected.gpr[0] = HexU64(0x0000_0000_0030_3802);
        let expected_x87 = expected.x87.as_mut().unwrap();
        expected_x87.registers[7] = if fchs && !negative {
            "000000000000008080bf".into()
        } else {
            "0000000000000080803f".into()
        };
        expected_x87.status_word = HexU64(0x3802);
        expected_x87.tag_word = HexU64(0x3ffc);
        expected_x87.instruction_pointer = HexU64(0x0010_0004);
        expected_x87.data_pointer = HexU64(0x0030_0004);
        assert_eq!(observed.state, expected);
        let stored = if fchs && !negative {
            "00004080"
        } else {
            "00004000"
        };
        assert_eq!(observed.memory[1].bytes_hex, stored);
        assert_eq!(observed.memory[2].bytes_hex, "a5");
        let (_, jit_result) = run(fixture, "jit", 4);
        assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
    }
}

#[test]
fn local_x87_historical_de_fixture_and_combined_status_rejections() {
    let (input, result) = run(FCHS_DE_ONLY, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 1);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0002);
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.registers[0] = "0000000000000080ffbf".into();
    expected_x87.status_word = HexU64(2);
    expected_x87.instruction_pointer = HexU64(0x0010_0000);
    assert_eq!(snapshot(&result).state, expected);
    let (_, jit_result) = run(FCHS_DE_ONLY, "jit", 4);
    assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));

    for fixture in [FCHS_DE_SF, FCHS_IE_DE_ES] {
        let (input, result) = run(fixture, "interpreter", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert_eq!(count(&result), 0);
        assert_eq!(snapshot(&result).state, input.fixture.initial_state);
        let (_, jit_result) = run(fixture, "jit", 4);
        assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
    }
}

#[test]
fn local_x87_historical_ze_and_all_flags_fixtures_preserve_status_and_deny_jit() {
    for (fixture, expected_status, expected_raw) in [
        (FCHS_ZE_ONLY, 0x0004, "0000000000000080ffbf"),
        (FABS_ALL_FLAGS, 0x003f, "0000000000000080ff3f"),
    ] {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 1);
        let mut expected = input.fixture.initial_state.clone();
        expected.rip = HexU64(0x0010_0002);
        let expected_x87 = expected.x87.as_mut().unwrap();
        expected_x87.registers[0] = expected_raw.into();
        expected_x87.status_word = HexU64(expected_status);
        expected_x87.instruction_pointer = HexU64(0x0010_0000);
        assert_eq!(snapshot(&result).state, expected);
        let (_, jit_result) = run(fixture, "jit", 4);
        assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
    }
    for fixture in [FCHS_ZE_SF, FABS_ALL_FLAGS_ES] {
        let (input, result) = run(fixture, "interpreter", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert_eq!(count(&result), 0);
        assert_eq!(snapshot(&result).state, input.fixture.initial_state);
        let (_, jit_result) = run(fixture, "jit", 4);
        assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
    }
}

#[test]
fn local_x87_historical_ie_and_sf_ie_bare_success_fixtures() {
    let (input, result) = run(FCHS_IE_ONLY, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 1);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0002);
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.registers[0] = "0000000000000080ffbf".into();
    expected_x87.status_word = HexU64(1);
    expected_x87.instruction_pointer = HexU64(0x0010_0000);
    assert_eq!(snapshot(&result).state, expected);
    let (_, jit_result) = run(FCHS_IE_ONLY, "jit", 4);
    assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
    let (input, result) = run(FCHS_IE_SF, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 1);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0002);
    let x87 = expected.x87.as_mut().unwrap();
    x87.registers[0] = "0000000000000080ffbf".into();
    x87.status_word = HexU64(0x41);
    x87.instruction_pointer = HexU64(0x0010_0000);
    assert_eq!(snapshot(&result).state, expected);
    let (_, jit_result) = run(FCHS_IE_SF, "jit", 4);
    assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));

    let (input, result) = run(FABS_IE_SF, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 1);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0002);
    let x87 = expected.x87.as_mut().unwrap();
    x87.registers[0] = "0000000000000080ff3f".into();
    x87.status_word = HexU64(0x41);
    x87.instruction_pointer = HexU64(0x0010_0000);
    assert_eq!(snapshot(&result).state, expected);
    let (_, jit_result) = run(FABS_IE_SF, "jit", 4);
    assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
}

#[test]
fn local_x87_historical_pe_bare_fixtures_and_frndint_producers() {
    for (fixture, expected_bytes, expected_status) in [
        (FCHS_PE_ONLY, "0000000000000080ffbf", 0x20),
        (FCHS_IE_DE_PE, "0000000000000080ffbf", 0x23),
    ] {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 1);
        let observed = snapshot(&result);
        let mut expected = input.fixture.initial_state.clone();
        expected.rip = HexU64(0x0010_0002);
        let x87 = expected.x87.as_mut().unwrap();
        x87.registers[0] = expected_bytes.into();
        x87.status_word = HexU64(expected_status);
        x87.instruction_pointer = HexU64(0x0010_0000);
        assert_eq!(observed.state, expected);
        let (_, jit_result) = run(fixture, "jit", 4);
        assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
    }
    for fixture in [FCHS_PE_SF, FCHS_IE_DE_PE_ES] {
        let (input, result) = run(fixture, "interpreter", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert_eq!(count(&result), 0);
        assert_eq!(snapshot(&result).state, input.fixture.initial_state);
        let (_, jit_result) = run(fixture, "jit", 4);
        assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
    }
    for (fixture, expected_register, expected_ip) in [
        (FRNDINT_FCHS_PE, "000000000000008000c0", 0x0010_0002),
        (FRNDINT_FABS_PE, "00000000000000800040", 0x0010_0002),
    ] {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 3);
        let observed = snapshot(&result);
        assert_eq!(observed.state.rip, HexU64(0x0010_0006));
        assert_eq!(observed.state.gpr[0], HexU64(0x61));
        let x87 = observed.state.x87.as_ref().unwrap();
        assert_eq!(x87.registers[0], expected_register);
        assert_eq!(x87.status_word, HexU64(0x61));
        assert_eq!(x87.instruction_pointer, HexU64(expected_ip));
        let (_, jit_result) = run(fixture, "jit", 4);
        assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
        assert_eq!(input.fixture.instruction_budget, 3);
    }
}

#[test]
fn local_x87_historical_oe_success_and_overflow_producer_fixtures_deny_jit() {
    let (input, result) = run(FCHS_OE_ONLY, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 1);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0002);
    let x87 = expected.x87.as_mut().unwrap();
    x87.registers[0] = "0000000000000080ffbf".into();
    x87.status_word = HexU64(0x08);
    x87.instruction_pointer = HexU64(0x0010_0000);
    assert_eq!(snapshot(&result).state, expected);
    let (_, jit_result) = run(FCHS_OE_ONLY, "jit", 4);
    assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));

    for (fixture, width, store, expected_raw, expected_memory) in [
        (
            OE_FST_M32_FCHS,
            4_usize,
            "d910d9e0dfe0",
            "0000000080ffffff7ec0",
            "0000807f",
        ),
        (
            OE_FST_M64_FABS,
            8,
            "dd10d9e1dfe0",
            "00fcfffffffffffffe43",
            "000000000000f07f",
        ),
    ] {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 3);
        let observed = snapshot(&result);
        assert_eq!(observed.memory[0].bytes_hex, store);
        assert_eq!(
            observed.memory[1].bytes_hex,
            format!("{expected_memory}{}", "00".repeat(256 - width))
        );
        let mut expected = input.fixture.initial_state.clone();
        expected.rip = HexU64(0x0010_0006);
        expected.gpr[0] = HexU64(0x0030_0028);
        let x87 = expected.x87.as_mut().unwrap();
        x87.registers[0] = expected_raw.into();
        x87.status_word = HexU64(0x28);
        x87.instruction_pointer = HexU64(0x0010_0002);
        x87.data_pointer = HexU64(0x0030_0000);
        assert_eq!(observed.state, expected);
        let (_, jit_result) = run(fixture, "jit", 4);
        assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
    }
}

#[test]
fn local_x87_historical_ue_fst_producers_preserve_flags_and_deny_jit() {
    for (fixture, expected_status, width, stored) in [
        (UE_FST_M32_FCHS, 0x30_u64, 4_usize, "00000000"),
        (UE_FST_M64_FABS, 0x33, 8, "0100000000000080"),
    ] {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 3);
        let observed = snapshot(&result);
        assert_eq!(
            observed.memory[0].bytes_hex,
            if width == 4 {
                "d910d9e0dfe0"
            } else {
                "dd10d9e1dfe0"
            }
        );
        assert_eq!(
            observed.memory[1].bytes_hex,
            format!("{stored}{}", "00".repeat(256 - width))
        );
        let mut expected = input.fixture.initial_state.clone();
        expected.rip = HexU64(0x0010_0006);
        expected.gpr[0] = HexU64(0x0030_0000 | expected_status);
        let expected_x87 = expected.x87.as_mut().unwrap();
        expected_x87.status_word = HexU64(expected_status);
        expected_x87.instruction_pointer = HexU64(0x0010_0002);
        expected_x87.data_pointer = HexU64(0x0030_0000);
        let mut raw = (0..20)
            .step_by(2)
            .map(|index| {
                u8::from_str_radix(&expected_x87.registers[0][index..index + 2], 16).unwrap()
            })
            .collect::<Vec<_>>();
        if width == 4 {
            raw[9] ^= 0x80;
        } else {
            raw[9] &= 0x7f;
        }
        expected_x87.registers[0] = raw
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(observed.state, expected);
        let (_, jit_result) = run(fixture, "jit", 4);
        assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
        assert_eq!(input.fixture.instruction_budget, 3);
    }
    for fixture in [UE_ES_FCHS, UE_HISTORY_ES_FCHS] {
        let (input, result) = run(fixture, "interpreter", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert!(count(&result) <= 1);
        assert_eq!(snapshot(&result).state.rip, input.fixture.initial_state.rip);
        let (_, jit_result) = run(fixture, "jit", 4);
        assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
    }
}

#[test]
fn local_x87_bare_snan_sign_operations_preserve_payloads_and_deny_jit() {
    for (fixture, fchs, negative) in FCHS_FABS_SNAN_FIXTURES {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 1);
        let observed = snapshot(&result);
        let transformed = if fchs {
            if negative {
                "efcdab8967452381ff7f"
            } else {
                "efcdab8967452381ffff"
            }
        } else {
            "efcdab8967452381ff7f"
        };
        let mut expected = input.fixture.initial_state.clone();
        expected.rip = HexU64(0x0010_0002);
        let expected_x87 = expected.x87.as_mut().unwrap();
        expected_x87.registers[0] = transformed.into();
        expected_x87.status_word = HexU64(0);
        expected_x87.instruction_pointer = HexU64(0x0010_0000);
        assert_eq!(observed.state, expected, "FCHS={fchs}, negative={negative}");
        assert_eq!(observed.memory.len(), 1);
        assert_eq!(
            observed.memory[0].bytes_hex,
            if fchs { "d9e0" } else { "d9e1" }
        );

        let (_, jit_result) = run(fixture, "jit", 4);
        assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
    }
}

#[test]
fn local_x87_bare_subnormal_sign_operations_preserve_fraction_and_deny_jit() {
    for (fixture, fchs, negative) in FCHS_FABS_SUBNORMAL_FIXTURES {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 1);
        let observed = snapshot(&result);
        let transformed = if fchs && negative {
            "efcdab89674523010000"
        } else if fchs {
            "efcdab89674523010080"
        } else {
            "efcdab89674523010000"
        };
        let mut expected = input.fixture.initial_state.clone();
        expected.rip = HexU64(0x0010_0002);
        let expected_x87 = expected.x87.as_mut().unwrap();
        expected_x87.registers[0] = transformed.into();
        expected_x87.status_word = HexU64(0);
        expected_x87.instruction_pointer = HexU64(0x0010_0000);
        assert_eq!(observed.state, expected, "FCHS={fchs}, negative={negative}");
        assert_eq!(observed.memory.len(), 1);
        assert_eq!(
            observed.memory[0].bytes_hex,
            if fchs { "d9e0" } else { "d9e1" }
        );

        let (_, jit_result) = run(fixture, "jit", 4);
        assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
    }
}

#[test]
fn local_x87_fchs_subnormal_wrong_tag_fixture_is_atomic_in_the_cli_runner() {
    let (input, result) = run(FCHS_SUBNORMAL_WRONG_TAG, "interpreter", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    assert_eq!(snapshot(&result).state, input.fixture.initial_state);
}

#[test]
fn local_x87_fchs_snan_wrong_tag_fixture_is_atomic_in_the_cli_runner() {
    let (input, result) = run(FCHS_SNAN_WRONG_TAG, "interpreter", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    assert_eq!(snapshot(&result).state, input.fixture.initial_state);
}

#[test]
fn local_x87_fchs_qnan_wrong_tag_fixture_is_atomic_in_the_cli_runner() {
    let (input, result) = run(FCHS_QNAN_WRONG_TAG, "interpreter", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    assert_eq!(snapshot(&result).state, input.fixture.initial_state);
}

#[test]
fn local_x87_fchs_infinity_wrong_tag_fixture_is_atomic_in_the_cli_runner() {
    let (input, result) = run(FCHS_INFINITY_WRONG_TAG, "interpreter", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    let observed = snapshot(&result);
    assert_eq!(observed.state, input.fixture.initial_state);
}

#[test]
fn local_x87_fld_ftst_fnstsw_zero_fixture_checks_state_and_memory() {
    let (input, result) = run(FTST_ZERO_COMPOSITION, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 3);
    let observed = snapshot(&result);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0006);
    expected.gpr[0] = HexU64(0x0000_0000_0030_7800);
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.registers[7] = "00000000000000000080".into();
    expected_x87.status_word = HexU64(0x7800);
    expected_x87.tag_word = HexU64(0x7ffc);
    expected_x87.instruction_pointer = HexU64(0x0010_0002);
    expected_x87.data_pointer = HexU64(0x0030_0004);
    assert_eq!(observed.state, expected);
    assert_eq!(observed.memory.len(), 3);
    assert_eq!(observed.memory[0].bytes_hex, "d900d9e4dfe0");
    assert_eq!(observed.memory[1].bytes_hex, "00000080");
    assert_eq!(observed.memory[2].bytes_hex, "a5");
    let (_, jit_result) = run(FTST_ZERO_COMPOSITION, "jit", 4);
    assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
}

#[test]
fn local_x87_fld_fst_sti_fnstsw_zero_fixture_checks_state_and_memory() {
    let (input, result) = run(FST_STI_ZERO_COMPOSITION, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 3);
    let observed = snapshot(&result);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0006);
    expected.gpr[0] = HexU64(0x0000_0000_0030_3800);
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.registers[0] = "00000000000000000080".into();
    expected_x87.registers[7] = "00000000000000000080".into();
    expected_x87.status_word = HexU64(0x3800);
    expected_x87.tag_word = HexU64(0x7ffd);
    expected_x87.instruction_pointer = HexU64(0x0010_0002);
    expected_x87.data_pointer = HexU64(0x0030_0004);
    assert_eq!(observed.state, expected);
    assert_eq!(observed.memory.len(), 3);
    assert_eq!(observed.memory[0].bytes_hex, "d900ddd1dfe0");
    assert_eq!(observed.memory[1].bytes_hex, "00000080");
    assert_eq!(observed.memory[2].bytes_hex, "a5");
}

#[test]
fn local_x87_fld_fstp_sti_fnstsw_zero_fixture_checks_state_and_memory() {
    let (input, result) = run(FSTP_STI_ZERO_COMPOSITION, "interpreter", 0);
    assert!(matches!(result.outcome, Outcome::StopAddress {}));
    assert_eq!(count(&result), 3);
    let observed = snapshot(&result);
    let mut expected = input.fixture.initial_state.clone();
    expected.rip = HexU64(0x0010_0006);
    expected.gpr[0] = HexU64(0x0000_0000_0030_0000);
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.registers[0] = "00000000000000000080".into();
    expected_x87.registers[7] = "00000000000000000080".into();
    expected_x87.status_word = HexU64(0x0000);
    expected_x87.tag_word = HexU64(0xfffd);
    expected_x87.instruction_pointer = HexU64(0x0010_0002);
    expected_x87.data_pointer = HexU64(0x0030_0004);
    assert_eq!(observed.state, expected);
    assert_eq!(observed.memory.len(), 3);
    assert_eq!(observed.memory[0].bytes_hex, "d900ddd9dfe0");
    assert_eq!(observed.memory[1].bytes_hex, "00000080");
    assert_eq!(observed.memory[2].bytes_hex, "a5");
    let (_, jit_result) = run(FSTP_STI_ZERO_COMPOSITION, "jit", 4);
    assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
}

#[test]
fn local_x87_fld_fst_and_fstp_sti_infinity_fixtures_check_state_memory_and_jit_denial() {
    for (fixture, code, raw, status, tags) in [
        (
            FST_STI_INFINITY_POSITIVE,
            "d900ddd1dfe0",
            "0000000000000080ff7f",
            0x3800,
            0xbffe,
        ),
        (
            FST_STI_INFINITY_NEGATIVE,
            "d900ddd1dfe0",
            "0000000000000080ffff",
            0x3800,
            0xbffe,
        ),
        (
            FSTP_STI_INFINITY_POSITIVE,
            "d900ddd9dfe0",
            "0000000000000080ff7f",
            0x0000,
            0xfffe,
        ),
        (
            FSTP_STI_INFINITY_NEGATIVE,
            "d900ddd9dfe0",
            "0000000000000080ffff",
            0x0000,
            0xfffe,
        ),
    ] {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 3);
        let observed = snapshot(&result);
        let mut expected = input.fixture.initial_state.clone();
        expected.rip = HexU64(0x0010_0006);
        expected.gpr[0] = HexU64(0x0030_0000 | status);
        let expected_x87 = expected.x87.as_mut().unwrap();
        expected_x87.registers[0] = raw.into();
        expected_x87.registers[7] = raw.into();
        expected_x87.status_word = HexU64(status);
        expected_x87.tag_word = HexU64(tags);
        expected_x87.instruction_pointer = HexU64(0x0010_0002);
        expected_x87.data_pointer = HexU64(0x0030_0004);
        assert_eq!(observed.state, expected, "{code}");
        assert_eq!(observed.memory.len(), 3);
        assert_eq!(observed.memory[0].bytes_hex, code);
        assert_eq!(
            observed.memory[1].bytes_hex,
            if raw.ends_with("ffff") {
                "000080ff"
            } else {
                "0000807f"
            }
        );
        assert_eq!(observed.memory[2].bytes_hex, "a5");
        let (_, jit_result) = run(fixture, "jit", 4);
        assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
    }
}

#[test]
fn local_x87_fld_fst_and_fstp_sti_qnan_fixtures_check_state_memory_and_jit_denial() {
    for (fixture, code, raw, status, tags, input) in [
        (
            FST_STI_QNAN_POSITIVE,
            "d900ddd1dfe0",
            "00000000004523c1ff7f",
            0x3800,
            0xbffe,
            "4523c17f",
        ),
        (
            FST_STI_QNAN_NEGATIVE,
            "d900ddd1dfe0",
            "00000000004523c1ffff",
            0x3800,
            0xbffe,
            "4523c1ff",
        ),
        (
            FSTP_STI_QNAN_POSITIVE,
            "d900ddd9dfe0",
            "00000000004523c1ff7f",
            0x0000,
            0xfffe,
            "4523c17f",
        ),
        (
            FSTP_STI_QNAN_NEGATIVE,
            "d900ddd9dfe0",
            "00000000004523c1ffff",
            0x0000,
            0xfffe,
            "4523c1ff",
        ),
    ] {
        let (input_fixture, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 3);
        let observed = snapshot(&result);
        let mut expected = input_fixture.fixture.initial_state.clone();
        expected.rip = HexU64(0x0010_0006);
        expected.gpr[0] = HexU64(0x0030_0000 | status);
        let expected_x87 = expected.x87.as_mut().unwrap();
        expected_x87.registers[0] = raw.into();
        expected_x87.registers[7] = raw.into();
        expected_x87.status_word = HexU64(status);
        expected_x87.tag_word = HexU64(tags);
        expected_x87.instruction_pointer = HexU64(0x0010_0002);
        expected_x87.data_pointer = HexU64(0x0030_0004);
        assert_eq!(observed.state, expected, "{code}");
        assert_eq!(observed.memory.len(), 3);
        assert_eq!(observed.memory[0].bytes_hex, code);
        assert_eq!(observed.memory[1].bytes_hex, input);
        assert_eq!(observed.memory[2].bytes_hex, "a5");
        let (_, jit_result) = run(fixture, "jit", 4);
        assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
    }
}

#[test]
fn local_x87_seeded_fst_sti_snan_fixtures_preserve_raw_bytes_and_deny_jit() {
    for (fixture, code, raw, status, tags) in [
        (
            FST_STI_SNAN_POSITIVE,
            "ddd1",
            "efcdab8967452381ff7f",
            0x0000,
            0xfffa,
        ),
        (
            FST_STI_SNAN_NEGATIVE,
            "ddd1",
            "efcdab8967452381ffff",
            0x0000,
            0xfffa,
        ),
        (
            FSTP_STI_SNAN_POSITIVE,
            "ddd9",
            "efcdab8967452381ff7f",
            0x0800,
            0xfffb,
        ),
        (
            FSTP_STI_SNAN_NEGATIVE,
            "ddd9",
            "efcdab8967452381ffff",
            0x0800,
            0xfffb,
        ),
    ] {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 1);
        let observed = snapshot(&result);
        let mut expected = input.fixture.initial_state.clone();
        expected.rip = HexU64(0x0010_0002);
        let expected_x87 = expected.x87.as_mut().unwrap();
        expected_x87.registers[1] = raw.into();
        if code == "ddd9" {
            expected_x87.status_word = HexU64(status);
            expected_x87.tag_word = HexU64(tags);
        } else {
            expected_x87.tag_word = HexU64(tags);
        }
        expected_x87.instruction_pointer = HexU64(0x0010_0000);
        assert_eq!(observed.state, expected, "{code} {raw}");
        assert_eq!(observed.memory.len(), 1);
        assert_eq!(observed.memory[0].bytes_hex, code);
        let (_, jit_result) = run(fixture, "jit", 4);
        assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
    }

    let (input, result) = run(FST_STI_SNAN_WRONG_TAG, "interpreter", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    let observed = snapshot(&result);
    assert_eq!(observed.state, input.fixture.initial_state);
    assert_eq!(observed.state.rip, HexU64(0x0010_0000));
    assert_eq!(observed.memory[0].bytes_hex, "ddd1");
}

#[test]
fn local_x87_seeded_fst_sti_subnormal_fixtures_preserve_raw_bytes_and_deny_jit() {
    for (fixture, code, raw, status, tags) in [
        (
            FST_STI_SUBNORMAL_POSITIVE,
            "ddd1",
            "efcdab89674523010000",
            0x0000,
            0xfffa,
        ),
        (
            FST_STI_SUBNORMAL_NEGATIVE,
            "ddd1",
            "efcdab89674523010080",
            0x0000,
            0xfffa,
        ),
        (
            FSTP_STI_SUBNORMAL_POSITIVE,
            "ddd9",
            "efcdab89674523010000",
            0x0800,
            0xfffb,
        ),
        (
            FSTP_STI_SUBNORMAL_NEGATIVE,
            "ddd9",
            "efcdab89674523010080",
            0x0800,
            0xfffb,
        ),
    ] {
        let (input, result) = run(fixture, "interpreter", 0);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(count(&result), 1);
        let observed = snapshot(&result);
        let mut expected = input.fixture.initial_state.clone();
        expected.rip = HexU64(0x0010_0002);
        let expected_x87 = expected.x87.as_mut().unwrap();
        expected_x87.registers[1] = raw.into();
        expected_x87.tag_word = HexU64(tags);
        if code == "ddd9" {
            expected_x87.status_word = HexU64(status);
        }
        expected_x87.instruction_pointer = HexU64(0x0010_0000);
        assert_eq!(observed.state, expected, "{code} {raw}");
        assert_eq!(observed.memory[0].bytes_hex, code);
        let (_, jit_result) = run(fixture, "jit", 4);
        assert!(matches!(jit_result.outcome, Outcome::Unsupported { .. }));
    }

    let (input, result) = run(FST_STI_SUBNORMAL_WRONG_TAG, "interpreter", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    assert_eq!(snapshot(&result).state, input.fixture.initial_state);
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
fn rep_stos_wdq_cli_fixtures_check_outcomes_and_full_snapshots() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &fs::read(directory.join("s2_19b_rep_stos_wdq_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 28);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let expected_input = Fixture::from_json(&raw).unwrap();
        let exit = if expected_input.fixture.expected_exit == ExpectedExit::HarnessError {
            5
        } else {
            0
        };
        let (input, result) = run(&raw, "interpreter", exit);
        match input.fixture.expected_exit {
            ExpectedExit::StopAddress => {
                assert!(
                    matches!(result.outcome, Outcome::StopAddress {}),
                    "{case_id}"
                );
            }
            ExpectedExit::InstructionBudgetExhausted => {
                assert!(
                    matches!(result.outcome, Outcome::InstructionBudgetExhausted {}),
                    "{case_id}"
                );
            }
            ExpectedExit::RepIterationBudgetExhausted => {
                assert!(
                    matches!(result.outcome, Outcome::RepIterationBudgetExhausted {}),
                    "{case_id}"
                );
            }
            ExpectedExit::Exception => {
                assert!(matches!(result.outcome, Outcome::Exception {}), "{case_id}");
            }
            ExpectedExit::HarnessError => {
                assert!(
                    matches!(result.outcome, Outcome::HarnessError { .. }),
                    "{case_id}"
                );
            }
            _ => panic!("unexpected fixture outcome in {case_id}"),
        }
        if matches!(result.outcome, Outcome::HarnessError { .. }) {
            continue;
        }
        let observed = snapshot(&result);
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gpr, "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        assert_eq!(observed.state.rip, expected_rip, "{case_id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}"
            );
        }
        if [
            "s2_19b_rep_stos_word_df0",
            "s2_19b_rep_stos_word_df1",
            "s2_19b_rep_stos_dword_df0",
            "s2_19b_rep_stos_dword_df1",
            "s2_19b_rep_stos_qword_df0",
            "s2_19b_rep_stos_qword_df1",
        ]
        .contains(&case_id.as_str())
        {
            let (_, jit_result) = run(&raw, "jit", 4);
            assert!(
                matches!(jit_result.outcome, Outcome::Unsupported { .. }),
                "{case_id}"
            );
            assert!(
                matches!(
                    &jit_result.instructions_executed,
                    Observation::Unavailable { reason } if reason == "engine was not executed"
                ),
                "{case_id}"
            );
            assert!(
                matches!(jit_result.snapshot, Observation::Unavailable { .. }),
                "{case_id}"
            );
        }
    }
}

#[test]
fn rep_movs_cli_fixtures_check_full_state_and_jit_denial() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&fs::read(directory.join("s2_19d_rep_movs_expected.json")).unwrap())
            .unwrap();
    assert_eq!(answers.len(), 56);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let expected_input = Fixture::from_json(&raw).unwrap();
        let harness = expected_input.fixture.expected_exit == ExpectedExit::HarnessError;
        let (input, result) = run(&raw, "interpreter", if harness { 5 } else { 0 });
        assert_eq!(input.fixture.case_id, case_id);
        match input.fixture.expected_exit {
            ExpectedExit::StopAddress => assert!(
                matches!(result.outcome, Outcome::StopAddress {}),
                "{case_id}"
            ),
            ExpectedExit::InstructionBudgetExhausted => assert!(
                matches!(result.outcome, Outcome::InstructionBudgetExhausted {}),
                "{case_id}"
            ),
            ExpectedExit::RepIterationBudgetExhausted => assert!(
                matches!(result.outcome, Outcome::RepIterationBudgetExhausted {}),
                "{case_id}"
            ),
            ExpectedExit::Exception => {
                assert!(matches!(result.outcome, Outcome::Exception {}), "{case_id}")
            }
            ExpectedExit::HarnessError => assert!(
                matches!(result.outcome, Outcome::HarnessError { .. }),
                "{case_id}"
            ),
            _ => panic!("unexpected outcome for {case_id}"),
        }
        let expected_retired = if case_id.starts_with("s2_19d_test_rep_movs_") {
            if case_id.ends_with("_success") { 2 } else { 1 }
        } else if input.fixture.expected_exit == ExpectedExit::StopAddress {
            1
        } else {
            0
        };
        assert_eq!(count(&result), expected_retired, "{case_id}");
        let (_, jit_result) = run(&raw, "jit", 4);
        assert!(
            matches!(jit_result.outcome, Outcome::Unsupported { .. }),
            "{case_id}"
        );
        assert!(
            matches!(&jit_result.instructions_executed, Observation::Unavailable { reason } if reason == "engine was not executed"),
            "{case_id}"
        );
        assert!(
            matches!(jit_result.snapshot, Observation::Unavailable { .. }),
            "{case_id}"
        );
        let observed = snapshot(&result);
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gpr, "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        assert_eq!(observed.state.rip, expected_rip, "{case_id}");
        if let Some(fault) = answer.get("fault") {
            if input.fixture.expected_exit == ExpectedExit::Exception {
                let exception = match &observed.exception {
                    Observation::Available {
                        value: Some(exception),
                    } => exception,
                    _ => panic!("missing architectural exception snapshot for {case_id}"),
                };
                assert_eq!(
                    exception.vector,
                    fault["vector"].as_u64().unwrap() as u8,
                    "{case_id}"
                );
                if let Some(error_code) = fault["error_code"].as_u64() {
                    assert_eq!(
                        exception.error_code,
                        Observation::Available {
                            value: HexU64(error_code)
                        },
                        "{case_id}"
                    );
                } else {
                    assert!(
                        matches!(exception.error_code, Observation::Unavailable { .. }),
                        "{case_id}"
                    );
                }
                if let Some(address) = fault["linear_address"].as_str() {
                    assert_eq!(
                        exception.fault_address,
                        Observation::Available {
                            value: HexU64(
                                u64::from_str_radix(address.trim_start_matches("0x"), 16).unwrap()
                            )
                        },
                        "{case_id}"
                    );
                } else {
                    assert!(
                        matches!(exception.fault_address, Observation::Unavailable { .. }),
                        "{case_id}"
                    );
                }
                assert_eq!(exception.faulting_rip, expected_rip, "{case_id}");
            } else if input.fixture.expected_exit == ExpectedExit::HarnessError {
                let reason = match &result.outcome {
                    Outcome::HarnessError { reason } => reason,
                    _ => unreachable!(),
                };
                let access = fault["access"].as_str().unwrap();
                let access_name = if access == "read" { "Read" } else { "Write" };
                let address = fault["address"].as_str().unwrap();
                let address_value =
                    u64::from_str_radix(address.trim_start_matches("0x"), 16).unwrap();
                assert!(
                    reason.contains(&format!("access: {access_name}")),
                    "{case_id}: {reason}"
                );
                assert!(
                    reason.contains(&format!("address: {address_value}")),
                    "{case_id}: {reason}"
                );
                if case_id.contains("destination_fault_first") {
                    assert!(reason.contains("PermissionDenied"), "{case_id}: {reason}");
                } else {
                    assert!(reason.contains("Unmapped"), "{case_id}: {reason}");
                }
            }
        }
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
fn rep_movs_addr32_cli_fixtures_check_full_state_and_jit_denial() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &fs::read(directory.join("s2_19l_rep_movs_addr32_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 12);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let expected_input = Fixture::from_json(&raw).unwrap();
        let harness = expected_input.fixture.expected_exit == ExpectedExit::HarnessError;
        let (input, result) = run(&raw, "interpreter", if harness { 5 } else { 0 });
        assert_eq!(input.fixture.case_id, case_id);
        match input.fixture.expected_exit {
            ExpectedExit::StopAddress => assert!(
                matches!(result.outcome, Outcome::StopAddress {}),
                "{case_id}"
            ),
            ExpectedExit::InstructionBudgetExhausted => assert!(
                matches!(result.outcome, Outcome::InstructionBudgetExhausted {}),
                "{case_id}"
            ),
            ExpectedExit::HarnessError => assert!(
                matches!(result.outcome, Outcome::HarnessError { .. }),
                "{case_id}"
            ),
            _ => panic!("unexpected outcome for {case_id}"),
        }
        let retired = if input.fixture.expected_exit == ExpectedExit::StopAddress
            && input.fixture.stop_rip != Some(input.fixture.initial_state.rip)
        {
            1
        } else {
            0
        };
        assert_eq!(count(&result), retired, "{case_id}");
        let (_, jit_result) = run(&raw, "jit", 4);
        assert!(
            matches!(jit_result.outcome, Outcome::Unsupported { .. }),
            "{case_id}"
        );
        assert!(
            matches!(&jit_result.instructions_executed, Observation::Unavailable { reason } if reason == "engine was not executed"),
            "{case_id}"
        );
        assert!(
            matches!(jit_result.snapshot, Observation::Unavailable { .. }),
            "{case_id}"
        );
        let observed = snapshot(&result);
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gpr, "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        assert_eq!(observed.state.rip, expected_rip, "{case_id}");
        if let Some(fault) = answer.get("fault") {
            let reason = match &result.outcome {
                Outcome::HarnessError { reason } => reason,
                _ => unreachable!(),
            };
            let access_name = if fault["access"] == "read" {
                "Read"
            } else {
                "Write"
            };
            let address = u64::from_str_radix(
                fault["address"].as_str().unwrap().trim_start_matches("0x"),
                16,
            )
            .unwrap();
            assert!(
                reason.contains(&format!("access: {access_name}")),
                "{case_id}: {reason}"
            );
            assert!(
                reason.contains(&format!("address: {address}")),
                "{case_id}: {reason}"
            );
            assert!(reason.contains("Unmapped"), "{case_id}: {reason}");
        }
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
fn rep_lods_jit_remains_fail_closed_without_fallback() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    for id in [
        "s2_19c_rep_lods_byte_df0",
        "s2_19c_rep_lods_word_df0",
        "s2_19c_rep_lods_dword_df0",
        "s2_19c_rep_lods_qword_df0",
        "s2_19c_rep_lods_normalize_misplaced_rex",
        "s2_19c_rep_lods_normalize_rexw",
    ] {
        let raw = fs::read(directory.join(format!("{id}.case.json"))).unwrap();
        let (_, result) = run(&raw, "jit", 4);
        assert!(
            matches!(result.outcome, Outcome::Unsupported { .. }),
            "{id}"
        );
        assert!(
            matches!(
                &result.instructions_executed,
                Observation::Unavailable { reason } if reason == "engine was not executed"
            ),
            "{id}"
        );
        assert!(
            matches!(result.snapshot, Observation::Unavailable { .. }),
            "{id}"
        );
    }
}

#[test]
fn lods_cli_fixtures_match_literal_full_state_answers_and_fail_atomically() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&fs::read(directory.join("s2_18f_lods_expected.json")).unwrap())
            .unwrap();
    assert_eq!(answers.len(), 22);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let is_unmapped = case_id.contains("_unmapped");
        let (input, result) = run(&raw, "interpreter", if is_unmapped { 5 } else { 0 });
        if case_id.contains("budget0") {
            assert!(matches!(
                result.outcome,
                Outcome::InstructionBudgetExhausted {}
            ));
            assert_eq!(count(&result), 0, "{case_id}");
        } else if is_unmapped {
            // Missing fixture mappings are harness errors, not invented guest faults.
            assert_eq!(
                input.fixture.expected_exit,
                ExpectedExit::HarnessError,
                "{case_id}"
            );
            assert!(
                matches!(result.outcome, Outcome::HarnessError { .. }),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if case_id.contains("noncanonical") {
            assert!(matches!(result.outcome, Outcome::Exception {}), "{case_id}");
            assert_eq!(count(&result), 0, "{case_id}");
            let Observation::Available { value: Some(fault) } = &snapshot(&result).exception else {
                panic!("{case_id}: expected #GP snapshot");
            };
            assert_eq!(fault.vector, 13, "{case_id}");
            assert_eq!(
                fault.error_code,
                Observation::Available { value: HexU64(0) }
            );
            assert_eq!(fault.faulting_rip, input.fixture.initial_state.rip);
        } else {
            assert!(
                matches!(result.outcome, Outcome::StopAddress {}),
                "{case_id}"
            );
            assert_eq!(count(&result), 1, "{case_id}");
        }
        let observed = snapshot(&result);
        let expected_gprs: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gprs, "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.state.rip, expected_rip, "{case_id}");
        assert_eq!(observed.rflags_defined, HexU64(0x0cd7), "{case_id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}"
            );
        }
        if case_id.contains("noncanonical") || is_unmapped || case_id.contains("budget0") {
            assert_eq!(
                observed.state.gpr, input.fixture.initial_state.gpr,
                "{case_id}"
            );
            assert_eq!(
                observed.state.rip, input.fixture.initial_state.rip,
                "{case_id}"
            );
            assert_eq!(
                observed.memory[1].bytes_hex, "d1c2b3a495867768899aabbccddeef10",
                "{case_id}"
            );
        }
    }
}

#[test]
fn stos_width_cli_fixtures_match_literal_full_state_and_fail_atomically() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &fs::read(directory.join("s2_18g_stos_widths_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 20);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let expects_harness_error = case_id.contains("unmapped") || case_id.contains("readonly");
        let (input, result) = run(
            &raw,
            "interpreter",
            if expects_harness_error { 5 } else { 0 },
        );
        let no_commit = case_id.contains("unmapped")
            || case_id.contains("readonly")
            || case_id.contains("noncanonical")
            || case_id.contains("budget");
        if case_id.contains("budget") {
            assert!(matches!(
                result.outcome,
                Outcome::InstructionBudgetExhausted {}
            ));
            assert_eq!(count(&result), 0, "{case_id}");
        } else if case_id.contains("unmapped") || case_id.contains("readonly") {
            assert_eq!(
                input.fixture.expected_exit,
                ExpectedExit::HarnessError,
                "{case_id}"
            );
            assert!(
                matches!(result.outcome, Outcome::HarnessError { .. }),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if case_id.contains("noncanonical") {
            assert!(matches!(result.outcome, Outcome::Exception {}), "{case_id}");
            assert_eq!(count(&result), 0, "{case_id}");
            let Observation::Available { value: Some(fault) } = &snapshot(&result).exception else {
                panic!("{case_id}: expected #GP snapshot");
            };
            assert_eq!(fault.vector, 13, "{case_id}");
            assert_eq!(
                fault.error_code,
                Observation::Available { value: HexU64(0) }
            );
            assert_eq!(fault.faulting_rip, input.fixture.initial_state.rip);
        } else {
            assert!(
                matches!(result.outcome, Outcome::StopAddress {}),
                "{case_id}"
            );
            assert_eq!(count(&result), 1, "{case_id}");
        }

        let observed = snapshot(&result);
        let expected_gprs: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gprs, "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.state.rip, expected_rip, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}"
            );
        }
        if no_commit {
            assert_eq!(
                observed.state.gpr, input.fixture.initial_state.gpr,
                "{case_id}"
            );
            assert_eq!(
                observed.state.rip, input.fixture.initial_state.rip,
                "{case_id}"
            );
            for region in &observed.memory {
                assert_eq!(
                    region.bytes_hex,
                    input
                        .fixture
                        .memory
                        .iter()
                        .find(|spec| spec.region_id == region.region_id)
                        .unwrap()
                        .initial_bytes_hex,
                    "{case_id}: {}",
                    region.region_id
                );
            }
        } else {
            let (_, jit_result) = run(&raw, "jit", 4);
            assert!(
                matches!(jit_result.outcome, Outcome::Unsupported { .. }),
                "{case_id}: JIT must remain closed"
            );
            assert!(
                matches!(jit_result.snapshot, Observation::Unavailable { .. }),
                "{case_id}: JIT must not fall back to the interpreter"
            );
            assert!(
                matches!(
                    jit_result.instructions_executed,
                    Observation::Unavailable { .. }
                ),
                "{case_id}: JIT retirement remains unavailable"
            );
        }
    }
}

#[test]
fn stos_addr32_cli_fixtures_match_literal_full_state_and_keep_jit_closed() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &fs::read(directory.join("s2_18l_stos_addr32_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 43);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let harness_error = case_id.contains("unmapped") || case_id.contains("readonly");
        let (input, result) = run(&raw, "interpreter", if harness_error { 5 } else { 0 });
        let budget = case_id.contains("budget0");
        if harness_error {
            assert_eq!(
                input.fixture.expected_exit,
                ExpectedExit::HarnessError,
                "{case_id}"
            );
            let reason = match &result.outcome {
                Outcome::HarnessError { reason } => reason,
                _ => panic!("{case_id}: expected HarnessError, got {:?}", result.outcome),
            };
            let error = &answer["harness_error"];
            let kind = error["kind"].as_str().unwrap();
            let access = error["access"].as_str().unwrap();
            let address = error["address"].as_str().unwrap();
            let address_value = u64::from_str_radix(address.trim_start_matches("0x"), 16).unwrap();
            assert!(
                matches!(kind, "Unmapped" | "PermissionDenied"),
                "{case_id}: invalid literal error kind {kind}"
            );
            assert_eq!(access, "Write", "{case_id}");
            assert_eq!(
                reason,
                &format!(
                    "fixture memory backend: {kind} {{ address: {address_value}, access: {access} }}"
                ),
                "{case_id}"
            );
        } else if budget {
            assert!(
                matches!(result.outcome, Outcome::InstructionBudgetExhausted {}),
                "{case_id}"
            );
        } else {
            assert!(
                matches!(result.outcome, Outcome::StopAddress {}),
                "{case_id}"
            );
        }
        assert_eq!(
            count(&result),
            answer["retired"].as_u64().unwrap(),
            "{case_id}"
        );

        let observed = snapshot(&result);
        let expected_gprs: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gprs, "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.state.rip, expected_rip, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        assert_eq!(observed.memory.len(), expected_memory.len(), "{case_id}");
        for (index, region) in observed.memory.iter().enumerate() {
            assert_eq!(
                region.region_id,
                if index == 0 { "code" } else { "data" },
                "{case_id}"
            );
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}"
            );
        }
        if harness_error || budget {
            assert_eq!(
                observed.state.gpr, input.fixture.initial_state.gpr,
                "{case_id}"
            );
            assert_eq!(
                observed.state.rip, input.fixture.initial_state.rip,
                "{case_id}"
            );
            for region in &observed.memory {
                assert_eq!(
                    region.bytes_hex,
                    input
                        .fixture
                        .memory
                        .iter()
                        .find(|spec| spec.region_id == region.region_id)
                        .unwrap()
                        .initial_bytes_hex,
                    "{case_id}: {}",
                    region.region_id
                );
            }
        }

        let (_, jit_result) = run(&raw, "jit", 4);
        assert!(
            matches!(jit_result.outcome, Outcome::Unsupported { .. }),
            "{case_id}"
        );
        assert!(
            matches!(jit_result.snapshot, Observation::Unavailable { .. }),
            "{case_id}"
        );
        assert!(
            matches!(
                jit_result.instructions_executed,
                Observation::Unavailable { .. }
            ),
            "{case_id}"
        );
    }
}

#[test]
fn scas_cli_fixtures_match_literal_full_state_and_status() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&fs::read(directory.join("s2_18h_scas_expected.json")).unwrap())
            .unwrap();
    assert_eq!(answers.len(), 24);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let harness_error = case_id.contains("unmapped");
        let (input, result) = run(&raw, "interpreter", if harness_error { 5 } else { 0 });
        if case_id.contains("budget") {
            assert!(
                matches!(result.outcome, Outcome::InstructionBudgetExhausted {}),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if harness_error {
            assert_eq!(
                input.fixture.expected_exit,
                ExpectedExit::HarnessError,
                "{case_id}"
            );
            assert!(
                matches!(result.outcome, Outcome::HarnessError { .. }),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if case_id.contains("noncanonical") || case_id.ends_with("test_fault") {
            assert!(matches!(result.outcome, Outcome::Exception {}), "{case_id}");
            let Observation::Available { value: Some(fault) } = &snapshot(&result).exception else {
                panic!("{case_id}: expected architectural fault");
            };
            assert_eq!(fault.vector, 13, "{case_id}");
            assert_eq!(
                fault.error_code,
                Observation::Available { value: HexU64(0) },
                "{case_id}"
            );
            assert_eq!(
                count(&result),
                u64::from(case_id.ends_with("test_fault")),
                "{case_id}"
            );
        } else {
            assert!(
                matches!(result.outcome, Outcome::StopAddress {}),
                "{case_id}"
            );
            assert_eq!(
                count(&result),
                if case_id.ends_with("test_composition") {
                    2
                } else {
                    1
                },
                "{case_id}"
            );
        }
        let observed = snapshot(&result);
        let expected_gprs: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gprs, "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.state.rip, expected_rip, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}: {}",
                region.region_id
            );
        }
        if matches!(input.fixture.expected_exit, ExpectedExit::StopAddress) {
            let (_, jit_result) = run(&raw, "jit", 4);
            assert!(
                matches!(jit_result.outcome, Outcome::Unsupported { .. }),
                "{case_id}: JIT remains closed"
            );
            assert!(
                matches!(jit_result.snapshot, Observation::Unavailable { .. }),
                "{case_id}: no interpreter fallback"
            );
            assert!(
                matches!(
                    jit_result.instructions_executed,
                    Observation::Unavailable { .. }
                ),
                "{case_id}: no JIT retirement count"
            );
        }
        if !matches!(input.fixture.expected_exit, ExpectedExit::StopAddress) {
            for region in &observed.memory {
                assert_eq!(
                    region.bytes_hex,
                    input
                        .fixture
                        .memory
                        .iter()
                        .find(|spec| spec.region_id == region.region_id)
                        .unwrap()
                        .initial_bytes_hex,
                    "{case_id}: {}",
                    region.region_id
                );
            }
        }
    }
}

#[test]
fn cld_std_cli_fixtures_match_literal_full_state_and_jit_denial() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&fs::read(directory.join("s2_18j_cld_std_expected.json")).unwrap())
            .unwrap();
    assert_eq!(answers.len(), 32);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let cli_exit =
            match serde_json::from_slice::<serde_json::Value>(&raw).unwrap()["expected_exit"]
                .as_str()
                .unwrap()
            {
                "harness_error" => 5,
                "unsupported" => 4,
                _ => 0,
            };
        let (input, result) = run(&raw, "interpreter", cli_exit);
        match input.fixture.expected_exit {
            ExpectedExit::StopAddress => {
                assert!(
                    matches!(result.outcome, Outcome::StopAddress {}),
                    "{case_id}"
                );
            }
            ExpectedExit::InstructionBudgetExhausted => {
                assert!(
                    matches!(result.outcome, Outcome::InstructionBudgetExhausted {}),
                    "{case_id}"
                );
            }
            ExpectedExit::HarnessError => {
                assert!(
                    matches!(result.outcome, Outcome::HarnessError { .. }),
                    "{case_id}"
                );
            }
            ExpectedExit::Unsupported => {
                assert!(
                    matches!(result.outcome, Outcome::Unsupported { .. }),
                    "{case_id}"
                );
            }
            other => panic!("{case_id}: unexpected expected_exit {other:?}"),
        }
        let observed = snapshot(&result);
        let expected_gprs: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let expected_instructions: HexU64 =
            serde_json::from_value(answer["instructions_executed"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gprs, "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.state.rip, expected_rip, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        assert_eq!(count(&result), expected_instructions.get(), "{case_id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}: {}",
                region.region_id
            );
        }
        if matches!(input.fixture.expected_exit, ExpectedExit::StopAddress) {
            let (_, jit_result) = run(&raw, "jit", 4);
            assert!(
                matches!(jit_result.outcome, Outcome::Unsupported { .. }),
                "{case_id}: JIT remains closed"
            );
            assert!(
                matches!(jit_result.snapshot, Observation::Unavailable { .. }),
                "{case_id}: no interpreter fallback"
            );
            assert!(
                matches!(
                    jit_result.instructions_executed,
                    Observation::Unavailable { .. }
                ),
                "{case_id}: no JIT retirement count"
            );
        }
    }
}

#[test]
fn rep_cmpsb_cli_fixtures_check_full_state_and_jit_denial() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &fs::read(directory.join("s2_19f_rep_cmpsb_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 31);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let harness_error = case_id.contains("unmapped") || case_id.contains("_fault");
        let expected_code = if harness_error {
            5
        } else if case_id.contains("undefined_af_reject") {
            4
        } else {
            0
        };
        let (input, result) = run(&raw, "interpreter", expected_code);
        if case_id.contains("budget_zero") {
            assert!(
                matches!(result.outcome, Outcome::InstructionBudgetExhausted {}),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if case_id.contains("limit_1025") {
            assert!(
                matches!(result.outcome, Outcome::RepIterationBudgetExhausted {}),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if harness_error {
            assert_eq!(
                input.fixture.expected_exit,
                ExpectedExit::HarnessError,
                "{case_id}"
            );
            assert!(
                matches!(result.outcome, Outcome::HarnessError { .. }),
                "{case_id}"
            );
            assert_eq!(
                count(&result),
                u64::from(case_id.starts_with("s2_19f_cmp_rax_rax")),
                "{case_id}"
            );
        } else if case_id.contains("noncanonical") {
            assert!(matches!(result.outcome, Outcome::Exception {}), "{case_id}");
            assert_eq!(count(&result), 0, "{case_id}");
        } else if case_id.contains("undefined_af_reject") {
            assert!(
                matches!(result.outcome, Outcome::Unsupported { .. }),
                "{case_id}"
            );
            assert_eq!(count(&result), 1, "{case_id}");
        } else {
            assert!(
                matches!(result.outcome, Outcome::StopAddress {}),
                "{case_id}"
            );
            assert_eq!(
                count(&result),
                if case_id.contains("cmp_rax_rax") {
                    2
                } else {
                    1
                },
                "{case_id}"
            );
        }
        let observed = snapshot(&result);
        let expected_gprs: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gprs, "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.state.rip, expected_rip, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}"
            );
        }
        let (_, jit_result) = run(&raw, "jit", 4);
        assert!(
            matches!(jit_result.outcome, Outcome::Unsupported { .. }),
            "{case_id}: JIT remains denied"
        );
        assert!(
            matches!(jit_result.snapshot, Observation::Unavailable { .. }),
            "{case_id}: no interpreter fallback"
        );
        assert!(
            matches!(
                jit_result.instructions_executed,
                Observation::Unavailable { .. }
            ),
            "{case_id}: no fabricated count"
        );
    }
}

#[test]
fn rep_scas_wdq_cli_fixtures_check_full_state_and_jit_denial() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &fs::read(directory.join("s2_19g_rep_scas_wdq_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 59);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let harness_error = case_id.contains("unmapped") || case_id.contains("_fault");
        let expected_code = if harness_error {
            5
        } else if case_id.contains("undefined_af_reject") {
            4
        } else {
            0
        };
        let (input, result) = run(&raw, "interpreter", expected_code);
        if case_id.contains("budget_zero") {
            assert!(
                matches!(result.outcome, Outcome::InstructionBudgetExhausted {}),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if case_id.contains("limit_1025") {
            assert!(
                matches!(result.outcome, Outcome::RepIterationBudgetExhausted {}),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if harness_error {
            assert_eq!(
                input.fixture.expected_exit,
                ExpectedExit::HarnessError,
                "{case_id}"
            );
            assert!(
                matches!(result.outcome, Outcome::HarnessError { .. }),
                "{case_id}"
            );
            assert_eq!(
                count(&result),
                u64::from(case_id.starts_with("s2_19g_cmp_rax_rax")),
                "{case_id}"
            );
        } else if case_id.contains("noncanonical") {
            assert!(matches!(result.outcome, Outcome::Exception {}), "{case_id}");
            assert_eq!(count(&result), 0, "{case_id}");
        } else if case_id.contains("undefined_af_reject") {
            assert!(
                matches!(result.outcome, Outcome::Unsupported { .. }),
                "{case_id}"
            );
            assert_eq!(count(&result), 1, "{case_id}");
        } else {
            assert!(
                matches!(result.outcome, Outcome::StopAddress {}),
                "{case_id}"
            );
            assert_eq!(
                count(&result),
                if case_id.contains("cmp_rax_rax") {
                    2
                } else {
                    1
                },
                "{case_id}"
            );
        }
        let observed = snapshot(&result);
        let expected_gprs: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gprs, "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.state.rip, expected_rip, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}"
            );
        }
        let (_, jit_result) = run(&raw, "jit", 4);
        assert!(
            matches!(jit_result.outcome, Outcome::Unsupported { .. }),
            "{case_id}: JIT remains denied"
        );
        assert!(
            matches!(jit_result.snapshot, Observation::Unavailable { .. }),
            "{case_id}: no interpreter fallback"
        );
        assert!(
            matches!(
                jit_result.instructions_executed,
                Observation::Unavailable { .. }
            ),
            "{case_id}: no fabricated count"
        );
    }
}

#[test]
fn rep_scasb_addr32_cli_fixtures_check_full_state_and_jit_denial() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &fs::read(directory.join("s2_19m_rep_scasb_addr32_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 13);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let harness_error = case_id.contains("unmapped");
        let expected_code = if harness_error {
            5
        } else if case_id.contains("rejected")
            || case_id.contains("undefined_af")
            || case_id.contains("bad_gate")
        {
            4
        } else {
            0
        };
        let (input, result) = run(&raw, "interpreter", expected_code);
        if case_id.contains("limit_1025") {
            assert!(
                matches!(result.outcome, Outcome::RepIterationBudgetExhausted {}),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if case_id.contains("budget_zero") {
            assert!(
                matches!(result.outcome, Outcome::InstructionBudgetExhausted {}),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if harness_error {
            assert_eq!(
                input.fixture.expected_exit,
                ExpectedExit::HarnessError,
                "{case_id}"
            );
            assert!(
                matches!(result.outcome, Outcome::HarnessError { .. }),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if case_id.contains("rejected")
            || case_id.contains("undefined_af")
            || case_id.contains("bad_gate")
        {
            assert!(
                matches!(result.outcome, Outcome::Unsupported { .. }),
                "{case_id}"
            );
            assert_eq!(
                count(&result),
                u64::from(case_id.contains("undefined_af")),
                "{case_id}"
            );
        } else {
            assert!(
                matches!(result.outcome, Outcome::StopAddress {}),
                "{case_id}"
            );
            assert_eq!(
                count(&result),
                if case_id.starts_with("s2_19m_cmp_rax_rax") {
                    2
                } else {
                    1
                },
                "{case_id}"
            );
        }
        let observed = snapshot(&result);
        let expected_gprs: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gprs, "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.state.rip, expected_rip, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}"
            );
        }
        let (_, jit_result) = run(&raw, "jit", 4);
        assert!(
            matches!(jit_result.outcome, Outcome::Unsupported { .. }),
            "{case_id}: JIT denied"
        );
        assert!(
            matches!(jit_result.snapshot, Observation::Unavailable { .. }),
            "{case_id}: no fallback"
        );
        assert!(matches!(
            jit_result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn rep_scas_wdq_addr32_cli_fixtures_check_full_state_and_jit_denial() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &fs::read(directory.join("s2_19o_rep_scas_wdq_addr32_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 14);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let harness_error = case_id.contains("unmapped");
        let expected_code = if harness_error {
            5
        } else if case_id.contains("rejected")
            || case_id.contains("undefined_af")
            || case_id.contains("bad_gate")
        {
            4
        } else {
            0
        };
        let (_, result) = run(&raw, "interpreter", expected_code);
        if case_id.contains("limit_1025") {
            assert!(
                matches!(result.outcome, Outcome::RepIterationBudgetExhausted {}),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if case_id.contains("budget_zero") {
            assert!(
                matches!(result.outcome, Outcome::InstructionBudgetExhausted {}),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if harness_error {
            assert!(
                matches!(result.outcome, Outcome::HarnessError { .. }),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if case_id.contains("rejected")
            || case_id.contains("undefined_af")
            || case_id.contains("bad_gate")
        {
            assert!(
                matches!(result.outcome, Outcome::Unsupported { .. }),
                "{case_id}"
            );
            assert_eq!(
                count(&result),
                u64::from(case_id.contains("undefined_af")),
                "{case_id}"
            );
        } else {
            assert!(
                matches!(result.outcome, Outcome::StopAddress {}),
                "{case_id}"
            );
            assert_eq!(
                count(&result),
                if case_id.contains("cmp_rax_rax") {
                    2
                } else {
                    1
                },
                "{case_id}"
            );
        }
        let observed = snapshot(&result);
        let expected_gprs: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gprs, "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.state.rip, expected_rip, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}"
            );
        }
        let (_, jit_result) = run(&raw, "jit", 4);
        assert!(
            matches!(jit_result.outcome, Outcome::Unsupported { .. }),
            "{case_id}: JIT denied"
        );
        assert!(
            matches!(jit_result.snapshot, Observation::Unavailable { .. }),
            "{case_id}: no fallback"
        );
        assert!(matches!(
            jit_result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn rep_cmpsb_addr32_cli_fixtures_check_full_state_and_jit_denial() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &fs::read(directory.join("s2_19n_rep_cmpsb_addr32_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 13);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let harness_error = case_id.contains("unmapped");
        let expected_code = if harness_error {
            5
        } else if case_id.contains("rejected")
            || case_id.contains("undefined_af")
            || case_id.contains("bad_gate")
        {
            4
        } else {
            0
        };
        let (input, result) = run(&raw, "interpreter", expected_code);
        if case_id.contains("limit_1025") {
            assert!(
                matches!(result.outcome, Outcome::RepIterationBudgetExhausted {}),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if case_id.contains("budget_zero") {
            assert!(
                matches!(result.outcome, Outcome::InstructionBudgetExhausted {}),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if harness_error {
            assert_eq!(
                input.fixture.expected_exit,
                ExpectedExit::HarnessError,
                "{case_id}"
            );
            assert!(
                matches!(result.outcome, Outcome::HarnessError { .. }),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if case_id.contains("rejected")
            || case_id.contains("undefined_af")
            || case_id.contains("bad_gate")
        {
            assert!(
                matches!(result.outcome, Outcome::Unsupported { .. }),
                "{case_id}"
            );
            assert_eq!(
                count(&result),
                u64::from(case_id.contains("undefined_af")),
                "{case_id}"
            );
        } else {
            assert!(
                matches!(result.outcome, Outcome::StopAddress {}),
                "{case_id}"
            );
            assert_eq!(
                count(&result),
                if case_id.starts_with("s2_19n_cmp_rax_rax") {
                    2
                } else {
                    1
                },
                "{case_id}"
            );
        }
        let observed = snapshot(&result);
        let expected_gprs: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gprs, "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.state.rip, expected_rip, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}"
            );
        }
        let (_, jit_result) = run(&raw, "jit", 4);
        assert!(
            matches!(jit_result.outcome, Outcome::Unsupported { .. }),
            "{case_id}: JIT denied"
        );
        assert!(
            matches!(jit_result.snapshot, Observation::Unavailable { .. }),
            "{case_id}: no fallback"
        );
        assert!(matches!(
            jit_result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn rep_cmps_wdq_addr32_cli_fixtures_check_full_state_and_jit_denial() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &fs::read(directory.join("s2_19p_rep_cmps_wdq_addr32_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 14);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let harness_error = case_id.contains("unmapped");
        let rejected = case_id.contains("rejected")
            || case_id.contains("bad_gate")
            || case_id.contains("undefined_af");
        let expected_code = if harness_error {
            5
        } else if rejected {
            4
        } else {
            0
        };
        let (input, result) = run(&raw, "interpreter", expected_code);
        if case_id.contains("limit_1025") {
            assert!(
                matches!(result.outcome, Outcome::RepIterationBudgetExhausted {}),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if case_id.contains("zero_budget") {
            assert!(
                matches!(result.outcome, Outcome::InstructionBudgetExhausted {}),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if harness_error {
            assert_eq!(
                input.fixture.expected_exit,
                ExpectedExit::HarnessError,
                "{case_id}"
            );
            assert!(
                matches!(result.outcome, Outcome::HarnessError { .. }),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if rejected {
            assert!(
                matches!(result.outcome, Outcome::Unsupported { .. }),
                "{case_id}"
            );
            assert_eq!(
                count(&result),
                u64::from(case_id.contains("undefined_af")),
                "{case_id}"
            );
        } else {
            assert!(
                matches!(result.outcome, Outcome::StopAddress {}),
                "{case_id}"
            );
            assert_eq!(
                count(&result),
                if case_id.starts_with("s2_19p_cmp_to") {
                    2
                } else {
                    1
                },
                "{case_id}"
            );
        }
        let observed = snapshot(&result);
        let expected_gprs: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gprs, "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.state.rip, expected_rip, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}"
            );
        }
        let (_, jit_result) = run(&raw, "jit", 4);
        assert!(
            matches!(jit_result.outcome, Outcome::Unsupported { .. }),
            "{case_id}"
        );
        assert!(
            matches!(jit_result.snapshot, Observation::Unavailable { .. }),
            "{case_id}"
        );
        assert!(matches!(
            jit_result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn repne_scasb_addr32_cli_fixtures_check_literal_snapshots_and_keep_jit_closed() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &fs::read(directory.join("s2_19q_repne_scasb_addr32_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 2);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let (_, result) = run(&raw, "interpreter", 0);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{case_id}"
        );
        assert_eq!(count(&result), 1, "{case_id}");
        let Observation::Available { value } = result.snapshot else {
            panic!("full snapshot");
        };
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        assert_eq!(value.state.gpr, expected_gpr, "{case_id}");
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(value.state.rflags, expected_flags, "{case_id}");
        assert_eq!(value.rflags_defined, expected_defined, "{case_id}");
        assert_eq!(value.state.rip, expected_rip, "{case_id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in &value.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}"
            );
        }
        let (_, jit_result) = run(&raw, "jit", 4);
        assert!(
            matches!(jit_result.outcome, Outcome::Unsupported { .. }),
            "{case_id}"
        );
        assert!(matches!(
            jit_result.snapshot,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn rep_cmps_wdq_cli_fixtures_check_full_state_and_jit_denial() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &fs::read(directory.join("s2_19h_rep_cmps_wdq_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 71);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let harness_error = case_id.contains("unmapped") || case_id.contains("_fault");
        let expected_code = if harness_error {
            5
        } else if case_id.contains("undefined_af_reject") {
            4
        } else {
            0
        };
        let (input, result) = run(&raw, "interpreter", expected_code);
        if case_id.contains("budget_zero") {
            assert!(
                matches!(result.outcome, Outcome::InstructionBudgetExhausted {}),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if case_id.contains("limit1025") {
            assert!(
                matches!(result.outcome, Outcome::RepIterationBudgetExhausted {}),
                "{case_id}"
            );
            assert_eq!(count(&result), 0, "{case_id}");
        } else if harness_error {
            assert_eq!(
                input.fixture.expected_exit,
                ExpectedExit::HarnessError,
                "{case_id}"
            );
            assert!(
                matches!(result.outcome, Outcome::HarnessError { .. }),
                "{case_id}"
            );
            assert_eq!(
                count(&result),
                u64::from(case_id.starts_with("s2_19h_cmp_rax_rax")),
                "{case_id}"
            );
            assert_eq!(answer["fault"]["access"], "read", "{case_id}");
            let _: HexU64 = serde_json::from_value(answer["fault"]["address"].clone()).unwrap();
        } else if case_id.contains("noncanonical") {
            assert!(matches!(result.outcome, Outcome::Exception {}), "{case_id}");
            assert_eq!(count(&result), 0, "{case_id}");
            let observed = snapshot(&result);
            let Observation::Available {
                value: Some(exception),
            } = &observed.exception
            else {
                panic!("{case_id}: missing exception detail");
            };
            let expected_vector: u8 =
                serde_json::from_value(answer["fault"]["vector"].clone()).unwrap();
            assert_eq!(exception.vector, expected_vector, "{case_id}");
            assert!(
                matches!(exception.error_code, Observation::Available { value } if value.get() == 0),
                "{case_id}"
            );
            assert_eq!(answer["fault"]["error_code"], 0, "{case_id}");
            assert!(
                matches!(
                    &exception.fault_address,
                    Observation::Unavailable { reason }
                        if reason == "exception has no reported linear address"
                ),
                "{case_id}"
            );
        } else if case_id.contains("undefined_af_reject") {
            assert!(
                matches!(result.outcome, Outcome::Unsupported { .. }),
                "{case_id}"
            );
            assert_eq!(count(&result), 1, "{case_id}");
        } else {
            assert!(
                matches!(result.outcome, Outcome::StopAddress {}),
                "{case_id}"
            );
            assert_eq!(
                count(&result),
                if case_id.contains("cmp_rax_rax") {
                    2
                } else {
                    1
                },
                "{case_id}"
            );
        }
        let observed = snapshot(&result);
        let expected_gprs: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gprs, "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.state.rip, expected_rip, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}"
            );
        }

        // The CLI must deny every new string form under JIT without falling back.
        let (_, jit) = run(&raw, "jit", 4);
        assert!(
            matches!(jit.outcome, Outcome::Unsupported { .. }),
            "{case_id}"
        );
        assert!(
            matches!(jit.snapshot, Observation::Unavailable { .. }),
            "{case_id}"
        );
        assert!(
            matches!(jit.instructions_executed, Observation::Unavailable { .. }),
            "{case_id}"
        );
    }
}

#[test]
fn cmps_cli_fixtures_match_literal_full_state_and_status() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&fs::read(directory.join("s2_18i_cmps_expected.json")).unwrap())
            .unwrap();
    assert_eq!(answers.len(), 33);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let harness_error = case_id.contains("unmapped");
        let (input, result) = run(&raw, "interpreter", if harness_error { 5 } else { 0 });
        let success = input.fixture.expected_exit == ExpectedExit::StopAddress;
        if case_id.contains("budget") {
            assert!(matches!(
                result.outcome,
                Outcome::InstructionBudgetExhausted {}
            ));
            assert_eq!(count(&result), 0);
        } else if harness_error {
            assert!(matches!(result.outcome, Outcome::HarnessError { .. }));
            assert_eq!(count(&result), 0);
        } else if case_id.contains("noncanonical")
            || case_id.ends_with("test_first_fault")
            || case_id.ends_with("test_second_fault")
        {
            assert!(matches!(result.outcome, Outcome::Exception {}), "{case_id}");
            let Observation::Available { value: Some(fault) } = &snapshot(&result).exception else {
                panic!("{case_id}: expected #GP");
            };
            assert_eq!(fault.vector, 13, "{case_id}");
            assert_eq!(
                fault.error_code,
                Observation::Available { value: HexU64(0) },
                "{case_id}"
            );
            assert_eq!(
                count(&result),
                u64::from(
                    case_id.ends_with("test_first_fault") || case_id.ends_with("test_second_fault")
                ),
                "{case_id}"
            );
        } else {
            assert!(
                matches!(result.outcome, Outcome::StopAddress {}),
                "{case_id}"
            );
            assert_eq!(
                count(&result),
                if case_id.ends_with("test_success") {
                    2
                } else {
                    1
                },
                "{case_id}"
            );
        }
        let observed = snapshot(&result);
        let expected_gprs: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gprs, "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.state.rip, expected_rip, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}: {}",
                region.region_id
            );
        }
        if success {
            let (_, jit_result) = run(&raw, "jit", 4);
            assert!(
                matches!(jit_result.outcome, Outcome::Unsupported { .. }),
                "{case_id}: JIT remains closed"
            );
            assert!(matches!(
                jit_result.snapshot,
                Observation::Unavailable { .. }
            ));
            assert!(matches!(
                jit_result.instructions_executed,
                Observation::Unavailable { .. }
            ));
        }
    }
}

#[test]
fn cmps_addr32_cli_fixtures_match_all_literal_snapshots_and_keep_jit_closed() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &fs::read(directory.join("s2_18n_cmps_addr32_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 63);
    assert_eq!(
        answers
            .keys()
            .filter(|id| id.contains("dual_unmapped"))
            .count(),
        4
    );

    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let harness_error = case_id.contains("unmapped")
            || case_id.ends_with("first_fault")
            || case_id.ends_with("second_fault");
        let (input, result) = run(&raw, "interpreter", if harness_error { 5 } else { 0 });
        assert_eq!(input.fixture.case_id, case_id);
        let retired = answer["retired"].as_u64().unwrap();
        match input.fixture.expected_exit {
            ExpectedExit::StopAddress => assert!(
                matches!(result.outcome, Outcome::StopAddress {}),
                "{case_id}"
            ),
            ExpectedExit::InstructionBudgetExhausted => assert!(
                matches!(result.outcome, Outcome::InstructionBudgetExhausted {}),
                "{case_id}"
            ),
            ExpectedExit::HarnessError => {
                let reason = match &result.outcome {
                    Outcome::HarnessError { reason } => reason,
                    _ => panic!("{case_id}: expected HarnessError, got {:?}", result.outcome),
                };
                let error = &answer["harness_error"];
                assert_eq!(error["kind"], "Unmapped", "{case_id}");
                assert_eq!(error["access"], "Read", "{case_id}");
                let address = error["address"].as_str().unwrap();
                let address_value =
                    u64::from_str_radix(address.trim_start_matches("0x"), 16).unwrap();
                assert_eq!(
                    reason,
                    &format!(
                        "fixture memory backend: Unmapped {{ address: {address_value}, access: Read }}"
                    ),
                    "{case_id}"
                );
            }
            other => panic!("{case_id}: unexpected fixture exit {other:?}"),
        }
        assert_eq!(count(&result), retired, "{case_id}");

        let observed = snapshot(&result);
        let expected_gprs: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gprs, "{case_id}");
        assert_eq!(
            observed.state.rflags.0 & expected_defined.0,
            expected_flags.0 & expected_defined.0,
            "{case_id}"
        );
        assert_eq!(observed.state.rip, expected_rip, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        assert_eq!(observed.memory.len(), expected_memory.len(), "{case_id}");
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}: {}",
                region.region_id
            );
        }

        let (_, jit_result) = run(&raw, "jit", 4);
        assert!(
            matches!(jit_result.outcome, Outcome::Unsupported { .. }),
            "{case_id}: JIT must remain closed"
        );
        assert!(
            matches!(jit_result.snapshot, Observation::Unavailable { .. }),
            "{case_id}: no interpreter fallback"
        );
        assert!(
            matches!(
                jit_result.instructions_executed,
                Observation::Unavailable { .. }
            ),
            "{case_id}: no JIT count"
        );
    }
}

#[test]
fn movs_addr32_cli_fixtures_match_literal_snapshots_and_keep_jit_closed() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &fs::read(directory.join("s2_18o_movs_addr32_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 73);

    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let is_error = answer["expected_exit"] == "harness_error";
        let (input, result) = run(&raw, "interpreter", if is_error { 5 } else { 0 });
        assert_eq!(input.fixture.case_id, case_id);
        let expected_exit = answer["expected_exit"].as_str().unwrap();
        match expected_exit {
            "stop_address" => assert!(
                matches!(result.outcome, Outcome::StopAddress {}),
                "{case_id}"
            ),
            "instruction_budget_exhausted" => assert!(
                matches!(result.outcome, Outcome::InstructionBudgetExhausted {}),
                "{case_id}"
            ),
            "harness_error" => {
                let reason = match &result.outcome {
                    Outcome::HarnessError { reason } => reason,
                    _ => panic!("{case_id}: expected HarnessError, got {:?}", result.outcome),
                };
                let error = &answer["harness_error"];
                let address = u64::from_str_radix(
                    error["address"].as_str().unwrap().trim_start_matches("0x"),
                    16,
                )
                .unwrap();
                let access = error["access"].as_str().unwrap();
                let kind = error["kind"].as_str().unwrap();
                let expected_reason = match kind {
                    "Unmapped" => format!(
                        "fixture memory backend: Unmapped {{ address: {address}, access: {access} }}"
                    ),
                    "PermissionDenied" => format!(
                        "fixture memory backend: PermissionDenied {{ address: {address}, access: {access} }}"
                    ),
                    other => panic!("{case_id}: unsupported fixture error kind {other}"),
                };
                assert_eq!(reason, &expected_reason, "{case_id}");
            }
            other => panic!("{case_id}: unknown expected exit {other}"),
        }
        assert_eq!(
            count(&result),
            answer["retired"].as_u64().unwrap(),
            "{case_id}"
        );

        let observed = snapshot(&result);
        let expected_gprs: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gprs, "{case_id}");
        assert_eq!(
            observed.state.rflags.0 & expected_defined.0,
            expected_flags.0 & expected_defined.0,
            "{case_id}"
        );
        assert_eq!(observed.state.rip, expected_rip, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        assert_eq!(observed.memory.len(), expected_memory.len(), "{case_id}");
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}"
            );
        }

        let (_, jit_result) = run(&raw, "jit", 4);
        assert!(
            matches!(jit_result.outcome, Outcome::Unsupported { .. }),
            "{case_id}"
        );
        assert!(
            matches!(jit_result.snapshot, Observation::Unavailable { .. }),
            "{case_id}"
        );
        assert!(
            matches!(
                jit_result.instructions_executed,
                Observation::Unavailable { .. }
            ),
            "{case_id}"
        );
    }
}

#[test]
fn movs_addr32_rejects_69_prefix_variants_as_unsupported_end_to_end() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let base = fs::read(directory.join("s2_18o_pos_b_df0.case.json")).unwrap();
    let mut cases: Vec<Vec<u8>> = Vec::new();
    let forms: [&[u8]; 4] = [
        &[0x67, 0xa4],
        &[0x67, 0x66, 0xa5],
        &[0x67, 0xa5],
        &[0x67, 0x48, 0xa5],
    ];
    let prefixes: [&[u8]; 8] = [
        &[0xf0],
        &[0xf2],
        &[0x26],
        &[0x2e],
        &[0x36],
        &[0x3e],
        &[0x64],
        &[0x65],
    ];
    for form in forms {
        for prefix in prefixes {
            cases.push([prefix, form].concat());
        }
    }
    cases.extend([
        vec![0x67, 0x67, 0xa4],
        vec![0x67, 0x67, 0x66, 0xa5],
        vec![0x67, 0x67, 0xa5],
        vec![0x67, 0x67, 0x48, 0xa5],
        vec![0x67, 0x66, 0xa4],
    ]);
    for rex in 0x40..=0x4f {
        cases.push(vec![0x67, rex, 0xa4]);
    }
    for rex in (0x40..=0x4f).filter(|rex| rex & 8 == 0) {
        cases.push(vec![0x67, rex, 0xa5]);
        cases.push(vec![0x67, 0x66, rex, 0xa5]);
    }
    assert_eq!(cases.len(), 69);

    for (index, bytes) in cases.into_iter().enumerate() {
        let mut fixture: serde_json::Value = serde_json::from_slice(&base).unwrap();
        fixture["case_id"] = serde_json::Value::String(format!("s2_18o_prefix_reject_{index:02}"));
        fixture["memory"][0]["length"] = serde_json::Value::from(bytes.len() as u64);
        fixture["memory"][0]["initial_bytes_hex"] =
            serde_json::Value::String(bytes.iter().map(|byte| format!("{byte:02x}")).collect());
        fixture["memory"][0]["comparison_mask_hex"] =
            serde_json::Value::String("ff".repeat(bytes.len()));
        fixture["stop_rip"] =
            serde_json::Value::String(format!("0x{:016x}", 0x0010_0000 + bytes.len() as u64));
        fixture["expected_exit"] = serde_json::Value::String("unsupported".into());
        let raw = serde_json::to_vec(&fixture).unwrap();
        let (input, result) = run(&raw, "interpreter", 4);
        assert_eq!(
            input.fixture.case_id,
            format!("s2_18o_prefix_reject_{index:02}")
        );
        assert!(
            matches!(result.outcome, Outcome::Unsupported { .. }),
            "{bytes:02x?}"
        );
        assert_eq!(count(&result), 0, "{bytes:02x?}");
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
fn test_rep_iteration_limit_fixtures_resume_with_literal_definedness_snapshots() {
    use od_isa::execute::{self, RunConfig, RunExit};
    use od_isa::memory::AccessContext;
    use od_testkit::memory::FlatMemory;

    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &fs::read(directory.join("s2_19i_rep_restart_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 6);

    for (case_id, expected) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let (input, cli_result) = run(&raw, "interpreter", 0);
        assert_eq!(
            cli_result.outcome,
            Outcome::RepIterationBudgetExhausted {},
            "{case_id}"
        );
        assert_eq!(count(&cli_result), 1, "{case_id}");
        let cli_snapshot = snapshot(&cli_result);
        let partial = &expected["partial"];
        let expected_partial_gpr: [HexU64; 16] =
            serde_json::from_value(partial["gpr"].clone()).unwrap();
        assert_eq!(cli_snapshot.state.gpr, expected_partial_gpr, "{case_id}");
        let expected_partial_rflags: HexU64 =
            serde_json::from_value(partial["rflags"].clone()).unwrap();
        let partial_defined: HexU64 =
            serde_json::from_value(partial["rflags_defined"].clone()).unwrap();
        assert_eq!(
            cli_snapshot.state.rflags.get() & partial_defined.get(),
            expected_partial_rflags.get() & partial_defined.get(),
            "{case_id}"
        );
        assert_eq!(cli_snapshot.rflags_defined, partial_defined, "{case_id}");
        assert_eq!(
            cli_snapshot.state.rip,
            serde_json::from_value(partial["rip"].clone()).unwrap(),
            "{case_id}"
        );
        let partial_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(partial["memory"].clone()).unwrap();
        for region in &cli_snapshot.memory {
            assert_eq!(
                region.bytes_hex, partial_memory[&region.region_id],
                "{case_id}: {}",
                region.region_id
            );
        }

        let mut resume_fixture = input.fixture.clone();
        resume_fixture.initial_state = cli_snapshot.state.clone();
        for region in &mut resume_fixture.memory {
            let partial_region = cli_snapshot
                .memory
                .iter()
                .find(|snapshot| snapshot.region_id == region.region_id)
                .unwrap();
            assert_eq!(
                region.guest_address, partial_region.guest_address,
                "{case_id}"
            );
            region.initial_bytes_hex = partial_region.bytes_hex.clone();
        }
        let resumed_fixture = Fixture::from_json(&serde_json::to_vec(&resume_fixture).unwrap())
            .unwrap()
            .fixture;
        for (restored, original) in resumed_fixture.memory.iter().zip(&input.fixture.memory) {
            assert_eq!(restored.guest_address, original.guest_address, "{case_id}");
            assert_eq!(restored.permissions, original.permissions, "{case_id}");
        }
        let mut memory = FlatMemory::from_fixture(&resumed_fixture).unwrap();
        let mut state = resumed_fixture.initial_state.to_cpu().unwrap();
        let resumed = execute::run_with_rflags_defined(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: resumed_fixture.stop_rip.map(HexU64::get),
                context: AccessContext::default(),
            },
            cli_snapshot.rflags_defined.get(),
            || false,
        )
        .unwrap();
        assert_eq!(resumed.exit, RunExit::StopAddress, "{case_id}");
        assert_eq!(resumed.instructions_executed, 1, "{case_id}");
        assert_eq!(resumed.rflags_defined, 0x0cc7, "{case_id}");

        let final_snapshot = &expected["resumed"];
        let final_gpr: [HexU64; 16] =
            serde_json::from_value(final_snapshot["gpr"].clone()).unwrap();
        assert_eq!(state.gpr, final_gpr.map(HexU64::get), "{case_id}");
        let expected_final_rflags: HexU64 =
            serde_json::from_value(final_snapshot["rflags"].clone()).unwrap();
        let expected_final_defined: HexU64 =
            serde_json::from_value(final_snapshot["rflags_defined"].clone()).unwrap();
        assert_eq!(
            state.rflags & expected_final_defined.get(),
            expected_final_rflags.get() & expected_final_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            resumed.rflags_defined,
            expected_final_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            state.rip,
            serde_json::from_value::<HexU64>(final_snapshot["rip"].clone())
                .unwrap()
                .get(),
            "{case_id}"
        );
        let final_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(final_snapshot["memory"].clone()).unwrap();
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, final_memory[&region.region_id],
                "{case_id}: {}",
                region.region_id
            );
        }
    }
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

#[test]
fn local_x87_fxch_st1_cli_wraparound_roundtrip_wrong_tag_and_jit_denial() {
    let mut fixture = Fixture::from_json(FCHS_NORMAL).unwrap().fixture;
    fixture.case_id = "s2_30bj_fxch_st1_top7_c1".into();
    fixture.profile = od_testkit::v2::Profile::LongX87StateV1;
    fixture.initial_state.rip = HexU64(0x0010_0000);
    let mut x87 = fixture
        .initial_state
        .x87
        .as_ref()
        .unwrap()
        .to_cpu()
        .unwrap();
    x87.control_word = 0x037f;
    x87.status_word = 0x3a00;
    x87.tag_word = 0;
    x87.registers[7] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f];
    x87.registers[0] = [0, 0, 0, 0, 0, 0, 0, 0xc0, 0x00, 0x40];
    x87.set_tag(7, od_isa::state::X87Tag::Valid);
    x87.set_tag(0, od_isa::state::X87Tag::Valid);
    fixture.initial_state.x87 = Some(od_testkit::v2::X87Snapshot::from(&x87));
    let code = fixture
        .memory
        .iter_mut()
        .find(|r| r.region_id == "code")
        .unwrap();
    code.guest_address = HexU64(0x0010_0000);
    code.length = 2;
    code.initial_bytes_hex = "d9c9".into();
    code.comparison_mask_hex = "ffff".into();
    fixture.instruction_budget = 1;
    fixture.stop_rip = Some(HexU64(0x0010_0002));
    fixture.expected_exit = ExpectedExit::StopAddress;
    let input = fixture.to_json().unwrap();
    let (loaded, result) = run(&input, "interpreter", 0);
    assert_eq!(count(&result), 1);
    let observed = snapshot(&result);
    let actual = observed.state.x87.as_ref().unwrap();
    assert_eq!(actual.registers[7], "00000000000000c00040");
    assert_eq!(actual.registers[0], "0000000000000080ff3f");
    assert_eq!(actual.tag_word, HexU64(0));
    assert_eq!(actual.status_word, HexU64(0x3800)); // TOP=7 and C1 cleared.
    assert_eq!(actual.instruction_pointer, loaded.fixture.initial_state.rip);
    assert_eq!(
        actual.data_pointer,
        loaded
            .fixture
            .initial_state
            .x87
            .as_ref()
            .unwrap()
            .data_pointer
    );
    assert_eq!(observed.state.rip, HexU64(0x0010_0002));

    let mut observe = loaded.fixture.clone();
    observe.case_id = "s2_30bj_fxch_fnstsw".into();
    let code = observe
        .memory
        .iter_mut()
        .find(|r| r.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "d9c9dfe0".into();
    code.length = 4;
    code.comparison_mask_hex = "ffff".repeat(2);
    observe.instruction_budget = 2;
    observe.stop_rip = Some(HexU64(0x0010_0004));
    let (_, observed_result) = run(&observe.to_json().unwrap(), "interpreter", 0);
    assert_eq!(count(&observed_result), 2);
    assert_eq!(
        snapshot(&observed_result).state.gpr[0].get() & 0xffff,
        0x3800
    );

    let mut roundtrip = loaded.fixture.clone();
    roundtrip.case_id = "s2_30bj_fxch_roundtrip".into();
    let code = roundtrip
        .memory
        .iter_mut()
        .find(|r| r.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "d9c9d9c9dfe0".into();
    code.length = 6;
    code.comparison_mask_hex = "ff".repeat(6);
    roundtrip.instruction_budget = 3;
    roundtrip.stop_rip = Some(HexU64(0x0010_0006));
    let (roundtrip_input, roundtrip_result) = run(&roundtrip.to_json().unwrap(), "interpreter", 0);
    assert_eq!(count(&roundtrip_result), 3);
    let final_state = snapshot(&roundtrip_result);
    let mut expected_x87 = roundtrip_input.fixture.initial_state.x87.clone().unwrap();
    expected_x87.status_word = HexU64(0x3800);
    expected_x87.instruction_pointer = HexU64(0x0010_0002);
    assert_eq!(final_state.state.x87, Some(expected_x87));

    let mut wrong_tag = loaded.fixture;
    wrong_tag.case_id = "s2_30bj_fxch_wrong_tag".into();
    wrong_tag.stop_rip = None;
    wrong_tag.expected_exit = ExpectedExit::Unsupported;
    wrong_tag.initial_state.x87.as_mut().unwrap().tag_word = HexU64(3);
    let (_, rejected) = run(&wrong_tag.to_json().unwrap(), "interpreter", 4);
    assert_eq!(count(&rejected), 0);
    assert!(matches!(rejected.outcome, Outcome::Unsupported { .. }));
    assert_eq!(snapshot(&rejected).state.x87, wrong_tag.initial_state.x87);

    let (_, jit) = run(&input, "jit", 4);
    assert!(matches!(jit.outcome, Outcome::Unsupported { .. }));
}

#[test]
fn local_x87_fxch_st0_i7_and_nonadjacent_cli_forms() {
    let make_fixture = |case_id: &str, bytes: &str, top: u8, budget: u64| {
        let mut fixture = Fixture::from_json(FCHS_NORMAL).unwrap().fixture;
        fixture.case_id = case_id.into();
        fixture.profile = od_testkit::v2::Profile::LongX87StateV1;
        fixture.initial_state.rip = HexU64(0x0010_1000);
        let mut x87 = fixture
            .initial_state
            .x87
            .as_ref()
            .unwrap()
            .to_cpu()
            .unwrap();
        x87.control_word = 0x037f;
        x87.status_word = (u16::from(top) << 11) | 0x0200;
        x87.tag_word = 0;
        x87.registers[usize::from(top)] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f];
        for physical in 0..8 {
            x87.set_tag(physical, od_isa::state::X87Tag::Valid);
        }
        fixture.initial_state.x87 = Some(od_testkit::v2::X87Snapshot::from(&x87));
        let code = fixture
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.guest_address = fixture.initial_state.rip;
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        fixture.instruction_budget = budget;
        fixture.stop_rip = Some(HexU64(
            fixture.initial_state.rip.get() + bytes.len() as u64 / 2,
        ));
        fixture.expected_exit = ExpectedExit::StopAddress;
        fixture
    };

    let self_exchange = make_fixture("s2_30bk_fxch_st0", "d9c8", 5, 1);
    let (_, self_result) = run(&self_exchange.to_json().unwrap(), "interpreter", 0);
    assert_eq!(count(&self_result), 1);
    let self_x87 = snapshot(&self_result).state.x87.clone().unwrap();
    assert_eq!(self_x87.registers[5], "0000000000000080ff3f");
    assert_eq!(self_x87.status_word, HexU64((5 << 11) as u64));
    assert_eq!(
        self_x87.instruction_pointer,
        self_exchange.initial_state.rip
    );

    let mut i7_wrap = make_fixture("s2_30bk_fxch_st7_wrap", "d9cf", 1, 1);
    let x87 = i7_wrap.initial_state.x87.as_mut().unwrap();
    let mut cpu = x87.to_cpu().unwrap();
    cpu.registers[0] = [0, 0, 0, 0, 0, 0, 0, 0xc0, 0x00, 0x40];
    i7_wrap.initial_state.x87 = Some(od_testkit::v2::X87Snapshot::from(&cpu));
    let (_, i7_result) = run(&i7_wrap.to_json().unwrap(), "interpreter", 0);
    assert_eq!(count(&i7_result), 1);
    let i7_x87 = snapshot(&i7_result).state.x87.clone().unwrap();
    assert_eq!(i7_x87.registers[1], "00000000000000c00040");
    assert_eq!(i7_x87.registers[0], "0000000000000080ff3f");

    let mut nonadjacent = make_fixture("s2_30bk_fxch_st3_roundtrip", "d9cbd9cbdfe0", 7, 3);
    let x87 = nonadjacent.initial_state.x87.as_mut().unwrap();
    let mut cpu = x87.to_cpu().unwrap();
    cpu.registers[2] = [0, 0, 0, 0, 0, 0, 0, 0xc0, 0x00, 0x40];
    nonadjacent.initial_state.x87 = Some(od_testkit::v2::X87Snapshot::from(&cpu));
    let (nonadjacent, roundtrip) = run(&nonadjacent.to_json().unwrap(), "interpreter", 0);
    assert_eq!(count(&roundtrip), 3);
    let final_x87 = snapshot(&roundtrip).state.x87.clone().unwrap();
    let initial_x87 = nonadjacent.fixture.initial_state.x87.unwrap();
    assert_eq!(final_x87.registers[7], initial_x87.registers[7]);
    assert_eq!(final_x87.registers[2], initial_x87.registers[2]);
    assert_eq!(final_x87.tag_word, initial_x87.tag_word);

    let (_, jit) = run(&self_exchange.to_json().unwrap(), "jit", 4);
    assert!(matches!(jit.outcome, Outcome::Unsupported { .. }));
}

#[test]
fn local_x87_fxch_signed_zero_payload_tag_transfer_and_roundtrip() {
    let make_fixture = |case_id: &str, bytes: &str, top: u8| {
        let mut fixture = Fixture::from_json(FCHS_NORMAL).unwrap().fixture;
        fixture.case_id = case_id.into();
        fixture.profile = od_testkit::v2::Profile::LongX87StateV1;
        fixture.initial_state.rip = HexU64(0x0010_1200);
        let mut x87 = fixture
            .initial_state
            .x87
            .as_ref()
            .unwrap()
            .to_cpu()
            .unwrap();
        x87.control_word = 0x037f;
        x87.status_word = (u16::from(top) << 11) | 0x0200;
        x87.tag_word = 0x6c93;
        for physical in 0..8 {
            x87.registers[physical] = [0x40 + physical as u8; 10];
        }
        fixture.initial_state.x87 = Some(od_testkit::v2::X87Snapshot::from(&x87));
        let code = fixture
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.guest_address = fixture.initial_state.rip;
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        fixture.instruction_budget = (bytes.len() / 4) as u64;
        fixture.stop_rip = Some(HexU64(
            fixture.initial_state.rip.get() + bytes.len() as u64 / 2,
        ));
        fixture.expected_exit = ExpectedExit::StopAddress;
        fixture
    };

    // TOP=1, i=7 wraps to physical 0. Exchange -0 and a canonical Normal;
    // all six other cells retain their raw bytes and packed tags.
    let mut mixed = make_fixture("s2_30bl_fxch_zero_normal_wrap", "d9cf", 1);
    let x87 = mixed.initial_state.x87.as_mut().unwrap();
    x87.registers[1] = "00000000000000000080".into();
    x87.registers[0] = "0000000000000080ff3f".into();
    x87.tag_word = HexU64((x87.tag_word.get() & !0xf) | 0b0100);
    let (loaded, result) = run(&mixed.to_json().unwrap(), "interpreter", 0);
    assert_eq!(count(&result), 1);
    let actual = snapshot(&result).state.x87.as_ref().unwrap();
    let initial_registers = &loaded.fixture.initial_state.x87.as_ref().unwrap().registers;
    for physical in 0..8 {
        let expected = match physical {
            0 => &initial_registers[1],
            1 => &initial_registers[0],
            _ => &initial_registers[physical],
        };
        assert_eq!(
            actual.registers[physical], *expected,
            "physical register {physical}"
        );
    }
    let mut expected_tags = loaded.fixture.initial_state.x87.unwrap().tag_word;
    // Physical 1 carried Zero (01), physical 0 Valid (00), so swap those tags.
    expected_tags = HexU64((expected_tags.get() & !0xf) | 0b0001);
    assert_eq!(actual.tag_word, expected_tags);
    assert_eq!(actual.status_word, HexU64((1 << 11) as u64));

    // Opposite signed zeros transfer between the same wrapped cells and return
    // byte-for-byte, with both Zero tags restored after two exchanges.
    let mut zeros = make_fixture("s2_30bl_fxch_opposite_zeros_roundtrip", "d9cfd9cfdfe0", 7);
    let x87 = zeros.initial_state.x87.as_mut().unwrap();
    x87.registers[7] = "00000000000000000080".into();
    x87.registers[6] = "00000000000000000000".into();
    x87.tag_word =
        HexU64((x87.tag_word.get() & !(0b11 << 14 | 0b11 << 12)) | (1 << 14) | (1 << 12));
    zeros.instruction_budget = 3;
    zeros.stop_rip = Some(HexU64(0x0010_1206));
    let (loaded, result) = run(&zeros.to_json().unwrap(), "interpreter", 0);
    assert_eq!(count(&result), 3);
    let actual = snapshot(&result).state.x87.as_ref().unwrap();
    let initial = loaded.fixture.initial_state.x87.unwrap();
    for physical in 0..8 {
        assert_eq!(
            actual.registers[physical], initial.registers[physical],
            "physical register {physical}"
        );
    }
    assert_eq!(actual.tag_word, initial.tag_word);
    assert_eq!(actual.status_word, HexU64((7 << 11) as u64));

    let (_, jit) = run(&mixed.to_json().unwrap(), "jit", 4);
    assert!(matches!(jit.outcome, Outcome::Unsupported { .. }));
}

#[test]
fn local_x87_fincstp_fdecstp_fnstsw_preserve_opaque_cells_and_deny_jit() {
    for (fixture_bytes, expected_top) in [(FINCSTP_FIXTURE, 0u64), (FDECSTP_FIXTURE, 6u64)] {
        let loaded = Fixture::from_json(fixture_bytes).unwrap();
        let before = loaded.fixture.initial_state.x87.as_ref().unwrap().clone();
        let (_, result) = run(fixture_bytes, "interpreter", 0);
        assert_eq!(count(&result), 2);
        let observed = snapshot(&result);
        let after = observed.state.x87.as_ref().unwrap();
        assert_eq!(after.status_word, HexU64(expected_top << 11));
        assert_eq!(after.tag_word, before.tag_word);
        assert_eq!(after.registers, before.registers);
        assert_eq!(after.instruction_pointer, loaded.fixture.initial_state.rip);
        assert_eq!(after.data_pointer, before.data_pointer);
        assert_eq!(after.last_opcode, before.last_opcode);
        assert_eq!(observed.state.rip, HexU64(0x0010_0004));
        assert_eq!(observed.state.gpr[0].get() & 0xffff, expected_top << 11);
    }
    for fixture_bytes in [FINCSTP_FDECSTP_FIXTURE, FDECSTP_FINCSTP_FIXTURE] {
        let loaded = Fixture::from_json(fixture_bytes).unwrap();
        let before = loaded.fixture.initial_state.x87.as_ref().unwrap().clone();
        let (_, result) = run(fixture_bytes, "interpreter", 0);
        assert_eq!(count(&result), 3);
        let observed = snapshot(&result);
        let after = observed.state.x87.as_ref().unwrap();
        assert_eq!(after.status_word, HexU64(7 << 11));
        assert_eq!(after.tag_word, before.tag_word);
        assert_eq!(after.registers, before.registers);
        assert_eq!(observed.state.rip, HexU64(0x0010_0006));
        assert_eq!(observed.state.gpr[0].get() & 0xffff, 7 << 11);
    }
    let (_, jit) = run(FINCSTP_FIXTURE, "jit", 4);
    assert!(matches!(jit.outcome, Outcome::Unsupported { .. }));
}

#[test]
fn local_x87_fld_st0_wrap_producers_atomic_rejects_and_jit_denial() {
    for (bytes, expected_top, expected_tag, expected_raw) in [
        (
            FLD_ST0_NEGATIVE_ZERO_WRAP,
            7u64,
            1u64,
            "00000000000000000080",
        ),
        (FLD_ST0_FNSTSW_WRAP, 6u64, 0u64, "0000000000000080ff3f"),
    ] {
        let loaded = Fixture::from_json(bytes).unwrap();
        let before = loaded.fixture.initial_state.x87.as_ref().unwrap().clone();
        let (_, result) = run(bytes, "interpreter", 0);
        assert_eq!(count(&result), loaded.fixture.instruction_budget);
        let after = snapshot(&result).state.x87.as_ref().unwrap();
        assert_eq!(after.status_word.get() >> 11 & 7, expected_top);
        assert_eq!(after.registers[expected_top as usize], expected_raw);
        assert_eq!(
            (after.tag_word.get() >> (expected_top * 2)) & 3,
            expected_tag
        );
        let source = ((before.status_word.get() >> 11) & 7) as usize;
        assert_eq!(after.registers[source], before.registers[source]);
        assert_eq!(
            (after.tag_word.get() >> (source * 2)) & 3,
            (before.tag_word.get() >> (source * 2)) & 3
        );
        assert_eq!(after.instruction_pointer, loaded.fixture.initial_state.rip);
    }
    for bytes in [FLD1_FLD_ST0_FNSTSW, FLDZ_FLD_ST0_FNSTSW] {
        let (_, result) = run(bytes, "interpreter", 0);
        assert_eq!(count(&result), 3);
        let after = snapshot(&result);
        let x87 = after.state.x87.as_ref().unwrap();
        assert_eq!(x87.status_word.get() >> 11 & 7, 6);
        assert_eq!(
            (x87.tag_word.get() >> (6 * 2)) & 3,
            if bytes == FLD1_FLD_ST0_FNSTSW { 0 } else { 1 }
        );
        let expected = if bytes == FLD1_FLD_ST0_FNSTSW {
            "0000000000000080ff3f"
        } else {
            "00000000000000000000"
        };
        assert_eq!(x87.registers[6], expected);
        assert_eq!(x87.registers[7], expected);
        assert_eq!(after.state.gpr[0].get() & 0xffff, 6 << 11);
    }
    for bytes in [
        FLD_ST0_EMPTY_SOURCE_REJECTS,
        FLD_ST0_OCCUPIED_DESTINATION_REJECTS,
    ] {
        let loaded = Fixture::from_json(bytes).unwrap();
        let (_, result) = run(bytes, "interpreter", 4);
        assert_eq!(count(&result), 0);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert_eq!(snapshot(&result).state, loaded.fixture.initial_state);
    }
    let (_, jit) = run(FLD_ST0_NEGATIVE_ZERO_WRAP, "jit", 4);
    assert!(matches!(jit.outcome, Outcome::Unsupported { .. }));
}

#[test]
fn local_x87_fld_sti_indexes_copy_selected_physical_source_and_remain_jit_denied() {
    for (source_index, bytes) in FLD_STI_FIXTURES.into_iter().enumerate() {
        let loaded = Fixture::from_json(bytes).unwrap();
        let before = loaded.fixture.initial_state.x87.as_ref().unwrap().clone();
        let old_top = ((before.status_word.get() >> 11) & 7) as usize;
        let source = (old_top + source_index + 1) & 7;
        let destination = (old_top + 7) & 7;
        let raw = before.registers[source].clone();
        let tag = (before.tag_word.get() >> (source * 2)) & 3;
        let (_, result) = run(bytes, "interpreter", 0);
        assert_eq!(count(&result), 1);
        let after = snapshot(&result).state.x87.as_ref().unwrap();
        assert_eq!(after.registers[source], raw);
        assert_eq!((after.tag_word.get() >> (source * 2)) & 3, tag);
        assert_eq!(after.registers[destination], raw);
        assert_eq!((after.tag_word.get() >> (destination * 2)) & 3, tag);
        assert_eq!(((after.status_word.get() >> 11) & 7) as usize, destination);
        assert_eq!(after.status_word.get() & (1 << 9), 0);
        assert_eq!(after.instruction_pointer, loaded.fixture.initial_state.rip);
        let mut expected_x87 = before;
        expected_x87.registers[destination] = raw;
        expected_x87.tag_word = HexU64(
            (expected_x87.tag_word.get() & !(3 << (destination * 2))) | (tag << (destination * 2)),
        );
        expected_x87.status_word = HexU64(
            (expected_x87.status_word.get() & !((7 << 11) | (1 << 9)))
                | ((destination as u64) << 11),
        );
        expected_x87.instruction_pointer = loaded.fixture.initial_state.rip;
        let mut expected_state = loaded.fixture.initial_state.clone();
        expected_state.rip = HexU64(expected_state.rip.get() + 2);
        expected_state.x87 = Some(expected_x87);
        assert_eq!(snapshot(&result).state, expected_state);
        let (_, jit) = run(bytes, "jit", 4);
        assert!(matches!(jit.outcome, Outcome::Unsupported { .. }));
    }
    for bytes in FLD_STI_REJECTS {
        let loaded = Fixture::from_json(bytes).unwrap();
        let (_, result) = run(bytes, "interpreter", 4);
        assert_eq!(count(&result), 0);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert_eq!(snapshot(&result).state, loaded.fixture.initial_state);
    }
    let loaded = Fixture::from_json(FLD_STI_SIGNED_ZERO_WRAP).unwrap();
    let before = loaded.fixture.initial_state.x87.as_ref().unwrap();
    assert_eq!((before.status_word.get() >> 11) & 7, 0);
    assert_eq!((before.tag_word.get() >> 2) & 3, 1); // source ST(1)
    assert_eq!((before.tag_word.get() & 3), 3); // ST(0) is Empty
    let (_, result) = run(FLD_STI_SIGNED_ZERO_WRAP, "interpreter", 0);
    assert_eq!(count(&result), 1);
    let after = snapshot(&result).state.x87.as_ref().unwrap();
    assert_eq!((after.status_word.get() >> 11) & 7, 7);
    assert_eq!(after.registers[7], "00000000000000000080");
    assert_eq!((after.tag_word.get() >> 14) & 3, 1);
    let (_, jit) = run(FLD_STI_SIGNED_ZERO_WRAP, "jit", 4);
    assert!(matches!(jit.outcome, Outcome::Unsupported { .. }));
}

#[test]
fn local_x87_ffree_st0_marks_only_physical_top_and_composes_with_top_adjustment() {
    for (bytes, expected_tag, raw) in [
        (FFREE_ST0_ALREADY_EMPTY, 3u64, "07001020304050607080"),
        (FFREE_ST0_SNAN_OPAQUE, 3u64, "0100000000000080ff7f"),
        (FFREE_ST0_MALFORMED_OPAQUE, 3u64, "00000000000000000001"),
    ] {
        let loaded = Fixture::from_json(bytes).unwrap();
        let initial = loaded.fixture.initial_state.x87.as_ref().unwrap();
        let top = ((initial.status_word.get() >> 11) & 7) as usize;
        let (_, result) = run(bytes, "interpreter", 0);
        assert_eq!(count(&result), 2);
        let after = snapshot(&result).state.x87.as_ref().unwrap();
        assert_eq!(after.registers[top], raw);
        assert_eq!((after.tag_word.get() >> (top * 2)) & 3, expected_tag);
        assert_eq!(after.status_word.get(), initial.status_word.get());
        assert_eq!(
            after.instruction_pointer.get(),
            loaded.fixture.initial_state.rip.get()
        );
    }

    let loaded = Fixture::from_json(FFREE_ST0_FINCP_FLD1).unwrap();
    let (_, result) = run(FFREE_ST0_FINCP_FLD1, "interpreter", 0);
    assert_eq!(count(&result), 4);
    let after = snapshot(&result).state.x87.as_ref().unwrap();
    assert_eq!((after.status_word.get() >> 11) & 7, 7);
    assert_eq!(after.registers[7], "0000000000000080ff3f");
    assert_eq!((after.tag_word.get() >> 14) & 3, 0);
    assert_eq!(
        after.instruction_pointer.get(),
        loaded.fixture.initial_state.rip.get() + 4
    );

    for bytes in [FFREE_ST0_FLD_ST0_REJECT, FFREE_ST0_ENV_REJECT] {
        let loaded = Fixture::from_json(bytes).unwrap();
        let (_, result) = run(bytes, "interpreter", 4);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert_eq!(
            count(&result),
            if bytes == FFREE_ST0_FLD_ST0_REJECT {
                1
            } else {
                0
            }
        );
        if bytes == FFREE_ST0_ENV_REJECT {
            assert_eq!(snapshot(&result).state, loaded.fixture.initial_state);
        }
    }
    let (_, jit) = run(FFREE_ST0_ALREADY_EMPTY, "jit", 4);
    assert!(matches!(jit.outcome, Outcome::Unsupported { .. }));
}

#[test]
fn local_x87_ffree_sti_marks_only_indexed_physical_cell_and_denies_jit() {
    for (index, bytes) in FFREE_STI_FIXTURES.into_iter().enumerate() {
        let loaded = Fixture::from_json(bytes).unwrap();
        let initial = &loaded.fixture.initial_state;
        let (_, result) = run(bytes, "interpreter", 0);
        let retired = if index == 6 { 3 } else { 1 };
        assert_eq!(count(&result), retired);
        let after = snapshot(&result).state.x87.as_ref().unwrap();
        let before = initial.x87.as_ref().unwrap();
        let top = ((before.status_word.get() >> 11) & 7) as usize;
        let physical = (top + index + 1) & 7;
        if index < 6 {
            assert_eq!(after.registers, before.registers);
            assert_eq!(after.status_word, before.status_word);
            assert_eq!(after.control_word, before.control_word);
            assert_eq!(after.data_pointer, before.data_pointer);
            assert_eq!(after.last_opcode, before.last_opcode);
            assert_eq!(after.instruction_pointer.get(), initial.rip.get());
            assert_eq!((after.tag_word.get() >> (physical * 2)) & 3, 3);
            let changed = before.tag_word.get() ^ after.tag_word.get();
            assert_eq!(changed & !(3 << (physical * 2)), 0);
            assert_eq!(snapshot(&result).state.rip.get(), initial.rip.get() + 2);
            assert_eq!(snapshot(&result).state.gpr, initial.gpr);
            assert_eq!(snapshot(&result).state.rflags, initial.rflags);
        } else {
            assert_eq!(
                (before.tag_word.get() >> 12) & 3,
                2,
                "ST(7) target must start Special"
            );
            assert_ne!(before.registers[6], "0000000000000080ff3f");
            assert_eq!((after.status_word.get() >> 11) & 7, 6);
            assert_eq!(after.registers[6], "0000000000000080ff3f");
            assert_eq!((after.tag_word.get() >> 12) & 3, 0);
        }
        let (_, jit) = run(bytes, "jit", 4);
        assert!(matches!(jit.outcome, Outcome::Unsupported { .. }));
    }

    let loaded = Fixture::from_json(FFREE_STI_ENV_REJECT).unwrap();
    let (_, result) = run(FFREE_STI_ENV_REJECT, "interpreter", 4);
    assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
    assert_eq!(count(&result), 0);
    assert_eq!(snapshot(&result).state, loaded.fixture.initial_state);
}

#[test]
fn local_x87_fnop_preserves_full_snapshot_and_denies_jit() {
    for bytes in FNOP_FIXTURES {
        let loaded = Fixture::from_json(bytes).unwrap();
        let initial = &loaded.fixture.initial_state;
        let (_, result) = run(bytes, "interpreter", 0);
        let observed = snapshot(&result);
        assert_eq!(count(&result), loaded.fixture.instruction_budget);
        assert_eq!(
            observed.state.rip.get(),
            initial.rip.get() + u64::from(loaded.fixture.memory[0].length)
        );
        let mut expected_gpr = initial.gpr.clone();
        if loaded.fixture.case_id != "s2_30bs_fnop_top7_c1_opaque" {
            expected_gpr[0] = HexU64(
                (expected_gpr[0].get() & !0xffff)
                    | (observed.state.x87.as_ref().unwrap().status_word.get() & 0xffff),
            );
        }
        assert_eq!(observed.state.gpr, expected_gpr);
        assert_eq!(observed.state.rflags, initial.rflags);
        let before = initial.x87.as_ref().unwrap();
        let after = observed.state.x87.as_ref().unwrap();
        assert_eq!(after.control_word, before.control_word);
        assert_eq!(after.data_pointer, before.data_pointer);
        assert_eq!(after.last_opcode, before.last_opcode);
        if loaded.fixture.case_id != "s2_30bs_ffree_fnop_fld1" {
            assert_eq!(after.status_word, before.status_word);
        }
        if loaded.fixture.case_id == "s2_30bs_fnop_all_empty_fnstsw" {
            assert_eq!(after.registers, before.registers);
            assert_eq!(after.tag_word, before.tag_word);
            assert_eq!(after.status_word, before.status_word);
        }
        if loaded.fixture.case_id == "s2_30bs_repeated_fnop" {
            assert_eq!(after.instruction_pointer, HexU64(initial.rip.get() + 2));
            assert_eq!(after.registers, before.registers);
            assert_eq!(after.tag_word, before.tag_word);
        } else if loaded.fixture.case_id == "s2_30bs_ffree_fnop_fld1" {
            assert_eq!(after.instruction_pointer, HexU64(initial.rip.get() + 4));
            assert_eq!(after.status_word.get() >> 11 & 7, 6);
            assert_eq!(after.registers[6], "0000000000000080ff3f");
            assert_eq!(after.tag_word.get() >> 12 & 3, 0);
        } else {
            assert_eq!(after.instruction_pointer, initial.rip);
        }
        let (_, jit) = run(bytes, "jit", 4);
        assert!(matches!(jit.outcome, Outcome::Unsupported { .. }));
    }
    for bytes in FNOP_REJECT_FIXTURES {
        let loaded = Fixture::from_json(bytes).unwrap();
        let (_, result) = run(bytes, "interpreter", 4);
        assert_eq!(count(&result), 0);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert_eq!(snapshot(&result).state, loaded.fixture.initial_state);
    }
}

#[test]
fn local_x87_fxam_classifies_bounded_normal_and_zero_and_denies_jit() {
    for (bytes, (expected_status, expected_ip)) in FXAM_FIXTURES.into_iter().zip([
        (0x3e00, 0x0010_0000),
        (0x3e00, 0x0010_0000),
        (0x3e00, 0x0010_0002),
        (0x3c00, 0x0010_0002),
        (0x7800, 0x0010_0002),
        (0x3e00, 0x0010_0004),
        (0x7a00, 0x0010_0004),
    ]) {
        let loaded = Fixture::from_json(bytes).unwrap();
        let initial = &loaded.fixture.initial_state;
        let (_, result) = run(bytes, "interpreter", 0);
        let observed = snapshot(&result);
        assert_eq!(count(&result), loaded.fixture.instruction_budget);
        assert_eq!(
            observed.state.rip.get(),
            initial.rip.get() + u64::from(loaded.fixture.memory[0].length)
        );
        let after = observed.state.x87.as_ref().unwrap();
        assert_eq!(after.status_word.get(), expected_status);
        assert_eq!(after.instruction_pointer.get(), expected_ip);
        assert_eq!(
            after.control_word,
            initial.x87.as_ref().unwrap().control_word
        );
        assert_eq!(
            after.data_pointer,
            initial.x87.as_ref().unwrap().data_pointer
        );
        assert_eq!(after.last_opcode, initial.x87.as_ref().unwrap().last_opcode);
        if loaded.fixture.case_id.contains("fnstsw") || loaded.fixture.case_id.contains("producer")
        {
            assert_eq!(
                observed.state.gpr[0].get() & 0xffff,
                u64::from(expected_status)
            );
        }
        if loaded.fixture.case_id.contains("wrap")
            || loaded.fixture.case_id.contains("fnstsw")
            || loaded.fixture.case_id.contains("repeat")
        {
            assert_eq!(after.registers[7], "0000000000000080ffbf");
            assert_eq!(after.tag_word.get() >> 14 & 3, 0);
        }
        let (_, jit) = run(bytes, "jit", 4);
        assert!(matches!(jit.outcome, Outcome::Unsupported { .. }));
    }
    for bytes in FXAM_REJECT_FIXTURES {
        let loaded = Fixture::from_json(bytes).unwrap();
        let (_, result) = run(bytes, "interpreter", 4);
        assert_eq!(count(&result), 0);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert_eq!(snapshot(&result).state, loaded.fixture.initial_state);
    }
}

#[test]
fn local_x87_fcmovb_snapshots_and_denies_jit() {
    for bytes in FCMOVB_FIXTURES {
        let loaded = Fixture::from_json(bytes).unwrap();
        let (_, result) = run(bytes, "interpreter", 0);
        let observed = snapshot(&result);
        assert!(matches!(result.outcome, Outcome::StopAddress {}));
        assert_eq!(
            count(&result),
            (loaded.fixture.memory[0].initial_bytes_hex.len() / 4) as u64
        );
        assert_eq!(
            observed.state.rip.get(),
            loaded.fixture.initial_state.rip.get() + u64::from(loaded.fixture.memory[0].length)
        );
        let initial = loaded.fixture.initial_state.x87.as_ref().unwrap();
        let after = observed.state.x87.as_ref().unwrap();
        let top = ((initial.status_word.get() >> 11) & 7) as usize;
        let code = loaded.fixture.memory[0].initial_bytes_hex.as_str();
        let modrm_offset = code.rfind("da").unwrap();
        let modrm = u8::from_str_radix(&code[modrm_offset + 2..modrm_offset + 4], 16).unwrap();
        let source = (top + (modrm & 7) as usize) & 7;
        if (code.starts_with("dac") && loaded.fixture.initial_state.rflags.get() & 1 != 0)
            || code == "dac1dac1dfe0"
        {
            assert_eq!(after.registers[top], initial.registers[source]);
            assert_eq!(
                (after.tag_word.get() >> (top * 2)) & 3,
                (initial.tag_word.get() >> (source * 2)) & 3
            );
        }
        if loaded.fixture.case_id == "s2_30bu_fcmovb_producer_minus1_vs_plus1" {
            assert_eq!(after.registers[7], initial.registers[0]);
        }
        if loaded.fixture.case_id == "s2_30bu_fcmovb_producer_plus1_vs_minus1" {
            assert_eq!(after.registers[6], "0000000000000080ff3f");
        }
        if loaded.fixture.case_id == "s2_30bv_fcmovb_st0_top7_fnstsw" {
            let mut expected = initial.clone();
            expected.instruction_pointer = loaded.fixture.initial_state.rip;
            assert_eq!(
                after, &expected,
                "self-index must preserve opaque non-TOP cells"
            );
            for index in 0..7 {
                assert_eq!((initial.tag_word.get() >> (index * 2)) & 3, 2);
                assert_eq!(initial.registers[index], "a5".repeat(10));
            }
        }
        assert_eq!(after.status_word.get() & 0x82ff, 0);
        let fcmovb_offset = (modrm_offset / 2) as u64;
        assert_eq!(
            after.instruction_pointer.get(),
            loaded.fixture.initial_state.rip.get() + fcmovb_offset
        );
        let (_, jit) = run(bytes, "jit", 4);
        assert!(matches!(jit.outcome, Outcome::Unsupported { .. }));
    }
    for bytes in FCMOVB_REJECTS {
        let loaded = Fixture::from_json(bytes).unwrap();
        if matches!(
            loaded.fixture.case_id.as_str(),
            "s2_30bu_fcmovb_c1_reject"
                | "s2_30bu_fcmovb_bad_fcw_reject"
                | "s2_30bv_fcmovb_st7_c1_reject"
                | "s2_30bv_fcmovb_st0_bad_fcw_reject"
        ) {
            let x87 = loaded.fixture.initial_state.x87.as_ref().unwrap();
            assert_eq!(x87.status_word.get() >> 11 & 7, 0);
            assert_eq!(x87.tag_word.get() & 0xf, 0x5);
            assert_eq!(x87.registers[0], "00000000000000000000");
            assert_eq!(x87.registers[1], "00000000000000000000");
            assert_eq!(
                x87.control_word.get() == 0x037f,
                loaded.fixture.case_id == "s2_30bu_fcmovb_c1_reject"
                    || loaded.fixture.case_id == "s2_30bv_fcmovb_st7_c1_reject"
            );
        }
        if loaded.fixture.case_id == "s2_30bv_fcmovb_st7_c1_reject" {
            let x87 = loaded.fixture.initial_state.x87.as_ref().unwrap();
            assert_eq!((x87.tag_word.get() >> 14) & 3, 0);
            assert_eq!(x87.registers[7], "0000000000000080ff3f");
        }
        let (_, result) = run(bytes, "interpreter", 4);
        assert_eq!(count(&result), 0);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert_eq!(snapshot(&result).state, loaded.fixture.initial_state);
    }
}

#[test]
fn local_x87_fcmovnb_snapshots_and_denies_jit() {
    for bytes in FCMOVNB_FIXTURES {
        let loaded = Fixture::from_json(bytes).unwrap();
        let (_, result) = run(bytes, "interpreter", 0);
        let observed = snapshot(&result);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{}",
            loaded.fixture.case_id
        );
        assert_eq!(
            count(&result),
            (loaded.fixture.memory[0].initial_bytes_hex.len() / 4) as u64
        );
        let before = loaded.fixture.initial_state.x87.as_ref().unwrap();
        let after = observed.state.x87.as_ref().unwrap();
        let code = &loaded.fixture.memory[0].initial_bytes_hex;
        if loaded.fixture.case_id == "s2_30bw_fcmovnb_taken_negative_zero" {
            assert_eq!(after.registers[0], before.registers[1]);
            assert_eq!(after.tag_word.get() & 3, (before.tag_word.get() >> 2) & 3);
        }
        if loaded.fixture.case_id == "s2_30bw_fcmovnb_untaken_distinct_tags" {
            assert_eq!(after.registers, before.registers);
            assert_eq!(after.tag_word, before.tag_word);
        }
        if loaded.fixture.case_id == "s2_30bw_fcmovnb_top7_wrap" {
            assert_eq!(after.registers[7], before.registers[0]);
            assert_eq!(after.tag_word.get() >> 14 & 3, before.tag_word.get() & 3);
        }
        if loaded.fixture.case_id == "s2_30bw_fcmovnb_producer_minus1_below_plus1" {
            assert_ne!(
                observed.state.rflags.get() & 1,
                0,
                "FCOMI 0,1 must leave CF=1"
            );
            assert_eq!(
                after.registers[6], "0000000000000080ffbf",
                "CF=1 must leave TOP untouched"
            );
        }
        if loaded.fixture.case_id == "s2_30bw_fcmovnb_producer_plus1_above_minus1" {
            assert_eq!(
                observed.state.rflags.get() & 1,
                0,
                "FCOMI 1,0 must leave CF=0"
            );
            assert_eq!(
                after.registers[6], "0000000000000080ffbf",
                "CF=0 must copy the produced negative one"
            );
            assert_eq!(
                after.tag_word.get() >> 12 & 3,
                0,
                "copied source must retain Valid tag"
            );
        }
        if loaded.fixture.case_id.starts_with("s2_30bx_fcmovnb_") {
            let index = u8::from_str_radix(&code[2..4], 16).unwrap() & 7;
            let top = ((before.status_word.get() >> 11) & 7) as u8;
            let source = (top + index) & 7;
            let mut expected = before.clone();
            if loaded.fixture.initial_state.rflags.get() & 1 == 0 {
                expected.registers[usize::from(top)] =
                    before.registers[usize::from(source)].clone();
                let tag = (before.tag_word.get() >> (u32::from(source) * 2)) & 3;
                let tag_word = expected.tag_word.get();
                let mask = 3u64 << (u32::from(top) * 2);
                expected.tag_word = HexU64((tag_word & !mask) | (tag << (u32::from(top) * 2)));
            }
            expected.instruction_pointer = loaded.fixture.initial_state.rip;
            assert_eq!(after, &expected, "{}", loaded.fixture.case_id);
        }
        let fcmov_offset = (code.rfind("db").unwrap() / 2) as u64;
        assert_eq!(
            after.instruction_pointer.get(),
            loaded.fixture.initial_state.rip.get() + fcmov_offset
        );
        assert_eq!(after.status_word.get() & 0x82ff, 0);
        let (_, jit) = run(bytes, "jit", 4);
        assert!(matches!(jit.outcome, Outcome::Unsupported { .. }));
    }
    for bytes in FCMOVNB_REJECTS {
        let loaded = Fixture::from_json(bytes).unwrap();
        let (_, result) = run(bytes, "interpreter", 4);
        assert_eq!(count(&result), 0);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert_eq!(snapshot(&result).state, loaded.fixture.initial_state);
    }
}

#[test]
fn local_x87_fcmove_snapshots_and_denies_jit() {
    for bytes in FCMOVE_FIXTURES {
        let loaded = Fixture::from_json(bytes).unwrap();
        let (_, result) = run(bytes, "interpreter", 0);
        let observed = snapshot(&result);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{}",
            loaded.fixture.case_id
        );
        assert_eq!(
            count(&result),
            (loaded.fixture.memory[0].initial_bytes_hex.len() / 4) as u64
        );
        let before = loaded.fixture.initial_state.x87.as_ref().unwrap();
        let after = observed.state.x87.as_ref().unwrap();
        let code_hex = &loaded.fixture.memory[0].initial_bytes_hex;
        let fcmove_offset = code_hex.rfind("da").unwrap() as u64 / 2;
        let mut expected = before.clone();
        let mut expected_flags = loaded.fixture.initial_state.rflags;
        let producer_equal = loaded.fixture.case_id == "s2_30by_fcmove_producer_equal_then_fchs";
        let producer_unequal =
            loaded.fixture.case_id == "s2_30by_fcmove_producer_unequal_after_fchs";
        if producer_equal || producer_unequal {
            // These producer streams have a hand-computed final state: FLD1
            // twice leaves TOP=6, and only the unequal stream applies FCHS
            // before FCOMI. FCMOVE then copies on equality only.
            expected.registers[6] = if producer_equal {
                "0000000000000080ff3f".into()
            } else {
                "0000000000000080ffbf".into()
            };
            expected.registers[7] = "0000000000000080ff3f".into();
            expected.status_word = HexU64(0x3000);
            expected.tag_word = HexU64(0x0ff4);
            expected.instruction_pointer = HexU64(loaded.fixture.initial_state.rip.get() + 8);
            expected_flags = HexU64(if producer_equal { 0x42 } else { 0x03 });
        } else {
            let top = ((expected.status_word.get() >> 11) & 7) as usize;
            let move_index = u8::from_str_radix(
                &code_hex[(fcmove_offset * 2 + 2) as usize..(fcmove_offset * 2 + 4) as usize],
                16,
            )
            .unwrap()
                & 7;
            let source = (top + usize::from(move_index)) & 7;
            let moves = if loaded.fixture.case_id == "s2_30by_fcmove_repeat" {
                2
            } else {
                1
            };
            for _ in 0..moves {
                if expected_flags.get() & 0x40 != 0 {
                    expected.registers[top] = expected.registers[source].clone();
                    let source_tag = (expected.tag_word.get() >> (source * 2)) & 3;
                    let mask = 3u64 << (top * 2);
                    expected.tag_word =
                        HexU64((expected.tag_word.get() & !mask) | (source_tag << (top * 2)));
                }
            }
            expected.instruction_pointer =
                HexU64(loaded.fixture.initial_state.rip.get() + fcmove_offset);
        }
        assert_eq!(
            observed.state.rflags, expected_flags,
            "{}",
            loaded.fixture.case_id
        );
        assert_eq!(after, &expected, "{}", loaded.fixture.case_id);
        assert_eq!(after.status_word.get() & 0x82ff, 0);
        assert_eq!(
            after.instruction_pointer.get(),
            loaded.fixture.initial_state.rip.get() + fcmove_offset
        );
        let (_, jit) = run(bytes, "jit", 4);
        assert!(matches!(jit.outcome, Outcome::Unsupported { .. }));
    }
    for bytes in FCMOVE_REJECTS {
        let loaded = Fixture::from_json(bytes).unwrap();
        let (_, result) = run(bytes, "interpreter", 4);
        assert_eq!(count(&result), 0, "{}", loaded.fixture.case_id);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert_eq!(snapshot(&result).state, loaded.fixture.initial_state);
    }
}

#[test]
fn local_x87_fcmovne_fixtures_check_raw_copy_and_untaken_state_and_deny_jit() {
    for bytes in FCMOVNE_FIXTURES {
        let loaded = Fixture::from_json(bytes).unwrap();
        let (_, result) = run(bytes, "interpreter", 0);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{}",
            loaded.fixture.case_id
        );
        assert_eq!(
            count(&result),
            loaded.fixture.instruction_budget,
            "{}",
            loaded.fixture.case_id
        );
        let observed = snapshot(&result);
        let before = loaded.fixture.initial_state.x87.as_ref().unwrap();
        let after = observed.state.x87.as_ref().unwrap();
        let mut expected = before.clone();
        let code_hex = &loaded.fixture.memory[0].initial_bytes_hex;
        let move_offset = (code_hex.rfind("db").unwrap() / 2) as u64;
        let producer_equal = loaded.fixture.case_id == "s2_30cb_fcmovne_producer_equal_then_fchs";
        let producer_unequal =
            loaded.fixture.case_id == "s2_30cb_fcmovne_producer_unequal_after_fchs";
        let mut expected_flags = loaded.fixture.initial_state.rflags;
        if producer_equal || producer_unequal {
            expected.registers[6] = if producer_equal {
                "0000000000000080ffbf".into()
            } else {
                "0000000000000080ff3f".into()
            };
            expected.registers[7] = "0000000000000080ff3f".into();
            expected.status_word = HexU64(0x3000);
            expected.tag_word = HexU64(0x0ff4);
            expected_flags = HexU64(if producer_equal { 0x42 } else { 0x03 });
        } else {
            let top = ((expected.status_word.get() >> 11) & 7) as usize;
            let modrm_offset = move_offset as usize * 2 + 2;
            let source_index =
                u8::from_str_radix(&code_hex[modrm_offset..modrm_offset + 2], 16).unwrap() & 7;
            let source = (top + usize::from(source_index)) & 7;
            let moves = if loaded.fixture.case_id == "s2_30cb_fcmovne_repeat" {
                2
            } else {
                1
            };
            for _ in 0..moves {
                if loaded.fixture.initial_state.rflags.get() & 0x40 == 0 {
                    expected.registers[top] = expected.registers[source].clone();
                    let tag = (expected.tag_word.get() >> (source * 2)) & 3;
                    let mask = 3u64 << (top * 2);
                    expected.tag_word =
                        HexU64((expected.tag_word.get() & !mask) | (tag << (top * 2)));
                }
            }
        }
        expected.instruction_pointer = HexU64(loaded.fixture.initial_state.rip.get() + move_offset);
        assert_eq!(after, &expected, "{}", loaded.fixture.case_id);
        assert_eq!(observed.state.rflags, expected_flags);
        let expected_memory = loaded
            .fixture
            .memory
            .iter()
            .map(|region| MemoryRegionSnapshot {
                region_id: region.region_id.clone(),
                guest_address: region.guest_address,
                bytes_hex: region.initial_bytes_hex.to_ascii_lowercase(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            observed.memory, expected_memory,
            "{}",
            loaded.fixture.case_id
        );
        let (_, jit) = run(bytes, "jit", 4);
        assert!(matches!(jit.outcome, Outcome::Unsupported { .. }));
    }
    for bytes in FCMOVNE_REJECTS {
        let loaded = Fixture::from_json(bytes).unwrap();
        let (_, result) = run(bytes, "interpreter", 4);
        assert_eq!(count(&result), 0, "{}", loaded.fixture.case_id);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert_eq!(snapshot(&result).state, loaded.fixture.initial_state);
        let (_, jit) = run(bytes, "jit", 4);
        assert!(matches!(jit.outcome, Outcome::Unsupported { .. }));
    }
}

#[test]
fn addr32_lods_fixtures_match_literal_full_state_answers_and_fail_closed() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &fs::read(directory.join("s2_18k_lods_addr32_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 39);
    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let expected_exit = answer["expected_exit"].as_str().unwrap();
        let is_harness_error = expected_exit == "harness_error";
        let (input, result) = run(&raw, "interpreter", if is_harness_error { 5 } else { 0 });
        assert_eq!(input.fixture.case_id, case_id);
        match expected_exit {
            "stop_address" => assert!(
                matches!(result.outcome, Outcome::StopAddress {}),
                "{case_id}"
            ),
            "instruction_budget_exhausted" => assert!(
                matches!(result.outcome, Outcome::InstructionBudgetExhausted {}),
                "{case_id}"
            ),
            "harness_error" => assert!(
                matches!(result.outcome, Outcome::HarnessError { .. }),
                "{case_id}"
            ),
            other => panic!("unexpected S2-18k exit {other} for {case_id}"),
        }
        assert_eq!(
            count(&result),
            answer["retired"].as_u64().unwrap(),
            "{case_id}"
        );

        let observed = snapshot(&result);
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gpr, "{case_id}");
        assert_eq!(observed.state.rflags, expected_flags, "{case_id}");
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        assert_eq!(observed.state.rip, expected_rip, "{case_id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}"
            );
        }
        if is_harness_error {
            let reason = match &result.outcome {
                Outcome::HarnessError { reason } => reason,
                _ => unreachable!(),
            };
            assert!(reason.contains("access: Read"), "{case_id}: {reason}");
            let address = answer["fault_address"].as_str().unwrap();
            let address_value = u64::from_str_radix(address.trim_start_matches("0x"), 16).unwrap();
            assert!(
                reason.contains(&format!("address: {address_value}")),
                "{case_id}: {reason}"
            );
            assert_eq!(
                observed.state.gpr, input.fixture.initial_state.gpr,
                "faulting LODS must not commit its accumulator or pointer: {case_id}"
            );
        }

        let (_, jit_result) = run(&raw, "jit", 4);
        assert!(
            matches!(jit_result.outcome, Outcome::Unsupported { .. }),
            "{case_id}"
        );
        assert!(
            matches!(
                &jit_result.instructions_executed,
                Observation::Unavailable { reason } if reason == "engine was not executed"
            ),
            "{case_id}"
        );
        assert!(
            matches!(jit_result.snapshot, Observation::Unavailable { .. }),
            "{case_id}"
        );
    }
}

#[test]
fn local_x87_fcmovbe_fixtures_check_full_state_producers_and_deny_jit() {
    let one = "0000000000000080ff3f";
    let minus_one = "0000000000000080ffbf";
    for bytes in FCMOVBE_FIXTURES {
        let loaded = Fixture::from_json(bytes).unwrap();
        let (_, result) = run(bytes, "interpreter", 0);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{}",
            loaded.fixture.case_id
        );
        assert_eq!(
            count(&result),
            loaded.fixture.instruction_budget,
            "{}",
            loaded.fixture.case_id
        );
        let observed = snapshot(&result);
        let mut expected = loaded.fixture.initial_state.clone();
        let expected_x87 = expected.x87.as_mut().unwrap();
        let code = &loaded.fixture.memory[0].initial_bytes_hex;
        let producer = loaded
            .fixture
            .case_id
            .starts_with("s2_30cc_fcmovbe_producer_");
        let needle = if producer { "dad1" } else { "da" };
        let move_offset = (code.rfind(needle).unwrap() / 2) as u64;

        if producer {
            expected_x87.registers[6] = one.into();
            expected_x87.registers[7] = if loaded.fixture.case_id.ends_with("greater") {
                minus_one.into()
            } else {
                one.into()
            };
            expected_x87.status_word = HexU64(0x3000);
            expected_x87.tag_word = HexU64(0x0ff4);
            expected.rflags = HexU64(match loaded.fixture.case_id.as_str() {
                "s2_30cc_fcmovbe_producer_greater" => 0x02,
                "s2_30cc_fcmovbe_producer_equal_then_fchs" => 0x42,
                "s2_30cc_fcmovbe_producer_less" => 0x03,
                _ => unreachable!(),
            });
            expected.gpr[0] = HexU64((expected.gpr[0].get() & !0xffff) | 0x3000);
            if loaded.fixture.case_id.ends_with("equal_then_fchs")
                || loaded.fixture.case_id.ends_with("less")
            {
                expected_x87.registers[6] = one.into();
            }
        } else {
            let top = ((expected_x87.status_word.get() >> 11) & 7) as usize;
            let source_index = u8::from_str_radix(
                &code[move_offset as usize * 2 + 2..move_offset as usize * 2 + 4],
                16,
            )
            .unwrap()
                & 7;
            let source = (top + usize::from(source_index)) & 7;
            if expected.rflags.get() & 0x41 != 0 {
                let raw = expected_x87.registers[source].clone();
                expected_x87.registers[top] = raw;
                let source_tag = (expected_x87.tag_word.get() >> (source * 2)) & 3;
                let tag_mask = 3u64 << (top * 2);
                expected_x87.tag_word =
                    HexU64((expected_x87.tag_word.get() & !tag_mask) | (source_tag << (top * 2)));
            }
            if code.ends_with("dfe0") {
                expected.gpr[0] = HexU64(
                    (expected.gpr[0].get() & !0xffff) | (expected_x87.status_word.get() & 0xffff),
                );
            }
        }
        expected_x87.instruction_pointer =
            HexU64(loaded.fixture.initial_state.rip.get() + move_offset);
        expected.rip = HexU64(loaded.fixture.initial_state.rip.get() + (code.len() / 2) as u64);
        assert_eq!(observed.state, expected, "{}", loaded.fixture.case_id);
        let expected_memory = loaded
            .fixture
            .memory
            .iter()
            .map(|region| MemoryRegionSnapshot {
                region_id: region.region_id.clone(),
                guest_address: region.guest_address,
                bytes_hex: region.initial_bytes_hex.to_ascii_lowercase(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            observed.memory, expected_memory,
            "{}",
            loaded.fixture.case_id
        );
        let (_, jit) = run(bytes, "jit", 4);
        assert!(
            matches!(jit.outcome, Outcome::Unsupported { .. }),
            "{}",
            loaded.fixture.case_id
        );
    }
    for bytes in FCMOVBE_REJECTS {
        let loaded = Fixture::from_json(bytes).unwrap();
        let (_, result) = run(bytes, "interpreter", 4);
        assert_eq!(count(&result), 0, "{}", loaded.fixture.case_id);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert_eq!(snapshot(&result).state, loaded.fixture.initial_state);
        let (_, jit) = run(bytes, "jit", 4);
        assert!(matches!(jit.outcome, Outcome::Unsupported { .. }));
    }
}

#[test]
fn local_x87_fcmovu_test_pf_fixtures_check_full_state_and_deny_jit() {
    let one = "0000000000000080ff3f";
    for bytes in FCMOVU_FIXTURES {
        let loaded = Fixture::from_json(bytes).unwrap();
        let (_, result) = run(bytes, "interpreter", 0);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{}",
            loaded.fixture.case_id
        );
        assert_eq!(count(&result), 5, "{}", loaded.fixture.case_id);
        let observed = snapshot(&result);
        let mut expected = loaded.fixture.initial_state.clone();
        let pf = expected.gpr[0].get() == 3;
        assert!(matches!(expected.gpr[0].get(), 1 | 3));
        let expected_x87 = expected.x87.as_mut().unwrap();
        expected_x87.registers[7] = one.into();
        expected_x87.registers[6] = if pf { one } else { "00000000000000000000" }.into();
        let mut tag_word = expected_x87.tag_word.get();
        tag_word &= !(3 << (7 * 2));
        tag_word = (tag_word & !(3 << (6 * 2))) | if pf { 0 } else { 1 << (6 * 2) };
        expected_x87.tag_word = HexU64(tag_word);
        expected_x87.status_word = HexU64((expected_x87.status_word.get() & !0x3800) | 0x3000);
        expected_x87.instruction_pointer = HexU64(loaded.fixture.initial_state.rip.get() + 7);
        expected.gpr[0] = HexU64((expected.gpr[0].get() & !0xffff) | 0x3000);
        expected.rflags = HexU64((expected.rflags.get() & !0x8d5) | if pf { 0x4 } else { 0 });
        expected.rip = HexU64(loaded.fixture.initial_state.rip.get() + 11);
        assert_eq!(observed.state, expected, "{}", loaded.fixture.case_id);
        let expected_memory = loaded
            .fixture
            .memory
            .iter()
            .map(|region| MemoryRegionSnapshot {
                region_id: region.region_id.clone(),
                guest_address: region.guest_address,
                bytes_hex: region.initial_bytes_hex.to_ascii_lowercase(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            observed.memory, expected_memory,
            "{}",
            loaded.fixture.case_id
        );
        let (_, jit) = run(bytes, "jit", 4);
        assert!(
            matches!(jit.outcome, Outcome::Unsupported { .. }),
            "{}",
            loaded.fixture.case_id
        );
    }
}

#[test]
fn local_x87_indexed_fcmovu_fixtures_check_full_state_and_deny_jit() {
    for bytes in FCMOVU_INDEXED_FIXTURES {
        let loaded = Fixture::from_json(bytes).unwrap();
        let (_, result) = run(bytes, "interpreter", 0);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{}",
            loaded.fixture.case_id
        );
        assert_eq!(
            count(&result),
            loaded.fixture.instruction_budget,
            "{}",
            loaded.fixture.case_id
        );
        let observed = snapshot(&result);
        let mut expected = loaded.fixture.initial_state.clone();
        let code = &loaded.fixture.memory[0].initial_bytes_hex;
        let top = ((expected.x87.as_ref().unwrap().status_word.get() >> 11) & 7) as usize;
        let source_index = u8::from_str_radix(&code[2..4], 16).unwrap() & 7;
        let source = (top + usize::from(source_index)) & 7;
        let instruction_count = loaded.fixture.instruction_budget;
        let fcmovu_offset = if instruction_count == 3 { 2 } else { 0 };
        let pf = expected.rflags.get() & 4 != 0;
        let case_id = loaded.fixture.case_id.as_str();
        let expected_pf = !case_id.ends_with("st0_pf0");
        assert_eq!(pf, expected_pf, "{}", loaded.fixture.case_id);
        let expected_x87 = expected.x87.as_mut().unwrap();
        let source_bytes = expected_x87.registers[source].clone();
        let source_tag = (expected_x87.tag_word.get() >> (source * 2)) & 3;
        if pf {
            if source_index != 0 {
                assert_ne!(
                    expected_x87.registers[top], source_bytes,
                    "fixture must make the taken copy observable: {}",
                    loaded.fixture.case_id
                );
            }
            expected_x87.registers[top] = expected_x87.registers[source].clone();
            let tag_mask = !(3u64 << (top * 2));
            expected_x87.tag_word =
                HexU64((expected_x87.tag_word.get() & tag_mask) | (source_tag << (top * 2)));
            assert_eq!(expected_x87.registers[top], source_bytes);
            assert_eq!((expected_x87.tag_word.get() >> (top * 2)) & 3, source_tag);
        }
        expected_x87.instruction_pointer =
            HexU64(loaded.fixture.initial_state.rip.get() + fcmovu_offset);
        if instruction_count == 3 {
            expected.gpr[0] = HexU64(
                (expected.gpr[0].get() & !0xffff) | (expected_x87.status_word.get() & 0xffff),
            );
        }
        expected.rip =
            HexU64(loaded.fixture.initial_state.rip.get() + u64::from(instruction_count) * 2);
        assert_eq!(observed.state, expected, "{}", loaded.fixture.case_id);
        let expected_memory = loaded
            .fixture
            .memory
            .iter()
            .map(|region| MemoryRegionSnapshot {
                region_id: region.region_id.clone(),
                guest_address: region.guest_address,
                bytes_hex: region.initial_bytes_hex.to_ascii_lowercase(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            observed.memory, expected_memory,
            "{}",
            loaded.fixture.case_id
        );
        let (_, jit) = run(bytes, "jit", 4);
        assert!(
            matches!(jit.outcome, Outcome::Unsupported { .. }),
            "{}",
            loaded.fixture.case_id
        );
    }
    for bytes in FCMOVU_REJECTS {
        let loaded = Fixture::from_json(bytes).unwrap();
        let pf = loaded.fixture.initial_state.rflags.get() & 4 != 0;
        assert_eq!(
            pf,
            !loaded.fixture.case_id.ends_with("invalid_source_pf0"),
            "{}",
            loaded.fixture.case_id
        );
        let (_, result) = run(bytes, "interpreter", 4);
        assert_eq!(count(&result), 0, "{}", loaded.fixture.case_id);
        assert!(
            matches!(result.outcome, Outcome::Unsupported { .. }),
            "{}",
            loaded.fixture.case_id
        );
        assert_eq!(
            snapshot(&result).state,
            loaded.fixture.initial_state,
            "{}",
            loaded.fixture.case_id
        );
        let expected_memory = loaded
            .fixture
            .memory
            .iter()
            .map(|region| MemoryRegionSnapshot {
                region_id: region.region_id.clone(),
                guest_address: region.guest_address,
                bytes_hex: region.initial_bytes_hex.to_ascii_lowercase(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            snapshot(&result).memory,
            expected_memory,
            "{}",
            loaded.fixture.case_id
        );
        let (_, jit) = run(bytes, "jit", 4);
        assert!(
            matches!(jit.outcome, Outcome::Unsupported { .. }),
            "{}",
            loaded.fixture.case_id
        );
    }
}

#[test]
fn local_x87_indexed_fcmovnu_fixtures_deny_jit_for_success_and_reject_rows() {
    for bytes in FCMOVNU_INDEXED_FIXTURES.into_iter().chain(FCMOVNU_REJECTS) {
        let loaded = Fixture::from_json(bytes).unwrap();
        let (_, jit) = run(bytes, "jit", 4);
        assert!(
            matches!(jit.outcome, Outcome::Unsupported { .. }),
            "{}",
            loaded.fixture.case_id
        );
    }
}

#[test]
fn local_x87_fcmovnbe_fixtures_check_full_state_producers_and_deny_jit() {
    let one = "0000000000000080ff3f";
    let minus_one = "0000000000000080ffbf";
    for bytes in FCMOVNBE_FIXTURES {
        let loaded = Fixture::from_json(bytes).unwrap();
        let (_, result) = run(bytes, "interpreter", 0);
        assert!(
            matches!(result.outcome, Outcome::StopAddress {}),
            "{}",
            loaded.fixture.case_id
        );
        assert_eq!(
            count(&result),
            loaded.fixture.instruction_budget,
            "{}",
            loaded.fixture.case_id
        );
        let observed = snapshot(&result);
        let mut expected = loaded.fixture.initial_state.clone();
        let expected_x87 = expected.x87.as_mut().unwrap();
        let code = &loaded.fixture.memory[0].initial_bytes_hex;
        let producer = loaded
            .fixture
            .case_id
            .starts_with("s2_30ce_fcmovnbe_producer_");
        let move_byte_offset = code.rfind("dbd").unwrap();
        let source_index =
            u8::from_str_radix(&code[move_byte_offset + 3..move_byte_offset + 4], 16).unwrap();
        let move_offset = (move_byte_offset / 2) as u64;

        if producer {
            expected_x87.registers[6] = minus_one.into();
            expected_x87.registers[7] = if loaded.fixture.case_id.ends_with("greater") {
                minus_one.into()
            } else {
                one.into()
            };
            expected_x87.status_word = HexU64(0x3000);
            expected_x87.tag_word = HexU64(0x0ff4);
            expected.rflags = HexU64(match loaded.fixture.case_id.as_str() {
                "s2_30ce_fcmovnbe_producer_greater" => 0x02,
                "s2_30ce_fcmovnbe_producer_equal_then_fchs" => 0x42,
                "s2_30ce_fcmovnbe_producer_less" => 0x03,
                _ => unreachable!(),
            });
            expected.gpr[0] = HexU64((expected.gpr[0].get() & !0xffff) | 0x3000);
            if loaded.fixture.case_id.ends_with("equal_then_fchs")
                || loaded.fixture.case_id.ends_with("less")
            {
                expected_x87.registers[6] = minus_one.into();
            }
        } else {
            let top = ((expected_x87.status_word.get() >> 11) & 7) as usize;
            let source = (top + usize::from(source_index)) & 7;
            if expected.rflags.get() & 0x41 == 0 {
                let raw = expected_x87.registers[source].clone();
                expected_x87.registers[top] = raw;
                let source_tag = (expected_x87.tag_word.get() >> (source * 2)) & 3;
                let tag_mask = 3u64 << (top * 2);
                expected_x87.tag_word =
                    HexU64((expected_x87.tag_word.get() & !tag_mask) | (source_tag << (top * 2)));
            }
            if code.ends_with("dfe0") {
                expected.gpr[0] = HexU64(
                    (expected.gpr[0].get() & !0xffff) | (expected_x87.status_word.get() & 0xffff),
                );
            }
        }
        expected_x87.instruction_pointer =
            HexU64(loaded.fixture.initial_state.rip.get() + move_offset);
        expected.rip = HexU64(
            loaded.fixture.initial_state.rip.get()
                + (loaded.fixture.memory[0].initial_bytes_hex.len() / 2) as u64,
        );
        assert_eq!(observed.state, expected, "{}", loaded.fixture.case_id);
        let expected_memory = loaded
            .fixture
            .memory
            .iter()
            .map(|region| MemoryRegionSnapshot {
                region_id: region.region_id.clone(),
                guest_address: region.guest_address,
                bytes_hex: region.initial_bytes_hex.to_ascii_lowercase(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            observed.memory, expected_memory,
            "{}",
            loaded.fixture.case_id
        );
        let (_, jit) = run(bytes, "jit", 4);
        assert!(
            matches!(jit.outcome, Outcome::Unsupported { .. }),
            "{}",
            loaded.fixture.case_id
        );
    }
    for bytes in FCMOVNBE_REJECTS {
        let loaded = Fixture::from_json(bytes).unwrap();
        let (_, result) = run(bytes, "interpreter", 4);
        assert_eq!(count(&result), 0, "{}", loaded.fixture.case_id);
        assert!(matches!(result.outcome, Outcome::Unsupported { .. }));
        assert_eq!(snapshot(&result).state, loaded.fixture.initial_state);
        let expected_memory = loaded
            .fixture
            .memory
            .iter()
            .map(|region| MemoryRegionSnapshot {
                region_id: region.region_id.clone(),
                guest_address: region.guest_address,
                bytes_hex: region.initial_bytes_hex.to_ascii_lowercase(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            snapshot(&result).memory,
            expected_memory,
            "{}",
            loaded.fixture.case_id
        );
        let (_, jit) = run(bytes, "jit", 4);
        assert!(matches!(jit.outcome, Outcome::Unsupported { .. }));
    }
}

#[test]
fn scas_addr32_cli_fixtures_match_literal_full_state_and_keep_jit_closed() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2");
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &fs::read(directory.join("s2_18m_scas_addr32_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 39);

    for (case_id, answer) in answers {
        let raw = fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let expected_exit = answer["expected_exit"].as_str().unwrap();
        let is_harness_error = expected_exit == "harness_error";
        let (input, result) = run(&raw, "interpreter", if is_harness_error { 5 } else { 0 });
        assert_eq!(input.fixture.case_id, case_id);

        match expected_exit {
            "stop_address" => {
                assert_eq!(
                    input.fixture.expected_exit,
                    ExpectedExit::StopAddress,
                    "{case_id}"
                );
                assert!(
                    matches!(result.outcome, Outcome::StopAddress {}),
                    "{case_id}"
                );
            }
            "instruction_budget_exhausted" => {
                assert_eq!(
                    input.fixture.expected_exit,
                    ExpectedExit::InstructionBudgetExhausted,
                    "{case_id}"
                );
                assert!(
                    matches!(result.outcome, Outcome::InstructionBudgetExhausted {}),
                    "{case_id}"
                );
            }
            "harness_error" => {
                assert_eq!(
                    input.fixture.expected_exit,
                    ExpectedExit::HarnessError,
                    "{case_id}"
                );
                let reason = match &result.outcome {
                    Outcome::HarnessError { reason } => reason,
                    other => panic!("{case_id}: expected HarnessError, got {other:?}"),
                };
                let error = &answer["harness_error"];
                let kind = error["kind"].as_str().unwrap();
                let access = error["access"].as_str().unwrap();
                let address = error["address"].as_str().unwrap();
                let address_value =
                    u64::from_str_radix(address.trim_start_matches("0x"), 16).unwrap();
                assert_eq!(kind, "Unmapped", "{case_id}");
                assert_eq!(access, "Read", "{case_id}");
                assert_eq!(
                    reason,
                    &format!(
                        "fixture memory backend: {kind} {{ address: {address_value}, access: {access} }}"
                    ),
                    "{case_id}"
                );
            }
            other => panic!("unexpected S2-18m exit {other} for {case_id}"),
        }
        assert_eq!(
            count(&result),
            answer["retired"].as_u64().unwrap(),
            "{case_id}"
        );

        let observed = snapshot(&result);
        let expected_gprs: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(observed.state.gpr, expected_gprs, "{case_id}");
        assert_eq!(
            observed.state.rflags.get() & expected_defined.get(),
            expected_flags.get() & expected_defined.get(),
            "{case_id}"
        );
        assert_eq!(observed.rflags_defined, expected_defined, "{case_id}");
        assert_eq!(observed.state.rip, expected_rip, "{case_id}");

        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        assert_eq!(observed.memory.len(), expected_memory.len(), "{case_id}");
        for region in &observed.memory {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}: {}",
                region.region_id
            );
            if is_harness_error || expected_exit == "instruction_budget_exhausted" {
                let initial = input
                    .fixture
                    .memory
                    .iter()
                    .find(|memory| memory.region_id == region.region_id)
                    .unwrap();
                assert_eq!(region.bytes_hex, initial.initial_bytes_hex, "{case_id}");
            }
        }
        if (is_harness_error || expected_exit == "instruction_budget_exhausted")
            && answer["retired"].as_u64() == Some(0)
        {
            assert_eq!(
                observed.state.gpr, input.fixture.initial_state.gpr,
                "{case_id}"
            );
            assert_eq!(
                observed.state.rflags, input.fixture.initial_state.rflags,
                "{case_id}"
            );
            assert_eq!(
                observed.state.rip, input.fixture.initial_state.rip,
                "{case_id}"
            );
        }

        let (_, jit) = run(&raw, "jit", 4);
        assert!(
            matches!(jit.outcome, Outcome::Unsupported { .. }),
            "{case_id}"
        );
        assert!(
            matches!(jit.snapshot, Observation::Unavailable { .. }),
            "{case_id}"
        );
        assert!(
            matches!(jit.instructions_executed, Observation::Unavailable { .. }),
            "{case_id}"
        );
    }
}
