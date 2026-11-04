//! Opt-in comparison of the existing fixed-home allocator at budgets four and zero.
//!
//! This is a single-block diagnostic. It uses no translation cache and retains
//! only one MAP_JIT publication at a time.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::AtomicU8;
use std::time::{Duration, Instant};

use od_host::PublishedCode;
use od_isa::LongModeState;
use od_isa::execute::{RunConfig, RunExit, RunResult, run as interpreter_run};
use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError};
use od_jit::allocator::AllocatorConfig;
use od_jit::block::{BlockLimits, BuildOutcome, CodeView, S3_32aAddRaxRaxAdmission, build_block};
use od_jit::block_compile::{
    BlockCompileOutcome, CompiledBlock, RUNTIME_FRAME_PREFIX_BYTES, allocate_lowered_block,
    compile_block, compile_block_for_s3_32a_add_rax_rax_candidate,
    compile_block_with_register_homes,
};
use od_jit::block_emitter::emit_runtime_block;
use od_jit::ir::Operation;
use od_jit::lowering::{LoweringOutcome, lower, lower_for_s3_32a_add_rax_rax_candidate};
use od_jit::runtime::RuntimeExitTag;
use serde::Serialize;

const BASE_RIP: u64 = 0x1000;
const INSTRUCTION_BUDGET: u64 = 6;
const HOME_BUDGET: u8 = 4;
const SPILL_ONLY_BUDGET: u8 = 0;
const PAIRS: usize = 20;
const MIN_SAMPLE: Duration = Duration::from_millis(200);
const CHUNK_CALLS: usize = 256;
const MAX_CALIBRATED_CALLS: usize = 1 << 24;
const MAX_CALIBRATION_MEASURED_SIDE_WALL: Duration = Duration::from_secs(3);
const MAX_CALIBRATION_WALL: Duration = Duration::from_secs(180);
const COMPILE_PAIRS: usize = 20;
const MAX_COMPILE_CALLS: usize = 1 << 20;
const WARMUP_CALLS: usize = 64;
const FROZEN_CODE_SHA256: &str = "4fe980f463d01c3fc413943b73860314d9c780c638acdc9bf50072290c89f778";
const DISTINCT_BRANCH_CODE_SHA256: &str =
    "72210fb82b0ffa9f88f10f63697a1051347d07587e3569028947119726fac8c7";
const CODE: [u8; 17] = [
    0x48, 0x03, 0xC0, // ADD RAX, RAX
    0x48, 0x2B, 0xD2, // SUB RDX, RDX
    0x48, 0x03, 0xC3, // ADD RAX, RBX
    0x48, 0x2B, 0xC9, // SUB RCX, RCX
    0x48, 0x3B, 0xC3, // CMP RAX, RBX
    0x74, 0x00, // JZ code_end; taken and fall-through converge
];
const DISTINCT_BRANCH_CODE: [u8; 27] = [
    0x48, 0x03, 0xC0, // ADD RAX, RAX
    0x48, 0x2B, 0xD2, // SUB RDX, RDX
    0x48, 0x03, 0xC3, // ADD RAX, RBX
    0x48, 0x2B, 0xC9, // SUB RCX, RCX
    0x48, 0x3B, 0xC3, // CMP RAX, RBX
    0x74, 0x0A, // JZ over the fall-through MOV
    0x48, 0xBB, // MOV RBX, imm64; outside the measured block
    0x11, 0x00, 0xFF, 0xEE, 0xDD, 0xCC, 0xBB, 0xAA,
];

#[derive(Clone, Copy)]
struct FixtureProfile {
    name: &'static str,
    code: &'static [u8],
    code_sha256: &'static str,
    branch_scope: &'static str,
    distinct_branch: bool,
}

const LEGACY_PROFILE: FixtureProfile = FixtureProfile {
    name: "same-target-jz",
    code: &CODE,
    code_sha256: FROZEN_CODE_SHA256,
    branch_scope: "Final JZ displacement is zero; taken and fall-through converge at code_end. This preserves the original S3-30a profile and does not test distinct branch routing.",
    distinct_branch: false,
};
const DISTINCT_BRANCH_PROFILE: FixtureProfile = FixtureProfile {
    name: "distinct-jz-fallthrough",
    code: &DISTINCT_BRANCH_CODE,
    code_sha256: DISTINCT_BRANCH_CODE_SHA256,
    branch_scope: "The first five instructions are unchanged; final JZ +10 selects code_end when taken and the next RIP (code_end-10) when not taken. The admitted MOV RBX,imm64 suffix lies beyond the emitted Jcc-terminated block and is outside the measured native call.",
    distinct_branch: true,
};

#[derive(Clone)]
struct Case {
    id: &'static str,
    initial: LongModeState,
    expected_zf: bool,
    expected_rip: u64,
    code: &'static [u8],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct StateSnapshot {
    gpr: [u64; 16],
    rip: u64,
    rflags: u64,
    fs_base: u64,
    gs_base: u64,
    auxiliary_state: String,
}

#[derive(Clone, Debug, Serialize)]
struct Observation {
    state: StateSnapshot,
    raw_exit: String,
    retired: u64,
    remaining_budget: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct AllocationReport {
    register_homes_budget: u8,
    assigned_register_home_values: u32,
    spilled_temporary_values: u32,
    value_spill_slots: u32,
    value_spill_loads: u32,
    value_spill_stores: u32,
    helper_capture_slots: u32,
    helper_saves: u32,
    helper_reloads: u32,
    planned_transfer_words: u32,
    planned_transfer_bytes: u32,
    helper_boundary_count: usize,
    frame_bytes: u32,
    emitted_code_bytes: usize,
    default_api_code_match: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
struct PairSample {
    order: String,
    interpreter_total_ns: u128,
    native_total_ns: u128,
    interpreter_ns_per_call: u128,
    native_ns_per_call: u128,
    native_over_interpreter: f64,
    minimum_sample_met: bool,
}

#[derive(Clone, Debug, Serialize)]
struct CompilePairSample {
    order: String,
    budget4_total_ns: u128,
    budget0_total_ns: u128,
    budget4_ns_per_compile: u128,
    budget0_ns_per_compile: u128,
    minimum_sample_met: bool,
}

#[derive(Clone, Debug, Serialize)]
struct CaseReport {
    id: String,
    initial: StateSnapshot,
    expected_zf: bool,
    expected_rip: u64,
    interpreter_oracle: Observation,
    preflight_native: Vec<(u8, Observation)>,
    execution_pairs: Vec<(u8, Vec<PairSample>)>,
    allocator_pairs: Vec<AllocatorPairSample>,
}

#[derive(Clone, Debug, Serialize)]
struct AllocatorPairSample {
    order: String,
    calls_per_side: usize,
    budget4_total_ns: u128,
    budget0_total_ns: u128,
    budget4_ns_per_call: u128,
    budget0_ns_per_call: u128,
    budget0_over_budget4: f64,
    budget4_publication_ns: u128,
    budget4_drop_ns: u128,
    budget0_publication_ns: u128,
    budget0_drop_ns: u128,
    minimum_sample_met: bool,
}

#[derive(Clone, Debug, Serialize)]
struct VariantReport {
    allocation: AllocationReport,
    publication_ns: Option<u128>,
    drop_ns: Option<u128>,
    publication_lifetime: String,
}

#[derive(Clone, Debug, Serialize)]
struct CalibrationReport {
    fixture_profile: String,
    common_repetitions_per_case_and_side: Option<usize>,
    warmup_calls_per_case_per_publication: usize,
    warmup_publications_per_case: usize,
    total_warmup_calls_per_case: usize,
    sample_pairs: usize,
    minimum_sample_ns: u128,
    maximum_repetitions_per_side: usize,
    maximum_measured_side_ns: u128,
    maximum_overall_wall_ns: u128,
    wall_bound_derivation: String,
    total_wall_ns: u128,
    outcome: String,
    calibration_checks: Vec<CalibrationCheck>,
}

#[derive(Clone, Debug, Serialize)]
struct CalibrationCheck {
    fixture_profile: String,
    record_kind: String,
    budget: u8,
    case_id: String,
    requested_repetitions: usize,
    native_calls: usize,
    interpreter_calls: Option<usize>,
    native_ns: Option<u128>,
    interpreter_ns: Option<u128>,
    projected_next_repetitions: Option<usize>,
    projected_native_ns: Option<u128>,
    projected_interpreter_ns: Option<u128>,
    projected_next_meets_floor: Option<bool>,
    projected_next_within_side_ceiling: Option<bool>,
    process_wall_ns: u128,
    sample_floor_met: Option<bool>,
    stop_reason: Option<String>,
}

#[derive(Clone, Debug)]
struct CalibrationRun {
    common_repetitions: Option<usize>,
    checks: Vec<CalibrationCheck>,
    total_wall_ns: u128,
    failure: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    schema_version: u32,
    fixture_profile: String,
    timing_started: bool,
    calibration_only_mode: bool,
    sample_floor_passed: Option<bool>,
    timing_status: String,
    sample_floor_policy: String,
    process_run: u8,
    host: crate::HostMetadata,
    frozen_code_sha256: String,
    code_bytes: Vec<u8>,
    guest_base_rip: u64,
    instruction_budget: u64,
    scope: String,
    compile_timer_boundary: String,
    publication_timer_boundary: String,
    execution_timer_boundary: String,
    cache_and_mapping_policy: String,
    exit_normalization: String,
    branch_scope: String,
    compile_calls_per_sample: Option<usize>,
    compile_pairs_ns_per_call: Vec<CompilePairSample>,
    variants: Vec<VariantReport>,
    calibration: Option<CalibrationReport>,
    cases: Vec<CaseReport>,
}

#[derive(Clone, Debug, Serialize)]
pub struct S3_32aReport {
    schema_version: u32,
    experiment: String,
    process_run: u8,
    host: crate::HostMetadata,
    timing_status: String,
    preflight_passed: bool,
    sample_floor_passed: Option<bool>,
    calibration_floor_passed: Option<bool>,
    source_fixture_sha256: String,
    code_bytes: Vec<u8>,
    guest_base_rip: u64,
    instruction_budget: u64,
    register_homes: u8,
    eager_flags: bool,
    cache_policy: String,
    compile_timer_boundary: String,
    publication_timer_boundary: String,
    native_timer_boundary: String,
    sample_floor_ns: u128,
    warmups_per_publication: usize,
    pairs_per_stratum: usize,
    common_compile_calls_per_side: Option<usize>,
    common_native_calls_per_side: Option<usize>,
    calibration_total_wall_ns: Option<u128>,
    calibration_deadline_ns: u128,
    calibration_side_ceiling_ns: u128,
    calibration_chunk_calls: usize,
    calibration_stop_reason: Option<String>,
    compile_calibration: Vec<S3_32aCalibrationSample>,
    native_calibration: Vec<S3_32aCalibrationSample>,
    variant_profiles: Vec<S3_32aVariantProfile>,
    compile_pairs: Vec<S3_32aCompilePair>,
    cases: Vec<S3_32aCaseReport>,
}

#[derive(Clone, Debug, Serialize)]
struct S3_32aVariantProfile {
    variant: String,
    register_homes: u8,
    ir_read_gpr_operations: usize,
    ir_integer_operations: usize,
    ir_temporary_definitions: usize,
    assigned_register_home_values: u32,
    spilled_temporary_values: u32,
    value_spill_slots: u32,
    helper_capture_slots: u32,
    helper_saves: u32,
    helper_reloads: u32,
    frame_bytes: u32,
    emitted_code_bytes: usize,
}

#[derive(Clone, Debug, Serialize)]
struct S3_32aCalibrationSample {
    variant: String,
    case_id: Option<String>,
    order: String,
    calls: usize,
    completed_calls: usize,
    elapsed_ns: u128,
    process_wall_ns: u128,
    process_deadline_ns: u128,
    side_ceiling_ns: u128,
    minimum_sample_met: bool,
    stop_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct S3_32aCompilePair {
    pair_index: usize,
    order: String,
    calls_per_side: usize,
    baseline_total_ns: u128,
    candidate_total_ns: u128,
    baseline_ns_per_compile: u128,
    candidate_ns_per_compile: u128,
    candidate_over_baseline: f64,
    minimum_sample_met: bool,
}

#[derive(Clone, Debug, Serialize)]
struct S3_32aNativePair {
    pair_index: usize,
    order: String,
    calls_per_side: usize,
    baseline_total_ns: u128,
    candidate_total_ns: u128,
    baseline_ns_per_call: u128,
    candidate_ns_per_call: u128,
    candidate_over_baseline: f64,
    baseline_publication_ns: u128,
    baseline_drop_ns: u128,
    candidate_publication_ns: u128,
    candidate_drop_ns: u128,
    minimum_sample_met: bool,
}

#[derive(Clone, Debug, Serialize)]
struct S3_32aCaseReport {
    id: String,
    initial: StateSnapshot,
    interpreter_oracle: Observation,
    baseline_preflight: Observation,
    candidate_preflight: Observation,
    native_pairs: Vec<S3_32aNativePair>,
}

#[derive(Debug)]
struct CostError(String);

impl std::fmt::Display for CostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CostError {}

fn fail(message: impl Into<String>) -> CostError {
    CostError(message.into())
}

fn cases(profile: FixtureProfile) -> Vec<Case> {
    let mut equal = LongModeState::default();
    equal.rip = BASE_RIP;
    equal.rflags = 0x202;
    for (index, value) in equal.gpr.iter_mut().enumerate() {
        *value = 0x1010 + index as u64;
    }
    equal.gpr[0] = 0; // 2*0 + RBX == RBX, so final CMP sets ZF.
    equal.gpr[3] = 7;
    equal.gpr[1] = 0x22;
    equal.gpr[2] = 0x33;

    let mut unequal = equal.clone();
    unequal.gpr[0] = 1; // 2*1 + 7 != 7, so final CMP clears ZF.

    let end = BASE_RIP + profile.code.len() as u64;
    let equal_rip = end;
    let unequal_rip = if profile.distinct_branch {
        end - 10
    } else {
        end
    };
    vec![
        Case {
            id: if profile.distinct_branch {
                "jz-taken"
            } else {
                "multi-helper-equal"
            },
            initial: equal,
            expected_zf: true,
            expected_rip: equal_rip,
            code: profile.code,
        },
        Case {
            id: if profile.distinct_branch {
                "jz-fallthrough"
            } else {
                "multi-helper-not-equal"
            },
            initial: unequal,
            expected_zf: false,
            expected_rip: unequal_rip,
            code: profile.code,
        },
    ]
}

pub fn preflight(process_run: u8) -> Result<Report, Box<dyn std::error::Error>> {
    build_report(process_run, true, false, None, LEGACY_PROFILE)
}

pub fn run(
    process_run: u8,
    calibration_journal: &Path,
) -> Result<Report, Box<dyn std::error::Error>> {
    build_report(
        process_run,
        false,
        false,
        Some(calibration_journal),
        LEGACY_PROFILE,
    )
}

pub fn calibration_only(
    process_run: u8,
    calibration_journal: &Path,
) -> Result<Report, Box<dyn std::error::Error>> {
    build_report(
        process_run,
        false,
        true,
        Some(calibration_journal),
        LEGACY_PROFILE,
    )
}

pub fn preflight_distinct_branch(process_run: u8) -> Result<Report, Box<dyn std::error::Error>> {
    build_report(process_run, true, false, None, DISTINCT_BRANCH_PROFILE)
}

pub fn run_distinct_branch(
    process_run: u8,
    calibration_journal: &Path,
) -> Result<Report, Box<dyn std::error::Error>> {
    build_report(
        process_run,
        false,
        false,
        Some(calibration_journal),
        DISTINCT_BRANCH_PROFILE,
    )
}

pub fn calibration_only_distinct_branch(
    process_run: u8,
    calibration_journal: &Path,
) -> Result<Report, Box<dyn std::error::Error>> {
    build_report(
        process_run,
        false,
        true,
        Some(calibration_journal),
        DISTINCT_BRANCH_PROFILE,
    )
}

/// Preflight the exact baseline versus S3-32a candidate on the frozen
/// distinct-branch fixture. This performs correctness checks without timing.
pub fn preflight_s3_32a(process_run: u8) -> Result<S3_32aReport, Box<dyn std::error::Error>> {
    build_s3_32a_report(process_run, true, false)
}

pub fn run_s3_32a(process_run: u8) -> Result<S3_32aReport, Box<dyn std::error::Error>> {
    build_s3_32a_report(process_run, false, false)
}

pub fn calibration_only_s3_32a(
    process_run: u8,
) -> Result<S3_32aReport, Box<dyn std::error::Error>> {
    build_s3_32a_report(process_run, false, true)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum S3_32aVariant {
    Baseline,
    Candidate,
}

impl S3_32aVariant {
    fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Candidate => "s3_32a_candidate",
        }
    }
}

struct S3_32aPreparedVariant {
    compiled: CompiledBlock,
    profile: S3_32aVariantProfile,
}

fn build_s3_32a_report(
    process_run: u8,
    preflight_only: bool,
    calibration_only: bool,
) -> Result<S3_32aReport, Box<dyn std::error::Error>> {
    if !(1..=3).contains(&process_run) {
        return Err(fail("process-run must be 1..=3").into());
    }
    let digest = od_testkit::sha256_hex(DISTINCT_BRANCH_PROFILE.code);
    if digest != DISTINCT_BRANCH_CODE_SHA256 {
        return Err(fail("frozen distinct branch fixture digest mismatch").into());
    }
    let view = CodeView::new(BASE_RIP, DISTINCT_BRANCH_PROFILE.code)
        .map_err(|error| fail(format!("code view: {error:?}")))?;
    let admission = S3_32aAddRaxRaxAdmission::from_code(view)
        .map_err(|error| fail(format!("S3-32a candidate admission: {error:?}")))?;
    let baseline = prepare_s3_32a_variant(S3_32aVariant::Baseline, view, &admission)?;
    let candidate = prepare_s3_32a_variant(S3_32aVariant::Candidate, view, &admission)?;
    if baseline.profile.ir_read_gpr_operations != candidate.profile.ir_read_gpr_operations + 1 {
        return Err(fail(format!(
            "exact fixture should remove one ReadGpr: baseline={}, candidate={}",
            baseline.profile.ir_read_gpr_operations, candidate.profile.ir_read_gpr_operations
        ))
        .into());
    }

    let cases = cases(DISTINCT_BRANCH_PROFILE);
    let interpreters = cases
        .iter()
        .map(interpreter_observation)
        .collect::<Result<Vec<_>, _>>()?;
    let mut preflight_observations: Vec<Vec<(S3_32aVariant, Observation)>> =
        (0..cases.len()).map(|_| Vec::new()).collect();
    for (variant, compiled) in [
        (S3_32aVariant::Baseline, &baseline.compiled),
        (S3_32aVariant::Candidate, &candidate.compiled),
    ] {
        let published = PublishedCode::publish_block(compiled.emitted())
            .map_err(|error| fail(format!("preflight publish {}: {error:?}", variant.name())))?;
        for (index, (case, oracle)) in cases.iter().zip(&interpreters).enumerate() {
            let native = native_observation(&published, case)?;
            validate_pair(case, &native, oracle)?;
            preflight_observations[index].push((variant, native));
        }
        drop(published);
    }
    let mut case_reports = cases
        .iter()
        .zip(&interpreters)
        .zip(preflight_observations)
        .map(|((case, oracle), observations)| {
            let baseline_preflight = observations
                .iter()
                .find(|(variant, _)| *variant == S3_32aVariant::Baseline)
                .map(|(_, observation)| observation.clone())
                .ok_or_else(|| fail("missing baseline preflight observation"))?;
            let candidate_preflight = observations
                .iter()
                .find(|(variant, _)| *variant == S3_32aVariant::Candidate)
                .map(|(_, observation)| observation.clone())
                .ok_or_else(|| fail("missing candidate preflight observation"))?;
            Ok::<_, CostError>(S3_32aCaseReport {
                id: case.id.into(),
                initial: snapshot(&case.initial),
                interpreter_oracle: oracle.clone(),
                baseline_preflight,
                candidate_preflight,
                native_pairs: Vec::new(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut compile_calibration = Vec::new();
    let mut native_calibration = Vec::new();
    let mut compile_calls = None;
    let mut native_calls = None;
    let mut compile_pairs = Vec::new();
    let mut calibration_floor_passed = None;
    let mut calibration_total_wall_ns = None;
    let mut calibration_stop_reason = None;
    if !preflight_only {
        let calibration_started = Instant::now();
        let limits = CalibrationLimits {
            deadline: calibration_started + MAX_CALIBRATION_WALL,
        };
        let (calls, samples, stop_reason) =
            calibrate_s3_32a_compile(process_run, view, &admission, calibration_started, limits)?;
        compile_calls = calls;
        compile_calibration = samples;
        calibration_stop_reason = stop_reason;

        if compile_calls.is_some() && calibration_stop_reason.is_none() {
            let (calls, samples, stop_reason) = calibrate_s3_32a_native(
                process_run,
                &cases,
                &interpreters,
                &[&baseline.compiled, &candidate.compiled],
                calibration_started,
                limits,
            )?;
            native_calls = calls;
            native_calibration = samples;
            calibration_stop_reason = stop_reason;
        }

        if calibration_stop_reason.is_none() {
            calibration_stop_reason =
                calibration_finalization_stop_reason(false, calibration_started.elapsed())
                    .map(str::to_owned);
        }
        if calibration_stop_reason.is_some() {
            compile_calls = None;
            native_calls = None;
        }
        calibration_total_wall_ns = Some(calibration_started.elapsed().as_nanos());
        calibration_floor_passed = Some(
            compile_calls.is_some() && native_calls.is_some() && calibration_stop_reason.is_none(),
        );

        if compile_calls.is_some() && native_calls.is_some() && !calibration_only {
            let calls = compile_calls.unwrap();
            compile_pairs = collect_s3_32a_compile_pairs(process_run, view, &admission, calls)?;
            let calls = native_calls.unwrap();
            for (case_index, (case, oracle)) in cases.iter().zip(&interpreters).enumerate() {
                case_reports[case_index].native_pairs = collect_s3_32a_native_pairs(
                    process_run,
                    case_index,
                    case,
                    oracle,
                    [&baseline.compiled, &candidate.compiled],
                    calls,
                )?;
            }
        }
    }

    let sample_floor_passed =
        if preflight_only || calibration_only || calibration_floor_passed == Some(false) {
            None
        } else {
            Some(
                compile_pairs
                    .iter()
                    .all(|sample: &S3_32aCompilePair| sample.minimum_sample_met)
                    && case_reports
                        .iter()
                        .flat_map(|case| &case.native_pairs)
                        .all(|sample| sample.minimum_sample_met),
            )
        };
    let timing_status = if preflight_only {
        "preflight_only_no_timing"
    } else if calibration_only && calibration_floor_passed == Some(true) {
        "calibration_only_unscored"
    } else if calibration_only {
        "calibration_only_failed_unscored"
    } else if calibration_floor_passed == Some(false) {
        "diagnostic_only_calibration_failed_no_scored_comparison"
    } else if sample_floor_passed == Some(true) {
        "sample_floor_passed_bounded_diagnostic"
    } else {
        "diagnostic_only_sample_floor_failed_no_scored_comparison"
    };
    Ok(S3_32aReport {
        schema_version: 1,
        experiment: "s3-32a-aliased-add-rax-rax-baseline-vs-candidate".into(),
        process_run,
        host: crate::HostMetadata::current(),
        timing_status: timing_status.into(),
        preflight_passed: true,
        sample_floor_passed,
        calibration_floor_passed,
        source_fixture_sha256: digest,
        code_bytes: DISTINCT_BRANCH_PROFILE.code.to_vec(),
        guest_base_rip: BASE_RIP,
        instruction_budget: INSTRUCTION_BUDGET,
        register_homes: HOME_BUDGET,
        eager_flags: true,
        cache_policy: "uncached compile and one PublishedCode mapping at a time; no shared cache lookup/admission/key changes".into(),
        compile_timer_boundary: "Each side calls the named baseline or exact S3-32a compiler; block build, decode, verified lowering/transform, allocation, emission, finalization and destruction of the unpublished CompiledBlock are timed. Admission is prepared once before timing. Diagnostic lowering/allocation and summary collection are outside the timer.".into(),
        publication_timer_boundary: "PublishedCode::publish_block and PublishedCode drop are recorded separately for every native side; the prior mapping is dropped before the next publication.".into(),
        native_timer_boundary: "Repeated PublishedCode::execute_block calls with the existing safe host wrapper and result collection. State-vector construction and validation are outside the timer; result validation occurs after each timed batch.".into(),
        sample_floor_ns: MIN_SAMPLE.as_nanos(),
        warmups_per_publication: WARMUP_CALLS,
        pairs_per_stratum: PAIRS,
        common_compile_calls_per_side: compile_calls,
        common_native_calls_per_side: native_calls,
        calibration_total_wall_ns,
        calibration_deadline_ns: MAX_CALIBRATION_WALL.as_nanos(),
        calibration_side_ceiling_ns: MAX_CALIBRATION_MEASURED_SIDE_WALL.as_nanos(),
        calibration_chunk_calls: CHUNK_CALLS,
        calibration_stop_reason,
        compile_calibration,
        native_calibration,
        variant_profiles: vec![baseline.profile, candidate.profile],
        compile_pairs,
        cases: case_reports,
    })
}

fn prepare_s3_32a_variant(
    variant: S3_32aVariant,
    view: CodeView<'_>,
    admission: &S3_32aAddRaxRaxAdmission,
) -> Result<S3_32aPreparedVariant, CostError> {
    let built = build_block(
        view,
        BASE_RIP,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(|error| fail(format!("{} diagnostic build: {error:?}", variant.name())))?;
    let lowered = match variant {
        S3_32aVariant::Baseline => lower(&built),
        S3_32aVariant::Candidate => lower_for_s3_32a_add_rax_rax_candidate(&built, admission),
    }
    .map_err(|error| fail(format!("{} diagnostic lowering: {error:?}", variant.name())))?;
    let lowered = match lowered {
        LoweringOutcome::Program(lowered) => lowered,
        LoweringOutcome::NoNativePrefix { .. } => {
            return Err(fail(format!("{} has no native prefix", variant.name())));
        }
    };
    let allocation = allocate_lowered_block(
        &lowered,
        AllocatorConfig {
            register_homes: HOME_BUDGET,
            reserved_prefix_bytes: RUNTIME_FRAME_PREFIX_BYTES,
        },
    )
    .map_err(|error| {
        fail(format!(
            "{} diagnostic allocation: {error:?}",
            variant.name()
        ))
    })?;
    let diagnostic_emitted = emit_runtime_block(&lowered.program, &allocation)
        .map_err(|error| fail(format!("{} diagnostic emit: {error:?}", variant.name())))?;
    let compiled = compile_s3_32a_variant(variant, view, admission)?;
    let diagnostic_bytes = diagnostic_emitted
        .to_le_bytes()
        .map_err(|error| fail(format!("{} diagnostic bytes: {error:?}", variant.name())))?;
    let compiled_bytes = compiled
        .emitted()
        .to_le_bytes()
        .map_err(|error| fail(format!("{} compiled bytes: {error:?}", variant.name())))?;
    if diagnostic_bytes != compiled_bytes
        || diagnostic_emitted.pc_map() != compiled.emitted().pc_map()
    {
        return Err(fail(format!(
            "{} diagnostic lowering/allocation/emitter differs from exact compile output",
            variant.name()
        )));
    }
    let locations = allocation.location_counts();
    let counters = allocation.counters();
    let frame = allocation.frame();
    let profile = S3_32aVariantProfile {
        variant: variant.name().into(),
        register_homes: HOME_BUDGET,
        ir_read_gpr_operations: lowered
            .program
            .operations
            .iter()
            .filter(|operation| matches!(operation, Operation::ReadGpr { .. }))
            .count(),
        ir_integer_operations: lowered
            .program
            .operations
            .iter()
            .filter(|operation| matches!(operation, Operation::Integer { .. }))
            .count(),
        ir_temporary_definitions: (locations.register_homes + locations.spill_values) as usize,
        assigned_register_home_values: locations.register_homes,
        spilled_temporary_values: locations.spill_values,
        value_spill_slots: counters.value_spill_slots,
        helper_capture_slots: counters.helper_capture_slots,
        helper_saves: counters.helper_saves,
        helper_reloads: counters.helper_reloads,
        frame_bytes: frame.frame_bytes,
        emitted_code_bytes: compiled_bytes.len(),
    };
    Ok(S3_32aPreparedVariant { compiled, profile })
}

fn compile_s3_32a_variant(
    variant: S3_32aVariant,
    view: CodeView<'_>,
    admission: &S3_32aAddRaxRaxAdmission,
) -> Result<CompiledBlock, CostError> {
    let outcome = match variant {
        S3_32aVariant::Baseline => compile_block(view, BASE_RIP),
        S3_32aVariant::Candidate => {
            compile_block_for_s3_32a_add_rax_rax_candidate(view, BASE_RIP, admission)
        }
    }
    .map_err(|error| fail(format!("{} compile: {error:?}", variant.name())))?;
    match outcome {
        BlockCompileOutcome::Compiled(compiled) => Ok(compiled),
        BlockCompileOutcome::NoNativePrefix { .. } => Err(fail(format!(
            "{} compile produced no native prefix",
            variant.name()
        ))),
    }
}

fn calibrate_s3_32a_compile(
    process_run: u8,
    view: CodeView<'_>,
    admission: &S3_32aAddRaxRaxAdmission,
    process_started: Instant,
    limits: CalibrationLimits,
) -> Result<(Option<usize>, Vec<S3_32aCalibrationSample>, Option<String>), CostError> {
    let mut calls = 256usize;
    let mut samples = Vec::new();
    let mut calibration_pair = 0usize;
    loop {
        let reverse = (calibration_pair + usize::from(process_run)) % 2 != 0;
        let (baseline, candidate) = if reverse {
            let candidate = time_s3_32a_compile_batch_bounded(
                S3_32aVariant::Candidate,
                view,
                admission,
                calls,
                limits,
            )?;
            let baseline = if candidate.stop_reason.is_none() {
                time_s3_32a_compile_batch_bounded(
                    S3_32aVariant::Baseline,
                    view,
                    admission,
                    calls,
                    limits,
                )?
            } else {
                skipped_calibration_batch("skipped_after_candidate_calibration_stop")
            };
            (baseline, candidate)
        } else {
            let baseline = time_s3_32a_compile_batch_bounded(
                S3_32aVariant::Baseline,
                view,
                admission,
                calls,
                limits,
            )?;
            let candidate = if baseline.stop_reason.is_none() {
                time_s3_32a_compile_batch_bounded(
                    S3_32aVariant::Candidate,
                    view,
                    admission,
                    calls,
                    limits,
                )?
            } else {
                skipped_calibration_batch("skipped_after_baseline_calibration_stop")
            };
            (baseline, candidate)
        };
        let ordered = if reverse {
            [
                (S3_32aVariant::Candidate, candidate.clone()),
                (S3_32aVariant::Baseline, baseline.clone()),
            ]
        } else {
            [
                (S3_32aVariant::Baseline, baseline.clone()),
                (S3_32aVariant::Candidate, candidate.clone()),
            ]
        };
        for (variant, timing) in ordered {
            samples.push(s3_32a_calibration_sample(
                variant,
                None,
                if reverse {
                    "candidate_then_baseline"
                } else {
                    "baseline_then_candidate"
                },
                calls,
                timing,
                process_started,
                limits,
            ));
        }
        if let Some(reason) = first_compile_calibration_stop_reason(reverse, &baseline, &candidate)
        {
            return Ok((None, samples, Some(reason)));
        }
        if baseline.elapsed >= MIN_SAMPLE && candidate.elapsed >= MIN_SAMPLE {
            if Instant::now() >= limits.deadline {
                return Ok((
                    None,
                    samples,
                    Some("overall_wall_ceiling_after_compile_calibration_finalization".into()),
                ));
            }
            return Ok((Some(calls), samples, None));
        }
        let Some(next) = calls
            .checked_mul(2)
            .filter(|next| *next <= MAX_COMPILE_CALLS)
        else {
            return Ok((
                None,
                samples,
                Some("compile_repetition_ceiling_before_sample_floor".into()),
            ));
        };
        if Instant::now() >= limits.deadline {
            return Ok((None, samples, Some("overall_wall_ceiling".into())));
        }
        calls = next;
        calibration_pair += 1;
    }
}

fn time_s3_32a_compile_batch(
    variant: S3_32aVariant,
    view: CodeView<'_>,
    admission: &S3_32aAddRaxRaxAdmission,
    calls: usize,
) -> Result<Duration, CostError> {
    let started = Instant::now();
    for _ in 0..calls {
        let compiled = compile_s3_32a_variant(variant, view, admission)?;
        std::hint::black_box(compiled.emitted().pc_map().code_len());
        drop(compiled);
    }
    Ok(started.elapsed())
}

fn time_s3_32a_compile_batch_bounded(
    variant: S3_32aVariant,
    view: CodeView<'_>,
    admission: &S3_32aAddRaxRaxAdmission,
    calls: usize,
    limits: CalibrationLimits,
) -> Result<TimedBatch, CostError> {
    let mut elapsed = Duration::ZERO;
    let mut completed_calls = 0;
    while completed_calls < calls {
        if let Some(stop_reason) = calibration_stop_reason(Some(limits), elapsed) {
            return Ok(TimedBatch {
                elapsed,
                completed_calls,
                stop_reason: Some(stop_reason),
            });
        }
        let chunk = (calls - completed_calls).min(CHUNK_CALLS);
        let started = Instant::now();
        for _ in 0..chunk {
            let compiled = compile_s3_32a_variant(variant, view, admission)?;
            std::hint::black_box(compiled.emitted().pc_map().code_len());
            drop(compiled);
            completed_calls += 1;
        }
        elapsed += started.elapsed();
        if let Some(stop_reason) = calibration_stop_reason(Some(limits), elapsed) {
            return Ok(TimedBatch {
                elapsed,
                completed_calls,
                stop_reason: Some(stop_reason),
            });
        }
    }
    Ok(TimedBatch {
        elapsed,
        completed_calls,
        stop_reason: None,
    })
}

fn skipped_calibration_batch(reason: &str) -> TimedBatch {
    TimedBatch {
        elapsed: Duration::ZERO,
        completed_calls: 0,
        stop_reason: Some(reason.into()),
    }
}

fn s3_32a_calibration_sample(
    variant: S3_32aVariant,
    case_id: Option<&str>,
    order: &str,
    requested_calls: usize,
    timing: TimedBatch,
    process_started: Instant,
    limits: CalibrationLimits,
) -> S3_32aCalibrationSample {
    let process_wall_ns = process_started.elapsed().as_nanos();
    let mut stop_reason = timing.stop_reason;
    if stop_reason.is_none() && Instant::now() >= limits.deadline {
        stop_reason = Some("overall_wall_ceiling_after_side_finalization".into());
    }
    S3_32aCalibrationSample {
        variant: variant.name().into(),
        case_id: case_id.map(str::to_owned),
        order: order.into(),
        calls: requested_calls,
        completed_calls: timing.completed_calls,
        elapsed_ns: timing.elapsed.as_nanos(),
        process_wall_ns,
        process_deadline_ns: MAX_CALIBRATION_WALL.as_nanos(),
        side_ceiling_ns: MAX_CALIBRATION_MEASURED_SIDE_WALL.as_nanos(),
        minimum_sample_met: timing.completed_calls == requested_calls
            && timing.elapsed >= MIN_SAMPLE
            && stop_reason.is_none(),
        stop_reason,
    }
}

fn calibrate_s3_32a_native(
    process_run: u8,
    cases: &[Case],
    interpreters: &[Observation],
    compiled: &[&CompiledBlock; 2],
    process_started: Instant,
    limits: CalibrationLimits,
) -> Result<(Option<usize>, Vec<S3_32aCalibrationSample>, Option<String>), CostError> {
    let mut calls = CHUNK_CALLS;
    let mut samples = Vec::new();
    let mut calibration_pair = 0usize;
    loop {
        let mut current = Vec::with_capacity(cases.len() * 2);
        for (case_index, (case, oracle)) in cases.iter().zip(interpreters).enumerate() {
            let reverse = (calibration_pair + usize::from(process_run) + case_index) % 2 != 0;
            let ordered = if reverse {
                [S3_32aVariant::Candidate, S3_32aVariant::Baseline]
            } else {
                [S3_32aVariant::Baseline, S3_32aVariant::Candidate]
            };
            for (side_index, variant) in ordered.into_iter().enumerate() {
                let variant_index = match variant {
                    S3_32aVariant::Baseline => 0,
                    S3_32aVariant::Candidate => 1,
                };
                let timing = if side_index > 0
                    && current
                        .iter()
                        .any(|sample: &S3_32aCalibrationSample| sample.stop_reason.is_some())
                {
                    skipped_calibration_batch("skipped_after_prior_calibration_stop")
                } else {
                    time_s3_32a_native_side_bounded(
                        compiled[variant_index],
                        case,
                        oracle,
                        calls,
                        limits,
                    )?
                };
                current.push(s3_32a_calibration_sample(
                    variant,
                    Some(case.id),
                    if reverse {
                        "candidate_then_baseline"
                    } else {
                        "baseline_then_candidate"
                    },
                    calls,
                    timing,
                    process_started,
                    limits,
                ));
            }
        }
        let all_met = current.iter().all(|sample| sample.minimum_sample_met);
        let stop_reason = current.iter().find_map(|sample| sample.stop_reason.clone());
        samples.extend(current);
        if let Some(reason) = stop_reason {
            return Ok((None, samples, Some(reason)));
        }
        if all_met {
            if Instant::now() >= limits.deadline {
                return Ok((
                    None,
                    samples,
                    Some("overall_wall_ceiling_after_native_calibration_finalization".into()),
                ));
            }
            return Ok((Some(calls), samples, None));
        }
        let Some(next) = calls
            .checked_mul(2)
            .filter(|next| *next <= MAX_CALIBRATED_CALLS)
        else {
            return Ok((
                None,
                samples,
                Some("native_repetition_ceiling_before_sample_floor".into()),
            ));
        };
        if Instant::now() >= limits.deadline {
            return Ok((None, samples, Some("overall_wall_ceiling".into())));
        }
        calls = next;
        calibration_pair += 1;
    }
}

fn time_s3_32a_native_side_bounded(
    compiled: &CompiledBlock,
    case: &Case,
    oracle: &Observation,
    calls: usize,
    limits: CalibrationLimits,
) -> Result<TimedBatch, CostError> {
    if Instant::now() >= limits.deadline {
        return Ok(skipped_calibration_batch(
            "overall_wall_ceiling_before_publish",
        ));
    }
    let published = PublishedCode::publish_block(compiled.emitted())
        .map_err(|error| fail(format!("S3-32a calibration publish: {error:?}")))?;
    for _ in 0..WARMUP_CALLS {
        if Instant::now() >= limits.deadline {
            drop(published);
            return Ok(skipped_calibration_batch(
                "overall_wall_ceiling_during_warmup",
            ));
        }
        let native = native_observation(&published, case)?;
        validate_pair(case, &native, oracle)?;
    }
    let mut timing = time_native_batch_bounded(&published, case, oracle, calls, Some(limits))?;
    drop(published);
    if timing.stop_reason.is_none() && Instant::now() >= limits.deadline {
        timing.stop_reason = Some("overall_wall_ceiling_after_native_teardown".into());
    }
    Ok(timing)
}

fn time_s3_32a_native_side(
    compiled: &CompiledBlock,
    case: &Case,
    oracle: &Observation,
    calls: usize,
) -> Result<(Duration, u128, u128), CostError> {
    let publication_started = Instant::now();
    let published = PublishedCode::publish_block(compiled.emitted())
        .map_err(|error| fail(format!("S3-32a publish: {error:?}")))?;
    let publication_ns = publication_started.elapsed().as_nanos();
    for _ in 0..WARMUP_CALLS {
        let native = native_observation(&published, case)?;
        validate_pair(case, &native, oracle)?;
    }
    let elapsed = time_native_batch(&published, case, oracle, calls)?;
    let drop_started = Instant::now();
    drop(published);
    let drop_ns = drop_started.elapsed().as_nanos();
    Ok((elapsed, publication_ns, drop_ns))
}

fn collect_s3_32a_compile_pairs(
    process_run: u8,
    view: CodeView<'_>,
    admission: &S3_32aAddRaxRaxAdmission,
    calls: usize,
) -> Result<Vec<S3_32aCompilePair>, CostError> {
    let mut samples = Vec::with_capacity(COMPILE_PAIRS);
    for pair_index in 0..COMPILE_PAIRS {
        let reverse = (pair_index + usize::from(process_run)) % 2 != 0;
        let (baseline, candidate) = if reverse {
            let candidate =
                time_s3_32a_compile_batch(S3_32aVariant::Candidate, view, admission, calls)?;
            let baseline =
                time_s3_32a_compile_batch(S3_32aVariant::Baseline, view, admission, calls)?;
            (baseline, candidate)
        } else {
            let baseline =
                time_s3_32a_compile_batch(S3_32aVariant::Baseline, view, admission, calls)?;
            let candidate =
                time_s3_32a_compile_batch(S3_32aVariant::Candidate, view, admission, calls)?;
            (baseline, candidate)
        };
        samples.push(S3_32aCompilePair {
            pair_index,
            order: if reverse {
                "candidate_then_baseline".into()
            } else {
                "baseline_then_candidate".into()
            },
            calls_per_side: calls,
            baseline_total_ns: baseline.as_nanos(),
            candidate_total_ns: candidate.as_nanos(),
            baseline_ns_per_compile: baseline.as_nanos() / calls as u128,
            candidate_ns_per_compile: candidate.as_nanos() / calls as u128,
            candidate_over_baseline: candidate.as_nanos() as f64
                / baseline.as_nanos().max(1) as f64,
            minimum_sample_met: baseline >= MIN_SAMPLE && candidate >= MIN_SAMPLE,
        });
    }
    Ok(samples)
}

fn collect_s3_32a_native_pairs(
    process_run: u8,
    case_index: usize,
    case: &Case,
    oracle: &Observation,
    compiled: [&CompiledBlock; 2],
    calls: usize,
) -> Result<Vec<S3_32aNativePair>, CostError> {
    let mut samples = Vec::with_capacity(PAIRS);
    for pair_index in 0..PAIRS {
        let reverse = (pair_index + usize::from(process_run) + case_index) % 2 != 0;
        let run_side = |variant| {
            let index = match variant {
                S3_32aVariant::Baseline => 0,
                S3_32aVariant::Candidate => 1,
            };
            time_s3_32a_native_side(compiled[index], case, oracle, calls)
        };
        let (baseline, candidate) = if reverse {
            let candidate = run_side(S3_32aVariant::Candidate)?;
            let baseline = run_side(S3_32aVariant::Baseline)?;
            (baseline, candidate)
        } else {
            let baseline = run_side(S3_32aVariant::Baseline)?;
            let candidate = run_side(S3_32aVariant::Candidate)?;
            (baseline, candidate)
        };
        samples.push(S3_32aNativePair {
            pair_index,
            order: if reverse {
                "candidate_then_baseline_sequential_publications".into()
            } else {
                "baseline_then_candidate_sequential_publications".into()
            },
            calls_per_side: calls,
            baseline_total_ns: baseline.0.as_nanos(),
            candidate_total_ns: candidate.0.as_nanos(),
            baseline_ns_per_call: baseline.0.as_nanos() / calls as u128,
            candidate_ns_per_call: candidate.0.as_nanos() / calls as u128,
            candidate_over_baseline: candidate.0.as_nanos() as f64
                / baseline.0.as_nanos().max(1) as f64,
            baseline_publication_ns: baseline.1,
            baseline_drop_ns: baseline.2,
            candidate_publication_ns: candidate.1,
            candidate_drop_ns: candidate.2,
            minimum_sample_met: baseline.0 >= MIN_SAMPLE && candidate.0 >= MIN_SAMPLE,
        });
    }
    Ok(samples)
}

fn build_report(
    process_run: u8,
    preflight_only: bool,
    calibration_only: bool,
    calibration_journal: Option<&Path>,
    profile: FixtureProfile,
) -> Result<Report, Box<dyn std::error::Error>> {
    if !(1..=3).contains(&process_run) {
        return Err(fail("process-run must be 1..=3").into());
    }
    let code_sha256 = od_testkit::sha256_hex(profile.code);
    if code_sha256 != profile.code_sha256 {
        return Err(fail(format!("frozen {} fixture digest mismatch", profile.name)).into());
    }
    let cases = cases(profile);
    let mut interpreters = Vec::with_capacity(cases.len());
    for case in &cases {
        interpreters.push(interpreter_observation(case)?);
    }

    let mut variant_reports = Vec::new();
    let mut case_reports = cases
        .iter()
        .zip(&interpreters)
        .map(|(case, oracle)| CaseReport {
            id: case.id.into(),
            initial: snapshot(&case.initial),
            expected_zf: case.expected_zf,
            expected_rip: case.expected_rip,
            interpreter_oracle: oracle.clone(),
            preflight_native: Vec::new(),
            execution_pairs: Vec::new(),
            allocator_pairs: Vec::new(),
        })
        .collect::<Vec<_>>();

    let mut calibration = None;
    let mut calibration_failed = false;
    let mut compile_calls_per_sample = None;
    let mut compile_pairs_ns_per_call = Vec::new();
    let mut budgets = vec![HOME_BUDGET, SPILL_ONLY_BUDGET];
    if process_run % 2 == 0 {
        budgets.reverse();
    }

    // Complete both variant parity and allocation gates before any timing
    // calibration or sample collection.
    for &budget in &budgets {
        let (compiled, allocation) = compile_variant(budget, profile.code)?;
        validate_allocation(&allocation)?;
        let published = PublishedCode::publish_block(compiled.emitted())
            .map_err(|error| fail(format!("preflight publish budget {budget}: {error:?}")))?;
        for (index, (case, interpreter)) in cases.iter().zip(&interpreters).enumerate() {
            let native = native_observation(&published, case)?;
            validate_pair(case, &native, interpreter)?;
            case_reports[index].preflight_native.push((budget, native));
        }
        drop(published);
        variant_reports.push(VariantReport {
            allocation,
            publication_ns: None,
            drop_ns: None,
            publication_lifetime: "one preflight publication; dropped before next budget".into(),
        });
    }

    if !preflight_only {
        let calibration_started = Instant::now();
        let repetitions = calibrate_common_repetitions(
            &cases,
            &interpreters,
            &budgets,
            profile.code,
            profile.name,
            calibration_journal.ok_or_else(|| fail("calibration journal path is required"))?,
            calibration_started,
        )?;
        calibration_failed = repetitions.failure.is_some();
        let common_repetitions = repetitions.common_repetitions.unwrap_or(0);
        let collect_samples = collect_scored_samples(calibration_only, calibration_failed);
        let warmup_calls_per_publication = if collect_samples { WARMUP_CALLS } else { 0 };
        let warmup_publications_per_case = if !collect_samples {
            0
        } else if profile.distinct_branch {
            PAIRS * 2
        } else {
            budgets.len()
        };
        calibration = Some(CalibrationReport {
            fixture_profile: profile.name.into(),
            common_repetitions_per_case_and_side: repetitions.common_repetitions,
            warmup_calls_per_case_per_publication: warmup_calls_per_publication,
            warmup_publications_per_case,
            total_warmup_calls_per_case: warmup_calls_per_publication
                * warmup_publications_per_case,
            sample_pairs: if calibration_only || calibration_failed {
                0
            } else {
                PAIRS
            },
            minimum_sample_ns: MIN_SAMPLE.as_nanos(),
            maximum_repetitions_per_side: MAX_CALIBRATED_CALLS,
            maximum_measured_side_ns: MAX_CALIBRATION_MEASURED_SIDE_WALL.as_nanos(),
            maximum_overall_wall_ns: MAX_CALIBRATION_WALL.as_nanos(),
            wall_bound_derivation: if profile.distinct_branch {
                "Uses the current distinct-jz-fallthrough fixture profile and the same 200ms side floor, 16,777,216-call ceiling, 3s measured-side ceiling, 180s process ceiling, and 256-call checkpoint interval. Repetitions are calibrated afresh for both budgets and cases; no legacy-profile timing projection is reused."
            } else {
                "Measured-side ceiling is 3s; the 16,777,216-call and cooperative 180s post-preflight calibration limits remain active. One observed condition (budget4, equal state) measured 181,908,558ns native at 4,194,304 completed calls and 1,000,016,691ns interpreter at 3,582,464 completed calls before the prior 1s ceiling. Linear projection to 8,388,608 calls is 363,817,116ns native and about 2,341,614,044ns interpreter; both exceed the 200ms floor and fit the 3s ceiling. This is a single-condition feasibility estimate, not a claim about other cases or budgets. The 180s limit is checked at 256-call chunk boundaries and after teardown."
            }.into(),
            total_wall_ns: repetitions.total_wall_ns,
            outcome: repetitions
                .failure
                .clone()
                .unwrap_or_else(|| "floor_met_for_every_case_and_side".into()),
            calibration_checks: repetitions.checks,
        });

        if collect_scored_samples(calibration_only, calibration_failed) {
            let (compile_calls, compile_pairs) = measure_compile_pairs(process_run, profile.code)?;
            compile_calls_per_sample = Some(compile_calls);
            compile_pairs_ns_per_call = compile_pairs;
        }

        if collect_scored_samples(calibration_only, calibration_failed) {
            if profile.distinct_branch {
                let (budget4, allocation4) = compile_variant(HOME_BUDGET, profile.code)?;
                let (budget0, allocation0) = compile_variant(SPILL_ONLY_BUDGET, profile.code)?;
                validate_allocation(&allocation4)?;
                validate_allocation(&allocation0)?;
                for (index, case) in cases.iter().enumerate() {
                    case_reports[index].allocator_pairs = collect_allocator_pairs(
                        process_run,
                        &budget4,
                        &budget0,
                        case,
                        &interpreters[index],
                        common_repetitions,
                        index,
                    )?;
                }
            } else {
                for (variant_index, &budget) in budgets.iter().enumerate() {
                    let (compiled, allocation) = compile_variant(budget, profile.code)?;
                    validate_allocation(&allocation)?;
                    if allocation != variant_reports[variant_index].allocation {
                        return Err(fail(format!(
                            "allocator summary changed after preflight for budget {budget}"
                        ))
                        .into());
                    }
                    let publication_started = Instant::now();
                    let published = PublishedCode::publish_block(compiled.emitted())
                        .map_err(|error| fail(format!("publish budget {budget}: {error:?}")))?;
                    let publication_ns = publication_started.elapsed().as_nanos();

                    for case in &cases {
                        for _ in 0..WARMUP_CALLS {
                            let native = native_observation(&published, case)?;
                            let expected = &interpreters
                                [cases.iter().position(|item| item.id == case.id).unwrap()];
                            validate_pair(case, &native, expected)?;
                        }
                    }

                    for (index, case) in cases.iter().enumerate() {
                        let mut samples = Vec::with_capacity(PAIRS);
                        for pair in 0..PAIRS {
                            let interpreter_first =
                                (pair + usize::from(process_run) + index) % 2 == 0;
                            let (interpreter_ns, native_ns) = if interpreter_first {
                                let interpreter_ns = time_interpreter_batch(
                                    case,
                                    &interpreters[index],
                                    common_repetitions,
                                )?;
                                let native_ns = time_native_batch(
                                    &published,
                                    case,
                                    &interpreters[index],
                                    common_repetitions,
                                )?;
                                (interpreter_ns, native_ns)
                            } else {
                                let native_ns = time_native_batch(
                                    &published,
                                    case,
                                    &interpreters[index],
                                    common_repetitions,
                                )?;
                                let interpreter_ns = time_interpreter_batch(
                                    case,
                                    &interpreters[index],
                                    common_repetitions,
                                )?;
                                (interpreter_ns, native_ns)
                            };
                            let interpreter_per_call =
                                interpreter_ns.as_nanos() / common_repetitions as u128;
                            let native_per_call = native_ns.as_nanos() / common_repetitions as u128;
                            samples.push(PairSample {
                                order: if interpreter_first {
                                    "interpreter_then_published_native".into()
                                } else {
                                    "published_native_then_interpreter".into()
                                },
                                interpreter_total_ns: interpreter_ns.as_nanos(),
                                native_total_ns: native_ns.as_nanos(),
                                interpreter_ns_per_call: interpreter_per_call,
                                native_ns_per_call: native_per_call,
                                native_over_interpreter: native_ns.as_nanos() as f64
                                    / interpreter_ns.as_nanos().max(1) as f64,
                                minimum_sample_met: interpreter_ns >= MIN_SAMPLE
                                    && native_ns >= MIN_SAMPLE,
                            });
                        }
                        case_reports[index].execution_pairs.push((budget, samples));
                    }

                    let drop_started = Instant::now();
                    drop(published);
                    let drop_ns = drop_started.elapsed().as_nanos();
                    variant_reports[variant_index].publication_ns = Some(publication_ns);
                    variant_reports[variant_index].drop_ns = Some(drop_ns);
                    variant_reports[variant_index].publication_lifetime =
                "published once; retained for warmup and both cases; dropped before next budget"
                    .into();
                }
            }
        }
    }

    let sample_floor_passed = if preflight_only || calibration_only || calibration_failed {
        None
    } else {
        Some(
            compile_pairs_ns_per_call
                .iter()
                .all(|sample| sample.minimum_sample_met)
                && case_reports
                    .iter()
                    .flat_map(|case| &case.execution_pairs)
                    .flat_map(|(_, samples)| samples)
                    .all(|sample| sample.minimum_sample_met)
                && case_reports
                    .iter()
                    .flat_map(|case| &case.allocator_pairs)
                    .all(|sample| sample.minimum_sample_met),
        )
    };
    let timing_status = if preflight_only {
        "preflight_only_no_timing".into()
    } else if calibration_only && calibration_failed {
        "calibration_only_failed_unscored".into()
    } else if calibration_only {
        "calibration_only_unscored".into()
    } else if calibration_failed {
        "diagnostic_only_calibration_failed_no_scored_comparison".into()
    } else {
        match sample_floor_passed {
            Some(true) => "sample_floor_passed_bounded_diagnostic".into(),
            Some(false) => "diagnostic_only_sample_floor_failed_no_scored_comparison".into(),
            None => "preflight_only_no_timing".into(),
        }
    };

    Ok(Report {
        schema_version: 2,
        fixture_profile: profile.name.into(),
        timing_started: !preflight_only,
        calibration_only_mode: calibration_only,
        sample_floor_passed,
        timing_status,
        process_run,
        host: crate::HostMetadata::current(),
        frozen_code_sha256: code_sha256,
        code_bytes: profile.code.to_vec(),
        guest_base_rip: BASE_RIP,
        instruction_budget: INSTRUCTION_BUDGET,
        scope: if profile.distinct_branch {
            "One frozen six-instruction register-only multi-helper block with distinct JZ taken/fallthrough RIPs; alternating register-homes budget4 versus diagnostic budget0 allocator-configuration pairs. The admitted fallthrough MOV is outside the measured block. This does not isolate marginal spill cost or choose an allocator algorithm globally."
        } else {
            "One frozen six-instruction register-only multi-helper block; budget4 baseline versus budget0 spill-only diagnostic. This does not choose an allocator globally."
        }.into(),
        compile_timer_boundary: "compile_block_with_register_homes: block build/decode, lowering, allocation, ARM64 emission, finalization, and destruction of each unpublished CompiledBlock within the timed compile batch; excludes allocator-summary reconstruction, publication, execution, input reset and validation".into(),
        publication_timer_boundary: "PublishedCode::publish_block and PublishedCode drop are timed and reported separately. The mapping remains live for warmup/timing; drop occurs after execution timing and before the other budget is published. These durations are never added to compile or execution samples.".into(),
        execution_timer_boundary: "Repeated PublishedCode::execute_block calls over one published artifact. The timer includes the safe host wrapper/context setup, generated entry call, and result copy/decode; state-vector creation and full-state/exit validation are outside the timer. The interpreter uses the same initial state and code bytes. For distinct-jz-fallthrough, the single compiled block ends at Jcc; no dispatcher traversal or MOV RBX suffix is timed.".into(),
        cache_and_mapping_policy: "No translation-cache lookup or cache entry. One MAP_JIT PublishedCode mapping at a time; publication is separate from repeated execution; each budget mapping is dropped before publishing the other.".into(),
        exit_normalization: if profile.distinct_branch {
            "Keep native BlockComplete and interpreter InstructionBudgetExhausted tags distinct. Require full LongModeState parity, six retired instructions, zero remaining budget, ZF matching the case, and the case-specific expected RIP (taken code_end; not-taken fallthrough at code_end-10). The native block ends at Jcc; the suffix is not executed or included in timed calls."
        } else {
            "Preserve raw tags. Normalize only BlockComplete versus InstructionBudgetExhausted when complete LongModeState matches, both retire exactly 6, both have zero remaining budget, and RIP equals code_end."
        }.into(),
        branch_scope: profile.branch_scope.into(),
        sample_floor_policy: "Retain every raw sample. A compile or execution side below 200ms sets sample_floor_passed=false and classifies the full process as diagnostic-only; no samples are trimmed and no comparison result should be scored.".into(),
        compile_calls_per_sample,
        compile_pairs_ns_per_call,
        variants: variant_reports,
        calibration,
        cases: case_reports,
    })
}

fn compile_variant(
    register_homes: u8,
    code: &'static [u8],
) -> Result<(CompiledBlock, AllocationReport), Box<dyn std::error::Error>> {
    let view =
        CodeView::new(BASE_RIP, code).map_err(|error| fail(format!("code view: {error:?}")))?;
    let built = build_block(
        view,
        BASE_RIP,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(|error| fail(format!("block build: {error:?}")))?;
    let BuildOutcome::Block(_) = &built else {
        return Err(fail("frozen fixture did not build a block").into());
    };
    let lowered = match lower(&built).map_err(|error| fail(format!("lowering: {error:?}")))? {
        LoweringOutcome::Program(program) => program,
        LoweringOutcome::NoNativePrefix { .. } => {
            return Err(fail("frozen fixture has no native prefix").into());
        }
    };
    let allocation = allocate_lowered_block(
        &lowered,
        AllocatorConfig {
            register_homes,
            reserved_prefix_bytes: RUNTIME_FRAME_PREFIX_BYTES,
        },
    )
    .map_err(|error| fail(format!("allocation summary: {error:?}")))?;
    let locations = allocation.location_counts();
    let counters = allocation.counters();
    let frame = allocation.frame();
    let helper_boundary_count = lowered
        .program
        .operations
        .iter()
        .filter(|operation| {
            matches!(
                operation,
                Operation::Integer { .. }
                    | Operation::CheckedLoad { .. }
                    | Operation::CheckedStore { .. }
            )
        })
        .count();
    drop(allocation);

    let outcome = compile_block_with_register_homes(view, BASE_RIP, register_homes)
        .map_err(|error| fail(format!("compile budget {register_homes}: {error:?}")))?;
    let BlockCompileOutcome::Compiled(compiled) = outcome else {
        return Err(fail("frozen fixture unexpectedly produced no native prefix").into());
    };
    let emitted_code_bytes = usize::try_from(compiled.emitted().pc_map().code_len())
        .map_err(|_| fail("emitted byte count exceeds host size"))?;
    let default_api_code_match = if register_homes == HOME_BUDGET {
        let default = match compile_block(view, BASE_RIP)
            .map_err(|error| fail(format!("default compile: {error:?}")))?
        {
            BlockCompileOutcome::Compiled(default) => default,
            BlockCompileOutcome::NoNativePrefix { .. } => {
                return Err(fail("default compiler unexpectedly emitted no native prefix").into());
            }
        };
        let configured_bytes = compiled
            .emitted()
            .to_le_bytes()
            .map_err(|error| fail(format!("configured emitted bytes: {error:?}")))?;
        let default_bytes = default
            .emitted()
            .to_le_bytes()
            .map_err(|error| fail(format!("default emitted bytes: {error:?}")))?;
        Some(configured_bytes == default_bytes)
    } else {
        None
    };
    let allocation = AllocationReport {
        register_homes_budget: register_homes,
        assigned_register_home_values: locations.register_homes,
        spilled_temporary_values: locations.spill_values,
        value_spill_slots: counters.value_spill_slots,
        value_spill_loads: counters.value_spill_loads,
        value_spill_stores: counters.value_spill_stores,
        helper_capture_slots: counters.helper_capture_slots,
        helper_saves: counters.helper_saves,
        helper_reloads: counters.helper_reloads,
        planned_transfer_words: counters.planned_transfer_words,
        planned_transfer_bytes: counters.planned_transfer_bytes,
        helper_boundary_count,
        frame_bytes: frame.frame_bytes,
        emitted_code_bytes,
        default_api_code_match,
    };
    Ok((compiled, allocation))
}

fn validate_allocation(report: &AllocationReport) -> Result<(), CostError> {
    if report.helper_boundary_count != 5 || report.emitted_code_bytes == 0 {
        return Err(fail(
            "frozen fixture lacks helper boundaries or emitted code",
        ));
    }
    if report.register_homes_budget == HOME_BUDGET {
        if report.assigned_register_home_values == 0
            || report.spilled_temporary_values == 0
            || report.helper_capture_slots == 0
            || report.helper_saves == 0
            || report.helper_reloads == 0
            || report.default_api_code_match != Some(true)
        {
            return Err(fail(
                "budget4 fixture did not exercise homes, spills, and captures",
            ));
        }
    } else if report.register_homes_budget == SPILL_ONLY_BUDGET {
        if report.assigned_register_home_values != 0
            || report.spilled_temporary_values == 0
            || report.value_spill_loads == 0
            || report.value_spill_stores == 0
            || report.helper_capture_slots != 0
            || report.helper_saves != 0
            || report.helper_reloads != 0
        {
            return Err(fail(
                "budget0 fixture did not exercise spill-only allocation",
            ));
        }
    } else {
        return Err(fail(format!(
            "unexpected allocator budget {}",
            report.register_homes_budget
        )));
    }
    Ok(())
}

fn interpreter_observation(case: &Case) -> Result<Observation, CostError> {
    let mut state = case.initial.clone();
    let mut memory = CodeMemory { code: case.code };
    let result = interpreter_run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: INSTRUCTION_BUDGET,
            stop_rip: None,
            context: AccessContext::default(),
        },
        || false,
    );
    Ok(interpreter_report(state, result))
}

fn interpreter_report(state: LongModeState, result: RunResult) -> Observation {
    Observation {
        state: snapshot(&state),
        raw_exit: format!("{:?}", result.exit),
        retired: result.instructions_executed,
        remaining_budget: INSTRUCTION_BUDGET.saturating_sub(result.instructions_executed),
    }
}

fn native_observation(published: &PublishedCode, case: &Case) -> Result<Observation, CostError> {
    let mut state = case.initial.clone();
    let stop_requested = AtomicU8::new(0);
    let result = published
        .execute_block(&mut state, INSTRUCTION_BUDGET, None, &stop_requested)
        .map_err(|error| fail(format!("native execution: {error:?}")))?;
    Ok(Observation {
        state: snapshot(&state),
        raw_exit: format!("{:?}", result.exit),
        retired: result.retired_instructions,
        remaining_budget: result.remaining_budget,
    })
}

fn validate_pair(
    case: &Case,
    native: &Observation,
    interpreter: &Observation,
) -> Result<(), CostError> {
    if native.state != interpreter.state
        || native.raw_exit != format!("{:?}", RuntimeExitTag::BlockComplete)
        || interpreter.raw_exit != format!("{:?}", RunExit::InstructionBudgetExhausted)
        || native.retired != INSTRUCTION_BUDGET
        || interpreter.retired != INSTRUCTION_BUDGET
        || native.remaining_budget != 0
        || interpreter.remaining_budget != 0
        || native.state.rip != case.expected_rip
        || ((native.state.rflags & (1 << 6)) != 0) != case.expected_zf
    {
        return Err(fail(format!(
            "parity failed for {}: native={native:?}, interpreter={interpreter:?}",
            case.id
        )));
    }
    Ok(())
}

fn snapshot(state: &LongModeState) -> StateSnapshot {
    StateSnapshot {
        gpr: state.gpr,
        rip: state.rip,
        rflags: state.rflags,
        fs_base: state.fs_base,
        gs_base: state.gs_base,
        auxiliary_state: format!(
            "fs_selector={:#x};gs_selector={:#x};gdtr={:?};fs_cache={:?};gs_cache={:?};x87={:?}",
            state.fs_selector,
            state.gs_selector,
            state.gdtr,
            state.fs_cache,
            state.gs_cache,
            state.x87
        ),
    }
}

struct CodeMemory {
    code: &'static [u8],
}

impl GuestMemory for CodeMemory {
    fn fetch(&mut self, address: u64, _context: AccessContext) -> Result<u8, MemoryError> {
        let offset = address
            .checked_sub(BASE_RIP)
            .and_then(|n| usize::try_from(n).ok());
        offset
            .and_then(|index| self.code.get(index).copied())
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(
        &mut self,
        address: u64,
        _context: AccessContext,
        _destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }

    fn write(
        &mut self,
        address: u64,
        _context: AccessContext,
        _source: &[u8],
    ) -> Result<(), MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Write,
        })
    }

    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        _context: AccessContext,
        _size: od_isa::OperandSize,
        _replacement: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }

    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        _context: AccessContext,
        _size: od_isa::OperandSize,
        _addend: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        _context: AccessContext,
        _size: od_isa::OperandSize,
        _expected: u64,
        _replacement: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }

    fn mfence(&mut self, _context: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

fn compile_once(register_homes: u8, code: &'static [u8]) -> Result<CompiledBlock, CostError> {
    let view =
        CodeView::new(BASE_RIP, code).map_err(|error| fail(format!("code view: {error:?}")))?;
    let outcome = compile_block_with_register_homes(view, BASE_RIP, register_homes)
        .map_err(|error| fail(format!("compile: {error:?}")))?;
    match outcome {
        BlockCompileOutcome::Compiled(compiled) => Ok(compiled),
        BlockCompileOutcome::NoNativePrefix { .. } => {
            Err(fail("compile produced no native prefix"))
        }
    }
}

fn measure_compile_pairs(
    process_run: u8,
    code: &'static [u8],
) -> Result<(usize, Vec<CompilePairSample>), CostError> {
    let mut calls = 256usize;
    loop {
        let first = compile_batch(HOME_BUDGET, code, calls)?;
        let second = compile_batch(SPILL_ONLY_BUDGET, code, calls)?;
        if first.0 >= MIN_SAMPLE && second.0 >= MIN_SAMPLE {
            break;
        }
        calls = calls
            .checked_mul(2)
            .filter(|n| *n <= MAX_COMPILE_CALLS)
            .ok_or_else(|| fail("compile calibration exceeded safety bound"))?;
    }
    let mut samples = Vec::with_capacity(COMPILE_PAIRS);
    for pair in 0..COMPILE_PAIRS {
        let reverse = (pair + usize::from(process_run)) % 2 != 0;
        let (four, zero) = if reverse {
            let zero = compile_batch(SPILL_ONLY_BUDGET, code, calls)?.0;
            let four = compile_batch(HOME_BUDGET, code, calls)?.0;
            (four, zero)
        } else {
            let four = compile_batch(HOME_BUDGET, code, calls)?.0;
            let zero = compile_batch(SPILL_ONLY_BUDGET, code, calls)?.0;
            (four, zero)
        };
        samples.push(CompilePairSample {
            order: if reverse {
                "budget0_then_budget4".into()
            } else {
                "budget4_then_budget0".into()
            },
            budget4_total_ns: four.as_nanos(),
            budget0_total_ns: zero.as_nanos(),
            budget4_ns_per_compile: four.as_nanos() / calls as u128,
            budget0_ns_per_compile: zero.as_nanos() / calls as u128,
            minimum_sample_met: four >= MIN_SAMPLE && zero >= MIN_SAMPLE,
        });
    }
    Ok((calls, samples))
}

fn compile_batch(
    register_homes: u8,
    code: &'static [u8],
    calls: usize,
) -> Result<(Duration, usize), CostError> {
    let started = Instant::now();
    let mut last_bytes = 0usize;
    for _ in 0..calls {
        let compiled = compile_once(register_homes, code)?;
        last_bytes = usize::try_from(compiled.emitted().pc_map().code_len())
            .map_err(|_| fail("emitted byte count exceeds host size"))?;
        std::hint::black_box(last_bytes);
    }
    Ok((started.elapsed(), last_bytes))
}

fn collect_allocator_pairs(
    process_run: u8,
    budget4: &CompiledBlock,
    budget0: &CompiledBlock,
    case: &Case,
    oracle: &Observation,
    calls: usize,
    case_index: usize,
) -> Result<Vec<AllocatorPairSample>, CostError> {
    let mut samples = Vec::with_capacity(PAIRS);
    for pair in 0..PAIRS {
        let reverse = (pair + usize::from(process_run) + case_index) % 2 != 0;
        let (four, zero) = if reverse {
            let zero = time_allocator_variant_side(budget0, case, oracle, calls)?;
            let four = time_allocator_variant_side(budget4, case, oracle, calls)?;
            (four, zero)
        } else {
            let four = time_allocator_variant_side(budget4, case, oracle, calls)?;
            let zero = time_allocator_variant_side(budget0, case, oracle, calls)?;
            (four, zero)
        };
        samples.push(AllocatorPairSample {
            order: if reverse {
                "budget0_then_budget4_sequential_publications".into()
            } else {
                "budget4_then_budget0_sequential_publications".into()
            },
            calls_per_side: calls,
            budget4_total_ns: four.0.as_nanos(),
            budget0_total_ns: zero.0.as_nanos(),
            budget4_ns_per_call: four.0.as_nanos() / calls as u128,
            budget0_ns_per_call: zero.0.as_nanos() / calls as u128,
            budget0_over_budget4: zero.0.as_nanos() as f64 / four.0.as_nanos().max(1) as f64,
            budget4_publication_ns: four.1,
            budget4_drop_ns: four.2,
            budget0_publication_ns: zero.1,
            budget0_drop_ns: zero.2,
            minimum_sample_met: four.0 >= MIN_SAMPLE && zero.0 >= MIN_SAMPLE,
        });
    }
    Ok(samples)
}

fn time_allocator_variant_side(
    compiled: &CompiledBlock,
    case: &Case,
    oracle: &Observation,
    calls: usize,
) -> Result<(Duration, u128, u128), CostError> {
    let publication_started = Instant::now();
    let published = PublishedCode::publish_block(compiled.emitted())
        .map_err(|error| fail(format!("paired publish: {error:?}")))?;
    let publication_ns = publication_started.elapsed().as_nanos();

    for _ in 0..WARMUP_CALLS {
        let native = native_observation(&published, case)?;
        validate_pair(case, &native, oracle)?;
    }
    let elapsed = time_native_batch(&published, case, oracle, calls)?;
    let drop_started = Instant::now();
    drop(published);
    let drop_ns = drop_started.elapsed().as_nanos();
    Ok((elapsed, publication_ns, drop_ns))
}

fn calibrate_common_repetitions(
    cases: &[Case],
    interpreters: &[Observation],
    budgets: &[u8],
    code: &'static [u8],
    fixture_profile: &'static str,
    journal_path: &Path,
    calibration_started: Instant,
) -> Result<CalibrationRun, CostError> {
    let mut common = CHUNK_CALLS;
    let mut checks = Vec::new();
    let limits = CalibrationLimits {
        deadline: calibration_started + MAX_CALIBRATION_WALL,
    };
    let mut failure = None;
    reset_calibration_journal(journal_path)?;
    for budget in budgets {
        let (compiled, _) = compile_variant(*budget, code).map_err(|e| fail(e.to_string()))?;
        let published = PublishedCode::publish_block(compiled.emitted())
            .map_err(|e| fail(format!("calibration publish: {e:?}")))?;
        for (index, case) in cases.iter().enumerate() {
            let mut calls = CHUNK_CALLS;
            let mut latest_native: Option<(usize, u128)> = None;
            let mut latest_interpreter: Option<(usize, u128)> = None;
            loop {
                let native = time_native_batch_bounded(
                    &published,
                    case,
                    &interpreters[index],
                    calls,
                    Some(limits),
                )?;
                let (interpreter, interpreter_stop, interpreter_calls) =
                    if native.stop_reason.is_some() {
                        (None, None, None)
                    } else {
                        let timed = time_interpreter_batch_bounded(
                            case,
                            &interpreters[index],
                            calls,
                            Some(limits),
                        )?;
                        (
                            Some(timed.elapsed),
                            timed.stop_reason,
                            Some(timed.completed_calls),
                        )
                    };
                let process_wall_ns = calibration_started.elapsed().as_nanos();
                let native_ns = native.elapsed.as_nanos();
                let interpreter_ns = interpreter.map(|value| value.as_nanos());
                if native.completed_calls > 0 {
                    latest_native = Some((native.completed_calls, native_ns));
                }
                if let (Some(calls), Some(ns)) = (interpreter_calls, interpreter_ns)
                    && calls > 0
                {
                    latest_interpreter = Some((calls, ns));
                }
                let floor_met = interpreter
                    .map(|interpreter| both_samples_meet_floor(native.elapsed, interpreter));
                let next_calls = if floor_met == Some(true) {
                    Some(calls)
                } else {
                    next_calibration_call_count(calls)
                };
                let projection = next_calls.filter(|next| *next > calls).and_then(|next| {
                    match (latest_native, latest_interpreter) {
                        (
                            Some((native_calls, native_ns)),
                            Some((interpreter_calls, interpreter_ns)),
                        ) => project_calibration_candidate(
                            next,
                            native_calls,
                            native_ns,
                            interpreter_calls,
                            interpreter_ns,
                        ),
                        _ => None,
                    }
                });
                let stop_reason = native
                    .stop_reason
                    .or(interpreter_stop)
                    .or_else(|| {
                        (process_wall_ns >= MAX_CALIBRATION_WALL.as_nanos())
                            .then(|| "overall_wall_ceiling".into())
                    })
                    .or_else(|| {
                        projection
                            .is_some_and(|next| !next.within_side_ceiling)
                            .then(|| "projected_side_ceiling".into())
                    })
                    .or_else(|| {
                        (floor_met != Some(true) && next_calls.is_none())
                            .then(|| "repetition_ceiling".into())
                    });
                let check = CalibrationCheck {
                    fixture_profile: fixture_profile.into(),
                    record_kind: "attempt".into(),
                    budget: *budget,
                    case_id: case.id.into(),
                    requested_repetitions: calls,
                    native_calls: native.completed_calls,
                    interpreter_calls,
                    native_ns: Some(native_ns),
                    interpreter_ns,
                    projected_next_repetitions: projection.map(|next| next.repetitions),
                    projected_native_ns: projection.map(|next| next.native_ns),
                    projected_interpreter_ns: projection.map(|next| next.interpreter_ns),
                    projected_next_meets_floor: projection.map(|next| next.meets_floor),
                    projected_next_within_side_ceiling: projection
                        .map(|next| next.within_side_ceiling),
                    process_wall_ns,
                    sample_floor_met: floor_met,
                    stop_reason: stop_reason.clone(),
                };
                append_calibration_check(journal_path, &check)?;
                checks.push(check);

                let post_sync_wall = calibration_started.elapsed();
                if post_sync_wall >= MAX_CALIBRATION_WALL && stop_reason.is_none() {
                    persist_terminal_stop(
                        journal_path,
                        &mut checks,
                        fixture_profile,
                        post_sync_wall.as_nanos(),
                        "overall_wall_ceiling_after_journal_sync",
                    )?;
                    failure = Some(format!(
                        "calibration exceeded overall wall ceiling after durable attempt: budget={budget}, case={}, repetitions={calls}, process_wall_ns={}",
                        case.id,
                        post_sync_wall.as_nanos()
                    ));
                    break;
                }

                if let Some(reason) = stop_reason {
                    failure = Some(format!(
                        "calibration stopped: {reason}; budget={budget}, case={}, repetitions={calls}, native_ns={native_ns}, interpreter_ns={interpreter_ns:?}, process_wall_ns={process_wall_ns}",
                        case.id
                    ));
                    break;
                }
                if floor_met == Some(true) {
                    common = common.max(calls);
                    break;
                }
                calls = next_calls.ok_or_else(|| fail("missing next calibration call count"))?;
            }
            if failure.is_some() {
                break;
            }
        }
        drop(published);
        if failure.is_some() {
            break;
        }
    }
    let final_elapsed = calibration_started.elapsed();
    if let Some(reason) = calibration_finalization_stop_reason(failure.is_some(), final_elapsed) {
        persist_terminal_stop(
            journal_path,
            &mut checks,
            fixture_profile,
            final_elapsed.as_nanos(),
            reason,
        )?;
        failure = Some(format!(
            "calibration exceeded overall wall ceiling after final teardown: process_wall_ns={}",
            final_elapsed.as_nanos()
        ));
    }
    Ok(CalibrationRun {
        common_repetitions: failure.is_none().then_some(common),
        checks,
        total_wall_ns: final_elapsed.as_nanos(),
        failure,
    })
}

fn reset_calibration_journal(path: &Path) -> Result<(), CostError> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|error| fail(format!("create calibration journal: {error}")))?;
    file.sync_data()
        .map_err(|error| fail(format!("sync calibration journal initialization: {error}")))?;
    Ok(())
}

fn append_calibration_check(path: &Path, check: &CalibrationCheck) -> Result<(), CostError> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| fail(format!("open calibration journal: {error}")))?;
    let mut encoded = serde_json::to_vec(check)
        .map_err(|error| fail(format!("serialize calibration check: {error}")))?;
    encoded.push(b'\n');
    file.write_all(&encoded)
        .map_err(|error| fail(format!("append calibration journal: {error}")))?;
    file.sync_data()
        .map_err(|error| fail(format!("sync calibration journal: {error}")))?;
    Ok(())
}

fn persist_terminal_stop(
    path: &Path,
    checks: &mut Vec<CalibrationCheck>,
    fixture_profile: &str,
    process_wall_ns: u128,
    reason: &str,
) -> Result<(), CostError> {
    let previous = checks.last().cloned();
    let mut terminal = previous.unwrap_or_else(|| CalibrationCheck {
        fixture_profile: fixture_profile.into(),
        record_kind: "terminal_stop".into(),
        budget: 0,
        case_id: "calibration".into(),
        requested_repetitions: 0,
        native_calls: 0,
        interpreter_calls: None,
        native_ns: None,
        interpreter_ns: None,
        projected_next_repetitions: None,
        projected_native_ns: None,
        projected_interpreter_ns: None,
        projected_next_meets_floor: None,
        projected_next_within_side_ceiling: None,
        process_wall_ns,
        sample_floor_met: None,
        stop_reason: None,
    });
    terminal.record_kind = "terminal_stop".into();
    terminal.requested_repetitions = 0;
    terminal.native_calls = 0;
    terminal.interpreter_calls = None;
    terminal.native_ns = None;
    terminal.interpreter_ns = None;
    terminal.process_wall_ns = process_wall_ns;
    terminal.sample_floor_met = None;
    terminal.stop_reason = Some(reason.into());
    append_calibration_check(path, &terminal)?;
    checks.push(terminal);
    Ok(())
}

fn sample_meets_floor(sample: Duration) -> bool {
    sample >= MIN_SAMPLE
}

fn both_samples_meet_floor(first: Duration, second: Duration) -> bool {
    sample_meets_floor(first) && sample_meets_floor(second)
}

fn collect_scored_samples(calibration_only: bool, calibration_failed: bool) -> bool {
    !calibration_only && !calibration_failed
}

pub fn report_requires_nonzero_exit(report: &serde_json::Value) -> bool {
    let floor_failed = report
        .get("sample_floor_passed")
        .and_then(serde_json::Value::as_bool)
        == Some(false);
    let calibration_failed = report
        .get("timing_status")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|status| status.contains("calibration") && status.contains("failed"));
    floor_failed || calibration_failed
}

fn next_calibration_call_count(calls: usize) -> Option<usize> {
    calls
        .checked_mul(2)
        .filter(|next| *next <= MAX_CALIBRATED_CALLS)
}

#[derive(Clone, Copy)]
struct CalibrationProjection {
    repetitions: usize,
    native_ns: u128,
    interpreter_ns: u128,
    meets_floor: bool,
    within_side_ceiling: bool,
}

fn project_calibration_candidate(
    next_repetitions: usize,
    native_calls: usize,
    native_ns: u128,
    interpreter_calls: usize,
    interpreter_ns: u128,
) -> Option<CalibrationProjection> {
    let native_projected = project_elapsed_ns(native_ns, native_calls, next_repetitions)?;
    let interpreter_projected =
        project_elapsed_ns(interpreter_ns, interpreter_calls, next_repetitions)?;
    Some(CalibrationProjection {
        repetitions: next_repetitions,
        native_ns: native_projected,
        interpreter_ns: interpreter_projected,
        meets_floor: native_projected >= MIN_SAMPLE.as_nanos()
            && interpreter_projected >= MIN_SAMPLE.as_nanos(),
        within_side_ceiling: native_projected <= MAX_CALIBRATION_MEASURED_SIDE_WALL.as_nanos()
            && interpreter_projected <= MAX_CALIBRATION_MEASURED_SIDE_WALL.as_nanos(),
    })
}

fn project_elapsed_ns(sample_ns: u128, sample_calls: usize, target_calls: usize) -> Option<u128> {
    if sample_calls == 0 {
        return None;
    }
    let numerator = sample_ns.checked_mul(target_calls as u128)?;
    let denominator = sample_calls as u128;
    numerator
        .checked_add(denominator - 1)
        .map(|rounded| rounded / denominator)
}

#[derive(Clone, Copy)]
struct CalibrationLimits {
    deadline: Instant,
}

#[derive(Clone)]
struct TimedBatch {
    elapsed: Duration,
    completed_calls: usize,
    stop_reason: Option<String>,
}

fn first_compile_calibration_stop_reason(
    candidate_first: bool,
    baseline: &TimedBatch,
    candidate: &TimedBatch,
) -> Option<String> {
    let ordered = if candidate_first {
        [candidate, baseline]
    } else {
        [baseline, candidate]
    };
    ordered
        .into_iter()
        .find_map(|batch| batch.stop_reason.clone())
}

fn calibration_stop_reason(limits: Option<CalibrationLimits>, elapsed: Duration) -> Option<String> {
    let limits = limits?;
    if Instant::now() >= limits.deadline {
        Some("overall_wall_ceiling".into())
    } else if elapsed >= MAX_CALIBRATION_MEASURED_SIDE_WALL {
        Some("per_side_wall_ceiling".into())
    } else {
        None
    }
}

fn calibration_finalization_stop_reason(
    already_failed: bool,
    elapsed: Duration,
) -> Option<&'static str> {
    (!already_failed && elapsed >= MAX_CALIBRATION_WALL)
        .then_some("overall_wall_ceiling_after_calibration_teardown")
}

fn time_native_batch(
    published: &PublishedCode,
    case: &Case,
    oracle: &Observation,
    calls: usize,
) -> Result<Duration, CostError> {
    let timed = time_native_batch_bounded(published, case, oracle, calls, None)?;
    Ok(timed.elapsed)
}

fn time_native_batch_bounded(
    published: &PublishedCode,
    case: &Case,
    oracle: &Observation,
    calls: usize,
    limits: Option<CalibrationLimits>,
) -> Result<TimedBatch, CostError> {
    let mut elapsed = Duration::ZERO;
    let mut completed_calls = 0;
    let mut remaining = calls;
    while remaining > 0 {
        if let Some(stop_reason) = calibration_stop_reason(limits, elapsed) {
            return Ok(TimedBatch {
                elapsed,
                completed_calls,
                stop_reason: Some(stop_reason),
            });
        }
        let count = remaining.min(CHUNK_CALLS);
        let mut states = vec![case.initial.clone(); count];
        let mut results = Vec::with_capacity(count);
        let stop_requested = AtomicU8::new(0);
        let started = Instant::now();
        for state in &mut states {
            results.push(
                published
                    .execute_block(state, INSTRUCTION_BUDGET, None, &stop_requested)
                    .map_err(|error| fail(format!("native timed execution: {error:?}")))?,
            );
        }
        elapsed += started.elapsed();
        for (state, result) in states.iter().zip(results) {
            if snapshot(state) != oracle.state {
                return Err(fail(format!("native timed state mismatch for {}", case.id)));
            }
            if result.exit != RuntimeExitTag::BlockComplete
                || result.retired_instructions != INSTRUCTION_BUDGET
                || result.remaining_budget != 0
            {
                return Err(fail(format!("native timed exit mismatch: {result:?}")));
            }
        }
        remaining -= count;
        completed_calls += count;
        if let Some(stop_reason) = calibration_stop_reason(limits, elapsed) {
            return Ok(TimedBatch {
                elapsed,
                completed_calls,
                stop_reason: Some(stop_reason),
            });
        }
    }
    Ok(TimedBatch {
        elapsed,
        completed_calls,
        stop_reason: None,
    })
}

fn time_interpreter_batch(
    case: &Case,
    oracle: &Observation,
    calls: usize,
) -> Result<Duration, CostError> {
    let timed = time_interpreter_batch_bounded(case, oracle, calls, None)?;
    Ok(timed.elapsed)
}

fn time_interpreter_batch_bounded(
    case: &Case,
    oracle: &Observation,
    calls: usize,
    limits: Option<CalibrationLimits>,
) -> Result<TimedBatch, CostError> {
    let mut elapsed = Duration::ZERO;
    let mut completed_calls = 0;
    let mut remaining = calls;
    while remaining > 0 {
        if let Some(stop_reason) = calibration_stop_reason(limits, elapsed) {
            return Ok(TimedBatch {
                elapsed,
                completed_calls,
                stop_reason: Some(stop_reason),
            });
        }
        let count = remaining.min(CHUNK_CALLS);
        let mut states = vec![case.initial.clone(); count];
        let mut results = Vec::with_capacity(count);
        let mut memory = CodeMemory { code: case.code };
        let started = Instant::now();
        for state in &mut states {
            results.push(interpreter_run(
                state,
                &mut memory,
                RunConfig {
                    instruction_budget: INSTRUCTION_BUDGET,
                    stop_rip: None,
                    context: AccessContext::default(),
                },
                || false,
            ));
        }
        elapsed += started.elapsed();
        for (state, result) in states.iter().zip(results) {
            if snapshot(state) != oracle.state {
                return Err(fail(format!(
                    "interpreter timed state mismatch for {}",
                    case.id
                )));
            }
            if result.exit != RunExit::InstructionBudgetExhausted
                || result.instructions_executed != INSTRUCTION_BUDGET
            {
                return Err(fail(format!(
                    "interpreter timed exit mismatch: {:?}",
                    result.exit
                )));
            }
        }
        remaining -= count;
        completed_calls += count;
        if let Some(stop_reason) = calibration_stop_reason(limits, elapsed) {
            return Ok(TimedBatch {
                elapsed,
                completed_calls,
                stop_reason: Some(stop_reason),
            });
        }
    }
    Ok(TimedBatch {
        elapsed,
        completed_calls,
        stop_reason: None,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        BASE_RIP, CHUNK_CALLS, CODE, CalibrationCheck, CalibrationLimits, DISTINCT_BRANCH_CODE,
        DISTINCT_BRANCH_CODE_SHA256, DISTINCT_BRANCH_PROFILE, HOME_BUDGET, INSTRUCTION_BUDGET,
        MAX_CALIBRATED_CALLS, MAX_CALIBRATION_MEASURED_SIDE_WALL, MAX_CALIBRATION_WALL, MIN_SAMPLE,
        Observation, PAIRS, S3_32aCaseReport, S3_32aReport, S3_32aVariant, StateSnapshot,
        TimedBatch, WARMUP_CALLS, append_calibration_check, both_samples_meet_floor,
        calibration_finalization_stop_reason, calibration_stop_reason, cases,
        collect_scored_samples, compile_s3_32a_variant, first_compile_calibration_stop_reason,
        interpreter_observation, next_calibration_call_count, persist_terminal_stop,
        project_calibration_candidate, report_requires_nonzero_exit, reset_calibration_journal,
        s3_32a_calibration_sample, sample_meets_floor, time_s3_32a_compile_batch_bounded,
        time_s3_32a_native_side_bounded,
    };
    use crate::calibration_report::calibration_only_report_succeeded;
    use std::fs;
    use std::time::{Duration, Instant};

    #[test]
    fn distinct_branch_profile_is_frozen_and_has_explicit_taken_and_fallthrough_rips() {
        assert_eq!(&DISTINCT_BRANCH_CODE[..15], &CODE[..15]);
        assert_eq!(&DISTINCT_BRANCH_CODE[15..17], &[0x74, 0x0A]);
        assert_eq!(
            &DISTINCT_BRANCH_CODE[17..],
            &[0x48, 0xBB, 0x11, 0x00, 0xFF, 0xEE, 0xDD, 0xCC, 0xBB, 0xAA]
        );
        assert_eq!(
            od_testkit::sha256_hex(&DISTINCT_BRANCH_CODE),
            DISTINCT_BRANCH_CODE_SHA256
        );

        let branch_cases = cases(DISTINCT_BRANCH_PROFILE);
        assert_eq!(branch_cases.len(), 2);
        assert_eq!(branch_cases[0].expected_zf, true);
        assert_eq!(branch_cases[0].expected_rip, BASE_RIP + 27);
        assert_eq!(branch_cases[1].expected_zf, false);
        assert_eq!(branch_cases[1].expected_rip, BASE_RIP + 17);

        assert_eq!(HOME_BUDGET, 4);
        assert_eq!(PAIRS, 20);
        assert_eq!(WARMUP_CALLS, 64);
    }

    #[test]
    fn s3_32a_calibration_only_validator_accepts_actual_serialized_report_shape() {
        let state = StateSnapshot {
            gpr: [0; 16],
            rip: BASE_RIP,
            rflags: 0x202,
            fs_base: 0,
            gs_base: 0,
            auxiliary_state: String::new(),
        };
        let observation = Observation {
            state: state.clone(),
            raw_exit: "BlockComplete".into(),
            retired: INSTRUCTION_BUDGET,
            remaining_budget: 0,
        };
        let report = S3_32aReport {
            schema_version: 1,
            experiment: "s3-32a-aliased-add-rax-rax-baseline-vs-candidate".into(),
            process_run: 1,
            host: crate::HostMetadata::current(),
            timing_status: "calibration_only_unscored".into(),
            preflight_passed: true,
            sample_floor_passed: None,
            calibration_floor_passed: Some(true),
            source_fixture_sha256: DISTINCT_BRANCH_CODE_SHA256.into(),
            code_bytes: DISTINCT_BRANCH_CODE.to_vec(),
            guest_base_rip: BASE_RIP,
            instruction_budget: INSTRUCTION_BUDGET,
            register_homes: HOME_BUDGET,
            eager_flags: true,
            cache_policy: "uncached".into(),
            compile_timer_boundary: "compile and drop".into(),
            publication_timer_boundary: "publish and drop separately".into(),
            native_timer_boundary: "repeated native calls".into(),
            sample_floor_ns: MIN_SAMPLE.as_nanos(),
            warmups_per_publication: WARMUP_CALLS,
            pairs_per_stratum: PAIRS,
            common_compile_calls_per_side: Some(1),
            common_native_calls_per_side: Some(1),
            calibration_total_wall_ns: Some(1),
            calibration_deadline_ns: MAX_CALIBRATION_WALL.as_nanos(),
            calibration_side_ceiling_ns: MAX_CALIBRATION_MEASURED_SIDE_WALL.as_nanos(),
            calibration_chunk_calls: CHUNK_CALLS,
            calibration_stop_reason: None,
            compile_calibration: Vec::new(),
            native_calibration: Vec::new(),
            variant_profiles: Vec::new(),
            compile_pairs: Vec::new(),
            cases: vec![S3_32aCaseReport {
                id: "jz-taken".into(),
                initial: state,
                interpreter_oracle: observation.clone(),
                baseline_preflight: observation.clone(),
                candidate_preflight: observation,
                native_pairs: Vec::new(),
            }],
        };
        let encoded = serde_json::to_value(&report).expect("serialize S3-32a report");
        assert!(encoded.get("status").is_none());
        assert_eq!(encoded["timing_status"], "calibration_only_unscored");
        assert_eq!(
            encoded["calibration_deadline_ns"],
            serde_json::json!(MAX_CALIBRATION_WALL.as_nanos())
        );
        assert_eq!(
            encoded["calibration_side_ceiling_ns"],
            serde_json::json!(MAX_CALIBRATION_MEASURED_SIDE_WALL.as_nanos())
        );
        assert_eq!(encoded["calibration_chunk_calls"], CHUNK_CALLS);
        assert_eq!(encoded["calibration_stop_reason"], serde_json::Value::Null);
        assert!(calibration_only_report_succeeded(&encoded));

        let mut compile_samples = encoded.clone();
        compile_samples["compile_pairs"] = serde_json::json!([{"pair_index": 0}]);
        assert!(!calibration_only_report_succeeded(&compile_samples));

        let mut native_samples = encoded.clone();
        native_samples["cases"][0]["native_pairs"] = serde_json::json!([{"pair_index": 0}]);
        assert!(!calibration_only_report_succeeded(&native_samples));

        let mut failed = encoded;
        failed["calibration_floor_passed"] = serde_json::json!(false);
        failed["timing_status"] = serde_json::json!("calibration_only_failed_unscored");
        assert!(!calibration_only_report_succeeded(&failed));
    }

    #[test]
    fn s3_32a_shared_deadline_stops_compile_and_native_calibration_before_work() {
        let view = od_jit::block::CodeView::new(BASE_RIP, &DISTINCT_BRANCH_CODE).unwrap();
        let admission = od_jit::block::S3_32aAddRaxRaxAdmission::from_code(view).unwrap();
        let process_started = Instant::now();
        let limits = CalibrationLimits {
            deadline: process_started - Duration::from_nanos(1),
        };

        let compile = time_s3_32a_compile_batch_bounded(
            S3_32aVariant::Baseline,
            view,
            &admission,
            CHUNK_CALLS,
            limits,
        )
        .unwrap();
        assert_eq!(compile.completed_calls, 0);
        assert_eq!(compile.stop_reason.as_deref(), Some("overall_wall_ceiling"));
        let compile_sample = s3_32a_calibration_sample(
            S3_32aVariant::Baseline,
            None,
            "baseline_then_candidate",
            CHUNK_CALLS,
            compile,
            process_started,
            limits,
        );
        assert!(!compile_sample.minimum_sample_met);
        assert_eq!(
            compile_sample.process_deadline_ns,
            MAX_CALIBRATION_WALL.as_nanos()
        );
        assert_eq!(
            compile_sample.side_ceiling_ns,
            MAX_CALIBRATION_MEASURED_SIDE_WALL.as_nanos()
        );
        assert_eq!(
            compile_sample.stop_reason.as_deref(),
            Some("overall_wall_ceiling")
        );

        let compiled = compile_s3_32a_variant(S3_32aVariant::Baseline, view, &admission).unwrap();
        let case = cases(DISTINCT_BRANCH_PROFILE).remove(0);
        let oracle = interpreter_observation(&case).unwrap();
        let native =
            time_s3_32a_native_side_bounded(&compiled, &case, &oracle, CHUNK_CALLS, limits)
                .unwrap();
        assert_eq!(native.completed_calls, 0);
        assert_eq!(
            native.stop_reason.as_deref(),
            Some("overall_wall_ceiling_before_publish")
        );
    }

    #[test]
    fn candidate_first_compile_stop_reports_the_executed_candidate_failure() {
        let baseline = TimedBatch {
            elapsed: Duration::ZERO,
            completed_calls: 0,
            stop_reason: Some("skipped_after_candidate_calibration_stop".into()),
        };
        let candidate = TimedBatch {
            elapsed: Duration::ZERO,
            completed_calls: 0,
            stop_reason: Some("overall_wall_ceiling".into()),
        };

        assert_eq!(
            first_compile_calibration_stop_reason(true, &baseline, &candidate).as_deref(),
            Some("overall_wall_ceiling")
        );
    }

    #[test]
    fn calibration_floor_and_doubling_boundaries_are_exact() {
        assert!(!sample_meets_floor(MIN_SAMPLE - Duration::from_nanos(1)));
        assert!(sample_meets_floor(MIN_SAMPLE));
        assert!(sample_meets_floor(MIN_SAMPLE + Duration::from_nanos(1)));

        assert_eq!(next_calibration_call_count(256), Some(512));
        assert_eq!(
            next_calibration_call_count(MAX_CALIBRATED_CALLS / 2),
            Some(MAX_CALIBRATED_CALLS)
        );
        assert_eq!(next_calibration_call_count(MAX_CALIBRATED_CALLS), None);

        assert_eq!(MAX_CALIBRATED_CALLS / CHUNK_CALLS, 1 << 16);
        assert_eq!(
            calibration_stop_reason(
                Some(CalibrationLimits {
                    deadline: Instant::now() + Duration::from_secs(1),
                }),
                MAX_CALIBRATION_MEASURED_SIDE_WALL,
            )
            .as_deref(),
            Some("per_side_wall_ceiling")
        );
        assert_eq!(
            calibration_stop_reason(
                Some(CalibrationLimits {
                    deadline: Instant::now(),
                }),
                Duration::ZERO,
            )
            .as_deref(),
            Some("overall_wall_ceiling")
        );
    }

    #[test]
    fn calibration_success_requires_both_sides_to_meet_floor() {
        assert!(!both_samples_meet_floor(
            MIN_SAMPLE - Duration::from_nanos(1),
            MIN_SAMPLE
        ));
        assert!(!both_samples_meet_floor(
            MIN_SAMPLE,
            MIN_SAMPLE - Duration::from_nanos(1)
        ));
        assert!(both_samples_meet_floor(MIN_SAMPLE, MIN_SAMPLE));
    }

    #[test]
    fn next_candidate_projection_uses_actual_partial_calls_and_both_sides() {
        let projection = project_calibration_candidate(
            8_388_608,
            4_194_304,
            181_908_558,
            3_582_464,
            1_000_016_691,
        )
        .expect("both measured sides have completed calls");
        assert_eq!(projection.native_ns, 363_817_116);
        assert_eq!(projection.interpreter_ns, 2_341_614_044);
        assert!(projection.meets_floor);
        assert!(projection.within_side_ceiling);

        let infeasible = project_calibration_candidate(
            8_388_608,
            4_194_304,
            200_000_000,
            4_194_304,
            1_700_000_000,
        )
        .expect("both measured sides have completed calls");
        assert!(infeasible.meets_floor);
        assert!(!infeasible.within_side_ceiling);
    }

    #[test]
    fn calibration_only_and_failed_calibration_never_collect_scores() {
        assert!(!collect_scored_samples(true, false));
        assert!(!collect_scored_samples(true, true));
        assert!(!collect_scored_samples(false, true));
        assert!(collect_scored_samples(false, false));
    }

    #[test]
    fn final_teardown_deadline_classifies_before_at_and_after_boundary() {
        assert_eq!(
            calibration_finalization_stop_reason(
                false,
                MAX_CALIBRATION_WALL - Duration::from_nanos(1)
            ),
            None
        );
        assert_eq!(
            calibration_finalization_stop_reason(false, MAX_CALIBRATION_WALL),
            Some("overall_wall_ceiling_after_calibration_teardown")
        );
        assert_eq!(
            calibration_finalization_stop_reason(
                false,
                MAX_CALIBRATION_WALL + Duration::from_nanos(1)
            ),
            Some("overall_wall_ceiling_after_calibration_teardown")
        );
        assert_eq!(
            calibration_finalization_stop_reason(true, MAX_CALIBRATION_WALL),
            None
        );
    }

    #[test]
    fn failed_calibration_attempt_is_durable_and_requires_nonzero_exit() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time is after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "od-bench-calibration-{}-{unique}.jsonl",
            std::process::id()
        ));
        reset_calibration_journal(&path).expect("initialize journal");
        let attempt = CalibrationCheck {
            fixture_profile: "test-fixture".into(),
            record_kind: "attempt".into(),
            budget: 0,
            case_id: "frozen-case".into(),
            requested_repetitions: MAX_CALIBRATED_CALLS,
            native_calls: MAX_CALIBRATED_CALLS,
            interpreter_calls: Some(MAX_CALIBRATED_CALLS),
            native_ns: Some(120_000_000),
            interpreter_ns: Some(180_000_000),
            projected_next_repetitions: None,
            projected_native_ns: None,
            projected_interpreter_ns: None,
            projected_next_meets_floor: None,
            projected_next_within_side_ceiling: None,
            process_wall_ns: 1_000_000_000,
            sample_floor_met: Some(false),
            stop_reason: Some("repetition_ceiling".into()),
        };
        append_calibration_check(&path, &attempt).expect("append diagnostic attempt");
        let mut checks = vec![attempt];
        persist_terminal_stop(
            &path,
            &mut checks,
            "test-fixture",
            MAX_CALIBRATION_WALL.as_nanos() + 1,
            "overall_wall_ceiling_after_calibration_teardown",
        )
        .expect("persist terminal deadline event");
        let encoded = fs::read_to_string(&path).expect("read durable journal");
        let lines = encoded.lines().collect::<Vec<_>>();
        let saved: serde_json::Value =
            serde_json::from_str(lines[0]).expect("attempt line is complete JSON");
        assert_eq!(saved["requested_repetitions"], MAX_CALIBRATED_CALLS);
        assert_eq!(saved["native_calls"], MAX_CALIBRATED_CALLS);
        assert_eq!(saved["interpreter_calls"], MAX_CALIBRATED_CALLS);
        assert_eq!(saved["native_ns"], 120_000_000u64);
        assert_eq!(saved["interpreter_ns"], 180_000_000u64);
        assert_eq!(saved["stop_reason"], "repetition_ceiling");
        let terminal: serde_json::Value =
            serde_json::from_str(lines[1]).expect("terminal line is complete JSON");
        assert_eq!(terminal["record_kind"], "terminal_stop");
        assert_eq!(
            terminal["process_wall_ns"],
            (MAX_CALIBRATION_WALL.as_nanos() + 1) as u64
        );
        assert_eq!(
            terminal["stop_reason"],
            "overall_wall_ceiling_after_calibration_teardown"
        );

        let report = serde_json::json!({
            "sample_floor_passed": null,
            "timing_status": "diagnostic_only_calibration_failed_no_scored_comparison"
        });
        assert!(report_requires_nonzero_exit(&report));
        let sample_floor_report = serde_json::json!({
            "sample_floor_passed": false,
            "timing_status": "diagnostic_only_sample_floor_failed_no_scored_comparison"
        });
        assert!(report_requires_nonzero_exit(&sample_floor_report));
        fs::remove_file(path).expect("remove temporary journal");
    }
}
