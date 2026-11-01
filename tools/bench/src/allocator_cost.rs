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
use od_jit::block::{BlockLimits, BuildOutcome, CodeView, build_block};
use od_jit::block_compile::{
    BlockCompileOutcome, CompiledBlock, RUNTIME_FRAME_PREFIX_BYTES, allocate_lowered_block,
    compile_block, compile_block_with_register_homes,
};
use od_jit::ir::Operation;
use od_jit::lowering::{LoweringOutcome, lower};
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

struct TimedBatch {
    elapsed: Duration,
    completed_calls: usize,
    stop_reason: Option<String>,
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
        DISTINCT_BRANCH_CODE_SHA256, DISTINCT_BRANCH_PROFILE, MAX_CALIBRATED_CALLS,
        MAX_CALIBRATION_MEASURED_SIDE_WALL, MAX_CALIBRATION_WALL, MIN_SAMPLE,
        append_calibration_check, both_samples_meet_floor, calibration_finalization_stop_reason,
        calibration_stop_reason, cases, collect_scored_samples, next_calibration_call_count,
        persist_terminal_stop, project_calibration_candidate, report_requires_nonzero_exit,
        reset_calibration_journal, sample_meets_floor,
    };
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
        fs::remove_file(path).expect("remove temporary journal");
    }
}
