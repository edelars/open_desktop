//! Single-block eager versus uncached demand-flags candidate comparison.
//!
//! This intentionally measures repeated execution of one retained published
//! block at a time. It does not include dispatcher lookup or shared-cache cost.

use std::hint::black_box;
use std::sync::atomic::AtomicU8;
use std::time::{Duration, Instant};

use od_host::PublishedCode;
use od_isa::LongModeState;
use od_isa::execute::{RunConfig, RunExit, run as interpreter_run};
use od_isa::memory::AccessContext;
use od_jit::block::CodeView;
use od_jit::block_compile::{
    BlockCompileOutcome, CompiledBlock, compile_block, compile_block_demand_flags_candidate,
};
use od_jit::runtime::RuntimeExitTag;
use serde::Serialize;

use crate::dispatch_cache::FetchMemory;
use crate::{HostMetadata, MAX_RUN_TIME};

const PROFILE_ID: &str = "s3-29f-s0-09-single-block-flags-v1";
const BASE_RIP: u64 = 0;
const MIN_SAMPLE: Duration = Duration::from_millis(200);
const SAMPLE_PAIRS: usize = 20;
const CALIBRATION_EXECUTIONS: u64 = 10_000;
const MAX_EXECUTIONS_PER_SAMPLE: u64 = 10_000_000;
const MAX_SAMPLE_RESTARTS: usize = 8;
const MAX_CODE_BYTES_PER_CASE: usize = 4 * 1024;

#[derive(Debug)]
struct BenchError(String);

impl std::fmt::Display for BenchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BenchError {}

fn err(message: impl Into<String>) -> BenchError {
    BenchError(message.into())
}

#[derive(Clone)]
struct CaseSpec {
    id: &'static str,
    bytes: Vec<u8>,
    initial: LongModeState,
    instruction_budget: u64,
    instruction_count: u32,
    expected_exit: ExpectedNativeExit,
    expected_branch_taken: Option<bool>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum ExpectedNativeExit {
    BlockComplete,
    BudgetExhausted,
}

#[derive(Serialize)]
pub struct FlagsCompareReport {
    schema: u32,
    profile_id: &'static str,
    profile_sha256: String,
    process_run: u8,
    measure_host: HostMetadata,
    timed_scope: &'static str,
    excluded_from_timing: &'static str,
    sample_pairs: usize,
    minimum_sample_ns: u128,
    mapping_policy: &'static str,
    comparison_limit: &'static str,
    cases: Vec<CaseReport>,
    total_elapsed_ns: u128,
}

#[derive(Serialize)]
struct CaseReport {
    id: &'static str,
    code_hex: String,
    code_sha256: String,
    base_rip: u64,
    initial_gpr: [u64; 16],
    initial_rflags: u64,
    instruction_budget: u64,
    expected_guest_instruction_count: u32,
    expected_native_exit: ExpectedNativeExit,
    expected_branch_taken: Option<bool>,
    interpreter_exit: String,
    interpreter_state: StateSnapshot,
    interpreter_retired: u64,
    interpreter_remaining_budget: u64,
    compile: CompileReport,
    calibration: CalibrationReport,
    measured: MeasuredCase,
}

#[derive(Clone, Serialize)]
struct StateSnapshot {
    gpr: [u64; 16],
    rip: u64,
    rflags: u64,
}

impl From<&LongModeState> for StateSnapshot {
    fn from(state: &LongModeState) -> Self {
        Self {
            gpr: state.gpr,
            rip: state.rip,
            rflags: state.rflags,
        }
    }
}

#[derive(Serialize)]
struct CompileReport {
    eager_elapsed_ns: u128,
    candidate_elapsed_ns: u128,
    eager_code_bytes: usize,
    candidate_code_bytes: usize,
    eager_code_sha256: String,
    candidate_code_sha256: String,
    published_block_guest_instruction_count: u32,
    published_block_code_end: u64,
}

#[derive(Serialize)]
struct CalibrationReport {
    calibration_executions_per_variant: u64,
    eager_elapsed_ns: u128,
    candidate_elapsed_ns: u128,
    estimated_executions_for_minimum_sample: u64,
    measured_executions_per_variant: u64,
}

#[derive(Serialize)]
struct MeasuredCase {
    restarts_for_short_samples: usize,
    eager: VariantSummary,
    candidate: VariantSummary,
    candidate_over_eager_ratios: Vec<f64>,
}

#[derive(Serialize)]
struct VariantSummary {
    samples: Vec<Sample>,
    median_ns_per_execution: u128,
    min_ns_per_execution: u128,
    max_ns_per_execution: u128,
    iqr_ns_per_execution: u128,
}

#[derive(Clone, Serialize)]
struct Sample {
    pair: usize,
    order: &'static str,
    executions: u64,
    elapsed_ns: u128,
    publish_elapsed_ns: u128,
    last_exit: String,
    last_state: StateSnapshot,
    last_retired: u64,
    last_remaining_budget: u64,
}

struct CompiledVariant {
    artifact: CompiledBlock,
    compile_elapsed_ns: u128,
}

struct ExecutionObservation {
    exit: RuntimeExitTag,
    state: LongModeState,
    retired: u64,
    remaining_budget: u64,
}

struct TimedVariant {
    elapsed_ns: u128,
    publish_elapsed_ns: u128,
    last: ExecutionObservation,
}

pub fn run(process_run: u8) -> Result<FlagsCompareReport, Box<dyn std::error::Error>> {
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    return Err("flags-compare requires native Apple Silicon macOS execution".into());

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    run_native(process_run).map_err(|error| Box::new(error) as Box<dyn std::error::Error>)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn run_native(process_run: u8) -> Result<FlagsCompareReport, BenchError> {
    if !(1..=3).contains(&process_run) {
        return Err(err("process-run must be in 1..=3"));
    }
    let started = Instant::now();
    let cases = fixed_profile();
    let profile_sha256 = profile_hash(&cases);
    let mut output = Vec::with_capacity(cases.len());
    for case in cases {
        if started.elapsed() > MAX_RUN_TIME {
            return Err(err("30 minute run limit exceeded"));
        }
        output.push(measure_case(&case, started)?);
    }
    Ok(FlagsCompareReport {
        schema: 1,
        profile_id: PROFILE_ID,
        profile_sha256,
        process_run,
        measure_host: HostMetadata::current(),
        timed_scope: "summed repeated PublishedCode::execute_block call durations for one published eager or uncached candidate block, including the host safe wrapper, runtime state materialization/snapshot, and arithmetic-flags helper work; guest state reset before each individually timed call",
        excluded_from_timing: "fixed-profile setup, code decode/compilation, publication, state reset/clone, outcome validation, result serialization, and mapping drop",
        sample_pairs: SAMPLE_PAIRS,
        minimum_sample_ns: MIN_SAMPLE.as_nanos(),
        mapping_policy: "one MAP_JIT PublishedCode owner at a time; each sample publishes one already compiled artifact, executes it repeatedly, then drops it before the paired variant is published",
        comparison_limit: "single-block flag-strategy comparison only; no dispatcher lookup, shared-cache path, interpreter fallback, checked-memory helper, guest-memory workload, physical x86, or Mini coverage",
        cases: output,
        total_elapsed_ns: started.elapsed().as_nanos(),
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_case(case: &CaseSpec, run_started: Instant) -> Result<CaseReport, BenchError> {
    let code_view = CodeView::new(BASE_RIP, &case.bytes)
        .map_err(|error| err(format!("{} code view failed: {error:?}", case.id)))?;
    let (eager, eager_code_bytes) = compile_variant(case, false, code_view)?;
    let (candidate, candidate_code_bytes) = compile_variant(case, true, code_view)?;
    if eager.artifact.code_end() != BASE_RIP + case.bytes.len() as u64
        || candidate.artifact.code_end() != BASE_RIP + case.bytes.len() as u64
        || eager.artifact.emitted().pc_map().instruction_count() != case.instruction_count
        || candidate.artifact.emitted().pc_map().instruction_count() != case.instruction_count
    {
        return Err(err(format!(
            "{} did not compile the complete fixed guest block",
            case.id
        )));
    }
    if eager_code_bytes.len() > MAX_CODE_BYTES_PER_CASE
        || candidate_code_bytes.len() > MAX_CODE_BYTES_PER_CASE
    {
        return Err(err(format!("{} emitted block exceeds 4 KiB", case.id)));
    }

    let code_end = BASE_RIP + case.bytes.len() as u64;
    let (interpreter_state, interpreter_exit, interpreter_retired, interpreter_remaining) =
        execute_interpreter(case, code_end)?;
    let expected_retired = match case.expected_exit {
        ExpectedNativeExit::BlockComplete => u64::from(case.instruction_count),
        ExpectedNativeExit::BudgetExhausted => case.instruction_budget,
    };
    if interpreter_retired != expected_retired {
        return Err(err(format!(
            "{} interpreter retired {interpreter_retired}, expected {expected_retired}",
            case.id
        )));
    }
    match case.expected_exit {
        ExpectedNativeExit::BlockComplete => {
            let (expected_rip, expected_exit) = if case.expected_branch_taken == Some(true) {
                (BASE_RIP, RunExit::InstructionBudgetExhausted)
            } else {
                (code_end, RunExit::StopAddress)
            };
            if interpreter_exit != expected_exit || interpreter_state.rip != expected_rip {
                return Err(err(format!(
                    "{} interpreter branch outcome mismatch: exit {interpreter_exit:?}, RIP {:#x}; expected {expected_exit:?}, RIP {expected_rip:#x}",
                    case.id, interpreter_state.rip
                )));
            }
        }
        ExpectedNativeExit::BudgetExhausted
            if interpreter_exit != RunExit::InstructionBudgetExhausted =>
        {
            return Err(err(format!(
                "{} interpreter exited {interpreter_exit:?}, expected budget exhaustion",
                case.id
            )));
        }
        _ => {}
    }

    let expected = ExecutionObservation {
        exit: match case.expected_exit {
            ExpectedNativeExit::BlockComplete => RuntimeExitTag::BlockComplete,
            ExpectedNativeExit::BudgetExhausted => RuntimeExitTag::BudgetExhausted,
        },
        state: interpreter_state.clone(),
        retired: interpreter_retired,
        remaining_budget: interpreter_remaining,
    };
    validate_variant(case, &eager.artifact, code_end, &expected)?;
    validate_variant(case, &candidate.artifact, code_end, &expected)?;

    let (mut calibration, mut executions) =
        calibrate(case, &eager, &candidate, code_end, run_started)?;
    let mut restarts = 0;
    let (eager_samples, candidate_samples) = loop {
        let mut eager_samples = Vec::with_capacity(SAMPLE_PAIRS);
        let mut candidate_samples = Vec::with_capacity(SAMPLE_PAIRS);
        let mut retry_with = None;
        for pair in 0..SAMPLE_PAIRS {
            if run_started.elapsed() > MAX_RUN_TIME {
                return Err(err("30 minute run limit exceeded during paired samples"));
            }
            let eager_first = pair % 2 == 0;
            let (first_name, first_variant, second_name, second_variant) = if eager_first {
                ("eager-first", &eager, "candidate-second", &candidate)
            } else {
                ("candidate-first", &candidate, "eager-second", &eager)
            };
            let first = measure_variant(case, first_variant, code_end, executions)?;
            let second = measure_variant(case, second_variant, code_end, executions)?;
            if first.elapsed_ns < MIN_SAMPLE.as_nanos() || second.elapsed_ns < MIN_SAMPLE.as_nanos()
            {
                let shortest = first.elapsed_ns.min(second.elapsed_ns);
                let adjusted = scale_executions(executions, MIN_SAMPLE.as_nanos(), shortest)?;
                retry_with = Some(adjusted.max(executions.saturating_add(1)));
                break;
            }
            let (eager_timed, candidate_timed, eager_order, candidate_order) = if eager_first {
                (first, second, first_name, second_name)
            } else {
                (second, first, second_name, first_name)
            };
            validate_observation(case, &eager_timed.last, &expected)?;
            validate_observation(case, &candidate_timed.last, &expected)?;
            eager_samples.push(sample(pair, eager_order, executions, eager_timed));
            candidate_samples.push(sample(pair, candidate_order, executions, candidate_timed));
        }
        if let Some(adjusted) = retry_with {
            restarts += 1;
            if restarts > MAX_SAMPLE_RESTARTS {
                return Err(err(format!(
                    "{} could not obtain calibrated >=200ms paired samples",
                    case.id
                )));
            }
            if adjusted > MAX_EXECUTIONS_PER_SAMPLE {
                return Err(err(format!(
                    "{} sample calibration exceeds the 10M execution limit",
                    case.id
                )));
            }
            executions = adjusted;
            continue;
        }
        break (eager_samples, candidate_samples);
    };
    calibration.measured_executions_per_variant = executions;

    let ratios = eager_samples
        .iter()
        .zip(&candidate_samples)
        .map(|(eager, candidate)| {
            (candidate.elapsed_ns as f64 / candidate.executions as f64)
                / (eager.elapsed_ns as f64 / eager.executions as f64)
        })
        .collect::<Vec<_>>();
    Ok(CaseReport {
        id: case.id,
        code_hex: hex(&case.bytes),
        code_sha256: od_testkit::sha256_hex(&case.bytes),
        base_rip: BASE_RIP,
        initial_gpr: case.initial.gpr,
        initial_rflags: case.initial.rflags,
        instruction_budget: case.instruction_budget,
        expected_guest_instruction_count: case.instruction_count,
        expected_native_exit: case.expected_exit,
        expected_branch_taken: case.expected_branch_taken,
        interpreter_exit: format!("{interpreter_exit:?}"),
        interpreter_state: StateSnapshot::from(&interpreter_state),
        interpreter_retired,
        interpreter_remaining_budget: interpreter_remaining,
        compile: CompileReport {
            eager_elapsed_ns: eager.compile_elapsed_ns,
            candidate_elapsed_ns: candidate.compile_elapsed_ns,
            eager_code_bytes: eager_code_bytes.len(),
            candidate_code_bytes: candidate_code_bytes.len(),
            eager_code_sha256: od_testkit::sha256_hex(&eager_code_bytes),
            candidate_code_sha256: od_testkit::sha256_hex(&candidate_code_bytes),
            published_block_guest_instruction_count: case.instruction_count,
            published_block_code_end: eager.artifact.code_end(),
        },
        calibration,
        measured: MeasuredCase {
            restarts_for_short_samples: restarts,
            eager: summarize(eager_samples),
            candidate: summarize(candidate_samples),
            candidate_over_eager_ratios: ratios,
        },
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn compile_variant(
    case: &CaseSpec,
    demand_flags: bool,
    code_view: CodeView<'_>,
) -> Result<(CompiledVariant, Vec<u8>), BenchError> {
    let started = Instant::now();
    let outcome = if demand_flags {
        compile_block_demand_flags_candidate(code_view, BASE_RIP)
    } else {
        compile_block(code_view, BASE_RIP)
    }
    .map_err(|error| err(format!("{} compilation failed: {error:?}", case.id)))?;
    let elapsed_ns = started.elapsed().as_nanos();
    let artifact = match outcome {
        BlockCompileOutcome::Compiled(block) => block,
        BlockCompileOutcome::NoNativePrefix { reason, .. } => {
            return Err(err(format!(
                "{} produced NoNativePrefix: {reason:?}",
                case.id
            )));
        }
    };
    let bytes = artifact
        .emitted()
        .code()
        .to_le_bytes()
        .map_err(|error| err(format!("{} code extraction failed: {error:?}", case.id)))?;
    Ok((
        CompiledVariant {
            artifact,
            compile_elapsed_ns: elapsed_ns,
        },
        bytes,
    ))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn execute_interpreter(
    case: &CaseSpec,
    code_end: u64,
) -> Result<(LongModeState, RunExit, u64, u64), BenchError> {
    let mut state = case.initial.clone();
    let mut memory = FetchMemory(&case.bytes);
    let result = interpreter_run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: case.instruction_budget,
            stop_rip: Some(code_end),
            context: AccessContext::default(),
        },
        || false,
    );
    Ok((
        state,
        result.exit,
        result.instructions_executed,
        case.instruction_budget
            .saturating_sub(result.instructions_executed),
    ))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn calibrate(
    case: &CaseSpec,
    eager: &CompiledVariant,
    candidate: &CompiledVariant,
    code_end: u64,
    run_started: Instant,
) -> Result<(CalibrationReport, u64), BenchError> {
    let eager_timed = measure_variant(case, eager, code_end, CALIBRATION_EXECUTIONS)?;
    let candidate_timed = measure_variant(case, candidate, code_end, CALIBRATION_EXECUTIONS)?;
    let fastest_elapsed = eager_timed.elapsed_ns.min(candidate_timed.elapsed_ns);
    let fastest_per_execution = fastest_elapsed / u128::from(CALIBRATION_EXECUTIONS);
    if fastest_per_execution == 0 {
        return Err(err(format!(
            "{} calibration timer resolution is insufficient",
            case.id
        )));
    }
    let executions = scale_executions(
        CALIBRATION_EXECUTIONS,
        MIN_SAMPLE.as_nanos(),
        fastest_elapsed,
    )?;
    if executions > MAX_EXECUTIONS_PER_SAMPLE {
        return Err(err(format!(
            "{} sample calibration exceeds the 10M execution limit",
            case.id
        )));
    }
    if run_started.elapsed() > MAX_RUN_TIME {
        return Err(err("30 minute run limit exceeded during calibration"));
    }
    Ok((
        CalibrationReport {
            calibration_executions_per_variant: CALIBRATION_EXECUTIONS,
            eager_elapsed_ns: eager_timed.elapsed_ns,
            candidate_elapsed_ns: candidate_timed.elapsed_ns,
            estimated_executions_for_minimum_sample: executions,
            measured_executions_per_variant: executions,
        },
        executions,
    ))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_variant(
    case: &CaseSpec,
    variant: &CompiledVariant,
    code_end: u64,
    executions: u64,
) -> Result<TimedVariant, BenchError> {
    let publish_started = Instant::now();
    let published = PublishedCode::publish_block(variant.artifact.emitted())
        .map_err(|error| err(format!("{} publication failed: {error:?}", case.id)))?;
    let publish_elapsed_ns = publish_started.elapsed().as_nanos();
    let stop_requested = AtomicU8::new(0);
    let mut elapsed_ns = 0u128;
    let mut last = None;
    for _ in 0..executions {
        let mut state = case.initial.clone();
        let started = Instant::now();
        let result = published
            .execute_block(
                &mut state,
                case.instruction_budget,
                Some(code_end),
                &stop_requested,
            )
            .map_err(|error| err(format!("{} native execution failed: {error:?}", case.id)))?;
        elapsed_ns = elapsed_ns
            .checked_add(started.elapsed().as_nanos())
            .ok_or_else(|| err(format!("{} elapsed time overflowed", case.id)))?;
        black_box((&state, result));
        last = Some(ExecutionObservation {
            exit: result.exit,
            state,
            retired: result.retired_instructions,
            remaining_budget: result.remaining_budget,
        });
    }
    drop(published);
    Ok(TimedVariant {
        elapsed_ns,
        publish_elapsed_ns,
        last: last.ok_or_else(|| err("measured sample had zero executions"))?,
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn validate_variant(
    case: &CaseSpec,
    variant: &CompiledBlock,
    code_end: u64,
    expected: &ExecutionObservation,
) -> Result<(), BenchError> {
    let compiled = CompiledVariant {
        artifact: variant.clone(),
        compile_elapsed_ns: 0,
    };
    let observed = measure_variant(case, &compiled, code_end, 1)?.last;
    validate_observation(case, &observed, expected)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn validate_observation(
    case: &CaseSpec,
    observed: &ExecutionObservation,
    expected: &ExecutionObservation,
) -> Result<(), BenchError> {
    if observed.exit != expected.exit
        || observed.state != expected.state
        || observed.retired != expected.retired
        || observed.remaining_budget != expected.remaining_budget
    {
        return Err(err(format!(
            "{} native/interpreter mismatch: exit {:?}/{:?}, state_equal={}, retired {}/{}, remaining {}/{}",
            case.id,
            observed.exit,
            expected.exit,
            observed.state == expected.state,
            observed.retired,
            expected.retired,
            observed.remaining_budget,
            expected.remaining_budget
        )));
    }
    Ok(())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn sample(pair: usize, order: &'static str, executions: u64, timed: TimedVariant) -> Sample {
    Sample {
        pair,
        order,
        executions,
        elapsed_ns: timed.elapsed_ns,
        publish_elapsed_ns: timed.publish_elapsed_ns,
        last_exit: format!("{:?}", timed.last.exit),
        last_state: StateSnapshot::from(&timed.last.state),
        last_retired: timed.last.retired,
        last_remaining_budget: timed.last.remaining_budget,
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn summarize(mut samples: Vec<Sample>) -> VariantSummary {
    let mut per_execution = samples
        .iter()
        .map(|sample| sample.elapsed_ns / u128::from(sample.executions))
        .collect::<Vec<_>>();
    per_execution.sort_unstable();
    let median = if per_execution.len() % 2 == 0 {
        (per_execution[per_execution.len() / 2 - 1] + per_execution[per_execution.len() / 2]) / 2
    } else {
        per_execution[per_execution.len() / 2]
    };
    let iqr = per_execution[per_execution.len() * 3 / 4]
        .saturating_sub(per_execution[per_execution.len() / 4]);
    samples.sort_by_key(|sample| sample.pair);
    VariantSummary {
        samples,
        median_ns_per_execution: median,
        min_ns_per_execution: per_execution[0],
        max_ns_per_execution: *per_execution.last().unwrap(),
        iqr_ns_per_execution: iqr,
    }
}

fn scale_executions(current: u64, target_ns: u128, observed_ns: u128) -> Result<u64, BenchError> {
    if observed_ns == 0 {
        return Err(err("cannot calibrate from a zero-duration sample"));
    }
    let scaled = u128::from(current)
        .checked_mul(target_ns)
        .and_then(|value| value.checked_add(observed_ns - 1))
        .ok_or_else(|| err("calibrated execution count overflowed"))?
        / observed_ns;
    u64::try_from(scaled.max(u128::from(current)))
        .map_err(|_| err("calibrated execution count exceeds platform limits"))
}

fn fixed_profile() -> Vec<CaseSpec> {
    let mut overwritten = LongModeState::default();
    overwritten.gpr[0] = 3;
    overwritten.gpr[3] = 9;
    overwritten.gpr[1] = 7;
    overwritten.gpr[2] = 7;

    let mut live_at_budget = LongModeState::default();
    live_at_budget.gpr[0] = u64::MAX;
    live_at_budget.gpr[3] = 1;
    live_at_budget.gpr[1] = 11;
    live_at_budget.gpr[2] = 5;

    let mut alias = LongModeState::default();
    alias.gpr[0] = 0;

    let mut jz_taken = LongModeState::default();
    jz_taken.gpr[0] = 41;
    jz_taken.gpr[3] = 41;

    let mut jnz_taken = LongModeState::default();
    jnz_taken.gpr[0] = 41;
    jnz_taken.gpr[3] = 42;

    vec![
        CaseSpec {
            id: "overwritten-add-sub-live-jz",
            bytes: vec![0x48, 0x01, 0xd8, 0x48, 0x29, 0xd1, 0x74, 0xf8],
            initial: overwritten,
            instruction_budget: 3,
            instruction_count: 3,
            expected_exit: ExpectedNativeExit::BlockComplete,
            expected_branch_taken: Some(true),
        },
        CaseSpec {
            id: "budget-exit-between-add-and-sub",
            bytes: vec![0x48, 0x01, 0xd8, 0x48, 0x29, 0xd1, 0x74, 0xf8],
            initial: live_at_budget,
            instruction_budget: 1,
            instruction_count: 3,
            expected_exit: ExpectedNativeExit::BudgetExhausted,
            expected_branch_taken: None,
        },
        CaseSpec {
            id: "alias-add-live-jz",
            bytes: vec![0x48, 0x01, 0xc0, 0x74, 0xfb],
            initial: alias,
            instruction_budget: 2,
            instruction_count: 2,
            expected_exit: ExpectedNativeExit::BlockComplete,
            expected_branch_taken: Some(true),
        },
        CaseSpec {
            id: "cmp-live-jz-taken",
            bytes: vec![0x48, 0x39, 0xd8, 0x74, 0xfb],
            initial: jz_taken.clone(),
            instruction_budget: 2,
            instruction_count: 2,
            expected_exit: ExpectedNativeExit::BlockComplete,
            expected_branch_taken: Some(true),
        },
        CaseSpec {
            id: "cmp-live-jnz-taken",
            bytes: vec![0x48, 0x39, 0xd8, 0x75, 0xfb],
            initial: jnz_taken.clone(),
            instruction_budget: 2,
            instruction_count: 2,
            expected_exit: ExpectedNativeExit::BlockComplete,
            expected_branch_taken: Some(true),
        },
        CaseSpec {
            id: "cmp-live-jz-not-taken",
            bytes: vec![0x48, 0x39, 0xd8, 0x74, 0xfb],
            initial: jnz_taken,
            instruction_budget: 2,
            instruction_count: 2,
            expected_exit: ExpectedNativeExit::BlockComplete,
            expected_branch_taken: Some(false),
        },
        CaseSpec {
            id: "cmp-live-jnz-not-taken",
            bytes: vec![0x48, 0x39, 0xd8, 0x75, 0xfb],
            initial: jz_taken,
            instruction_budget: 2,
            instruction_count: 2,
            expected_exit: ExpectedNativeExit::BlockComplete,
            expected_branch_taken: Some(false),
        },
    ]
}

fn profile_hash(cases: &[CaseSpec]) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(PROFILE_ID.as_bytes());
    for case in cases {
        bytes.extend_from_slice(case.id.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&BASE_RIP.to_le_bytes());
        bytes.extend_from_slice(&(case.bytes.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&case.bytes);
        bytes.extend_from_slice(&case.instruction_budget.to_le_bytes());
        bytes.extend_from_slice(&case.instruction_count.to_le_bytes());
        bytes.push(match case.expected_exit {
            ExpectedNativeExit::BlockComplete => 0,
            ExpectedNativeExit::BudgetExhausted => 1,
        });
        bytes.push(match case.expected_branch_taken {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        });
        bytes.extend_from_slice(&case.initial.rip.to_le_bytes());
        for register in case.initial.gpr {
            bytes.extend_from_slice(&register.to_le_bytes());
        }
        bytes.extend_from_slice(&case.initial.rflags.to_le_bytes());
    }
    od_testkit::sha256_hex(&bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
