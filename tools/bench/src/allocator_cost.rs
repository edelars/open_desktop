//! Opt-in comparison of the existing fixed-home allocator at budgets four and zero.
//!
//! This is a single-block diagnostic. It uses no translation cache and retains
//! only one MAP_JIT publication at a time.

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
const MAX_CALIBRATED_CALLS: usize = 1 << 22;
const COMPILE_PAIRS: usize = 20;
const MAX_COMPILE_CALLS: usize = 1 << 20;
const WARMUP_CALLS: usize = 64;
const FROZEN_CODE_SHA256: &str = "4fe980f463d01c3fc413943b73860314d9c780c638acdc9bf50072290c89f778";
const CODE: [u8; 17] = [
    0x48, 0x03, 0xC0, // ADD RAX, RAX
    0x48, 0x2B, 0xD2, // SUB RDX, RDX
    0x48, 0x03, 0xC3, // ADD RAX, RBX
    0x48, 0x2B, 0xC9, // SUB RCX, RCX
    0x48, 0x3B, 0xC3, // CMP RAX, RBX
    0x74, 0x00, // JZ code_end; taken and fall-through converge
];

#[derive(Clone)]
struct Case {
    id: &'static str,
    initial: LongModeState,
    expected_zf: bool,
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
    interpreter_oracle: Observation,
    preflight_native: Vec<(u8, Observation)>,
    execution_pairs: Vec<(u8, Vec<PairSample>)>,
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
    common_repetitions_per_case_and_side: usize,
    warmup_calls_per_case: usize,
    sample_pairs: usize,
    minimum_sample_ns: u128,
    calibration_checks_ns: Vec<(u8, String, u128, u128)>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    schema_version: u32,
    timing_started: bool,
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

fn cases() -> Vec<Case> {
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

    vec![
        Case {
            id: "multi-helper-equal",
            initial: equal,
            expected_zf: true,
        },
        Case {
            id: "multi-helper-not-equal",
            initial: unequal,
            expected_zf: false,
        },
    ]
}

pub fn preflight(process_run: u8) -> Result<Report, Box<dyn std::error::Error>> {
    build_report(process_run, true)
}

pub fn run(process_run: u8) -> Result<Report, Box<dyn std::error::Error>> {
    build_report(process_run, false)
}

fn build_report(
    process_run: u8,
    preflight_only: bool,
) -> Result<Report, Box<dyn std::error::Error>> {
    if !(1..=3).contains(&process_run) {
        return Err(fail("process-run must be 1..=3").into());
    }
    let code_sha256 = od_testkit::sha256_hex(&CODE);
    if code_sha256 != FROZEN_CODE_SHA256 {
        return Err(fail("frozen helper fixture digest mismatch").into());
    }
    let cases = cases();
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
            interpreter_oracle: oracle.clone(),
            preflight_native: Vec::new(),
            execution_pairs: Vec::new(),
        })
        .collect::<Vec<_>>();

    let mut calibration = None;
    let mut compile_calls_per_sample = None;
    let mut compile_pairs_ns_per_call = Vec::new();
    let mut budgets = vec![HOME_BUDGET, SPILL_ONLY_BUDGET];
    if process_run % 2 == 0 {
        budgets.reverse();
    }

    // Complete both variant parity and allocation gates before any timing
    // calibration or sample collection.
    for &budget in &budgets {
        let (compiled, allocation) = compile_variant(budget)?;
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
        let (compile_calls, compile_pairs) = measure_compile_pairs(process_run)?;
        compile_calls_per_sample = Some(compile_calls);
        compile_pairs_ns_per_call = compile_pairs;
        let repetitions = calibrate_common_repetitions(&cases, &interpreters, &budgets)?;
        let calibration_checks = repetitions.1;
        let common_repetitions = repetitions.0;

        for (variant_index, &budget) in budgets.iter().enumerate() {
            let (compiled, allocation) = compile_variant(budget)?;
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
                    let expected =
                        &interpreters[cases.iter().position(|item| item.id == case.id).unwrap()];
                    validate_pair(case, &native, expected)?;
                }
            }

            for (index, case) in cases.iter().enumerate() {
                let mut samples = Vec::with_capacity(PAIRS);
                for pair in 0..PAIRS {
                    let interpreter_first = (pair + usize::from(process_run) + index) % 2 == 0;
                    let (interpreter_ns, native_ns) = if interpreter_first {
                        let interpreter_ns =
                            time_interpreter_batch(case, &interpreters[index], common_repetitions)?;
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
                        let interpreter_ns =
                            time_interpreter_batch(case, &interpreters[index], common_repetitions)?;
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
                        minimum_sample_met: interpreter_ns >= MIN_SAMPLE && native_ns >= MIN_SAMPLE,
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
        calibration = Some(CalibrationReport {
            common_repetitions_per_case_and_side: common_repetitions,
            warmup_calls_per_case: WARMUP_CALLS,
            sample_pairs: PAIRS,
            minimum_sample_ns: MIN_SAMPLE.as_nanos(),
            calibration_checks_ns: calibration_checks,
        });
    }

    let sample_floor_passed = if preflight_only {
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
                    .all(|sample| sample.minimum_sample_met),
        )
    };
    let timing_status = match sample_floor_passed {
        None => "preflight_only_no_timing".into(),
        Some(true) => "sample_floor_passed_bounded_diagnostic".into(),
        Some(false) => "diagnostic_only_sample_floor_failed_no_scored_comparison".into(),
    };

    Ok(Report {
        schema_version: 1,
        timing_started: !preflight_only,
        sample_floor_passed,
        timing_status,
        process_run,
        host: crate::HostMetadata::current(),
        frozen_code_sha256: code_sha256,
        code_bytes: CODE.to_vec(),
        guest_base_rip: BASE_RIP,
        instruction_budget: INSTRUCTION_BUDGET,
        scope: "One frozen six-instruction register-only multi-helper block; budget4 baseline versus budget0 spill-only diagnostic. This does not choose an allocator globally.".into(),
        compile_timer_boundary: "compile_block_with_register_homes: block build/decode, lowering, allocation, ARM64 emission, finalization, and destruction of each unpublished CompiledBlock within the timed compile batch; excludes allocator-summary reconstruction, publication, execution, input reset and validation".into(),
        publication_timer_boundary: "PublishedCode::publish_block and PublishedCode drop are timed and reported separately. The mapping remains live for warmup/timing; drop occurs after execution timing and before the other budget is published. These durations are never added to compile or execution samples.".into(),
        execution_timer_boundary: "Repeated PublishedCode::execute_block calls over the same retained artifact; state vectors are initialized before timing, and full-state/exit validation is outside timing. Interpreter uses the same initial state and code fetch adapter.".into(),
        cache_and_mapping_policy: "No translation-cache lookup or cache entry. One MAP_JIT PublishedCode mapping at a time; publication is separate from repeated execution; each budget mapping is dropped before publishing the other.".into(),
        exit_normalization: "Preserve raw tags. Normalize only BlockComplete versus InstructionBudgetExhausted when complete LongModeState matches, both retire exactly 6, both have zero remaining budget, and RIP equals code_end.".into(),
        branch_scope: "Final JZ is checked via ZF=1/0, but its displacement is zero, so taken and fall-through both target code_end; this fixture does not test distinct branch routing.".into(),
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
) -> Result<(CompiledBlock, AllocationReport), Box<dyn std::error::Error>> {
    let view =
        CodeView::new(BASE_RIP, &CODE).map_err(|error| fail(format!("code view: {error:?}")))?;
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
    let mut memory = CodeMemory;
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
    let end = BASE_RIP + CODE.len() as u64;
    if native.state != interpreter.state
        || native.raw_exit != format!("{:?}", RuntimeExitTag::BlockComplete)
        || interpreter.raw_exit != format!("{:?}", RunExit::InstructionBudgetExhausted)
        || native.retired != INSTRUCTION_BUDGET
        || interpreter.retired != INSTRUCTION_BUDGET
        || native.remaining_budget != 0
        || interpreter.remaining_budget != 0
        || native.state.rip != end
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

struct CodeMemory;

impl GuestMemory for CodeMemory {
    fn fetch(&mut self, address: u64, _context: AccessContext) -> Result<u8, MemoryError> {
        let offset = address
            .checked_sub(BASE_RIP)
            .and_then(|n| usize::try_from(n).ok());
        offset
            .and_then(|index| CODE.get(index).copied())
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

fn compile_once(register_homes: u8) -> Result<CompiledBlock, CostError> {
    let view =
        CodeView::new(BASE_RIP, &CODE).map_err(|error| fail(format!("code view: {error:?}")))?;
    let outcome = compile_block_with_register_homes(view, BASE_RIP, register_homes)
        .map_err(|error| fail(format!("compile: {error:?}")))?;
    match outcome {
        BlockCompileOutcome::Compiled(compiled) => Ok(compiled),
        BlockCompileOutcome::NoNativePrefix { .. } => {
            Err(fail("compile produced no native prefix"))
        }
    }
}

fn measure_compile_pairs(process_run: u8) -> Result<(usize, Vec<CompilePairSample>), CostError> {
    let mut calls = 256usize;
    loop {
        let first = compile_batch(HOME_BUDGET, calls)?;
        let second = compile_batch(SPILL_ONLY_BUDGET, calls)?;
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
            let zero = compile_batch(SPILL_ONLY_BUDGET, calls)?.0;
            let four = compile_batch(HOME_BUDGET, calls)?.0;
            (four, zero)
        } else {
            let four = compile_batch(HOME_BUDGET, calls)?.0;
            let zero = compile_batch(SPILL_ONLY_BUDGET, calls)?.0;
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

fn compile_batch(register_homes: u8, calls: usize) -> Result<(Duration, usize), CostError> {
    let started = Instant::now();
    let mut last_bytes = 0usize;
    for _ in 0..calls {
        let compiled = compile_once(register_homes)?;
        last_bytes = usize::try_from(compiled.emitted().pc_map().code_len())
            .map_err(|_| fail("emitted byte count exceeds host size"))?;
        std::hint::black_box(last_bytes);
    }
    Ok((started.elapsed(), last_bytes))
}

fn calibrate_common_repetitions(
    cases: &[Case],
    interpreters: &[Observation],
    budgets: &[u8],
) -> Result<(usize, Vec<(u8, String, u128, u128)>), CostError> {
    let mut common = CHUNK_CALLS;
    let mut report = Vec::new();
    for budget in budgets {
        let (compiled, _) = compile_variant(*budget).map_err(|e| fail(e.to_string()))?;
        let published = PublishedCode::publish_block(compiled.emitted())
            .map_err(|e| fail(format!("calibration publish: {e:?}")))?;
        for (index, case) in cases.iter().enumerate() {
            let mut calls = CHUNK_CALLS;
            loop {
                let native = time_native_batch(&published, case, &interpreters[index], calls)?;
                let interpreter = time_interpreter_batch(case, &interpreters[index], calls)?;
                report.push((
                    *budget,
                    case.id.into(),
                    native.as_nanos(),
                    interpreter.as_nanos(),
                ));
                if native >= MIN_SAMPLE && interpreter >= MIN_SAMPLE {
                    common = common.max(calls);
                    break;
                }
                calls = calls
                    .checked_mul(2)
                    .filter(|n| *n <= MAX_CALIBRATED_CALLS)
                    .ok_or_else(|| fail("execution calibration exceeded safety bound"))?;
            }
        }
        drop(published);
    }
    Ok((common, report))
}

fn time_native_batch(
    published: &PublishedCode,
    case: &Case,
    oracle: &Observation,
    calls: usize,
) -> Result<Duration, CostError> {
    let mut elapsed = Duration::ZERO;
    let mut remaining = calls;
    while remaining > 0 {
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
    }
    Ok(elapsed)
}

fn time_interpreter_batch(
    case: &Case,
    oracle: &Observation,
    calls: usize,
) -> Result<Duration, CostError> {
    let mut elapsed = Duration::ZERO;
    let mut remaining = calls;
    while remaining > 0 {
        let count = remaining.min(CHUNK_CALLS);
        let mut states = vec![case.initial.clone(); count];
        let mut results = Vec::with_capacity(count);
        let mut memory = CodeMemory;
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
    }
    Ok(elapsed)
}
