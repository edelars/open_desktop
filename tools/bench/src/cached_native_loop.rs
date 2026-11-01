//! Bounded S3-36 warm NativeRegisterV1 register recurrence profile.
//!
//! This synthetic helper-free workload compares the complete cached dispatcher
//! call with the interpreter. It does not change runtime policies or imply a
//! general guest speedup.

use std::num::NonZeroU64;
use std::sync::atomic::AtomicU8;
use std::time::{Duration, Instant};

use od_isa::LongModeState;
use od_isa::execute::{RunConfig, RunExit, run as interpreter_run};
use od_isa::memory::AccessContext;
use od_jit::block::{CodeView, NativeRegisterV1Admission};
use od_jit::dispatch::{
    DispatchControl, DispatchExit, FallbackDispatchResult, MAX_SINGLE_ENTRY_CACHE_BYTES,
    SingleEntryBlockCache,
};
use od_jit::runtime::RuntimeExitTag;
use od_jit::translation::{
    AddressSpaceId, BackingId, BackingVersion, CodeDependencySpan, CodeIdentity,
    ExecutionContextId, ExecutionContextSnapshot, MappingId, MappingVersion,
};
use serde::Serialize;

use crate::{HostMetadata, MAX_RUN_TIME};

const PROFILE_ID: &str = "s3-36f-native-register-v1-cached-register-loop-v1";
pub(crate) const BASE_RIP: u64 = 0x1000;
const PAIRS: usize = 20;
const CALIBRATION_CALLS: u64 = 64;
const MIN_SAMPLE: Duration = Duration::from_millis(200);
const MAX_CALLS: u64 = 1 << 28;
const WARMUP_CALLS: u64 = 32;
const SOURCE: &[u8] = include_bytes!("cached_native_loop.rs");
const CLI_SOURCE: &[u8] = include_bytes!("main.rs");
const LIB_SOURCE: &[u8] = include_bytes!("lib.rs");
const DISPATCH_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/dispatch.rs");
const COMPILER_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/block_compile.rs");
const INTERPRETER_SOURCE: &[u8] = include_bytes!("../../../crates/od-isa/src/execute.rs");

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

#[derive(Clone, Copy)]
pub(crate) struct Stratum {
    pub(crate) id: &'static str,
    pub(crate) code: &'static [u8],
    pub(crate) instructions_per_iteration: u64,
    pub(crate) iterations: u64,
    pub(crate) seed: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct StateSnapshot {
    gpr: [u64; 16],
    rip: u64,
    rflags: u64,
}
impl From<&LongModeState> for StateSnapshot {
    fn from(s: &LongModeState) -> Self {
        Self {
            gpr: s.gpr,
            rip: s.rip,
            rflags: s.rflags,
        }
    }
}

#[derive(Serialize)]
pub struct PreflightReport {
    schema: u32,
    profile_id: &'static str,
    measure_host: HostMetadata,
    source_fingerprints: Vec<(&'static str, String)>,
    timing_started: bool,
    strata: Vec<StratumPreflight>,
}
#[derive(Serialize)]
struct StratumPreflight {
    id: &'static str,
    code_hex: String,
    code_sha256: String,
    resident_entries: usize,
    resident_code_bytes: usize,
    resident_logical_bytes: usize,
    seed: u64,
    iterations: u64,
    budget: u64,
    zero_work_checked: bool,
    boundary_checked: bool,
    mid_loop_budget_checked: bool,
    mid_loop_stop_checked: bool,
    recurrence_checked: bool,
    interpreter: Observation,
    cached_dispatch: Observation,
    mid_loop_budget: Observation,
    mid_loop_stop: Observation,
    final_cache_counts: CacheCounts,
}
#[derive(Serialize)]
pub struct Report {
    schema: u32,
    profile_id: &'static str,
    process_run: u8,
    measure_host: HostMetadata,
    source_fingerprints: Vec<(&'static str, String)>,
    timed_scope: &'static str,
    excluded_from_timing: &'static str,
    sampling_protocol: &'static str,
    strata: Vec<StratumReport>,
    total_elapsed_ns: u128,
}
#[derive(Serialize)]
struct StratumReport {
    id: &'static str,
    code_hex: String,
    code_sha256: String,
    seed: u64,
    iterations: u64,
    guest_work_per_call: u64,
    resident_entries: usize,
    resident_code_bytes: usize,
    resident_logical_bytes: usize,
    cold_first_dispatch_ns: u128,
    calibrated_calls_per_side: u64,
    native_warmup_calls_per_side: u64,
    interpreter_warmup_calls_per_side: u64,
    calibration: PairSample,
    restarts: Vec<Restart>,
    interpreter_samples: Vec<Sample>,
    native_samples: Vec<Sample>,
    native_over_interpreter_ratios: Vec<f64>,
}
#[derive(Clone, Serialize)]
struct Observation {
    state: StateSnapshot,
    exit: String,
    retired: u64,
    remaining_budget: u64,
}
#[derive(Clone, Copy, Debug, Default, Serialize)]
struct CacheCounts {
    hits: u64,
    misses: u64,
    compiles: u64,
    publications: u64,
    evictions: u64,
}
#[derive(Clone, Serialize)]
struct Sample {
    pair: usize,
    order: &'static str,
    seed: u64,
    calls: u64,
    elapsed_ns: u128,
    guest_instructions: u64,
    retired: u64,
    cache: CacheCounts,
}
#[derive(Clone, Serialize)]
struct PairSample {
    interpreter: Sample,
    native: Sample,
    next_calls: u64,
    interpreter_floor_met: bool,
    native_floor_met: bool,
}
#[derive(Serialize)]
struct Restart {
    attempt: usize,
    previous_calls: u64,
    next_calls: u64,
    discarded_interpreter: Vec<Sample>,
    discarded_native: Vec<Sample>,
}

pub fn preflight() -> Result<PreflightReport, Box<dyn std::error::Error>> {
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        Err("cached-native-loop requires native Apple Silicon macOS execution".into())
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    preflight_native().map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}
pub fn run(process_run: u8) -> Result<Report, Box<dyn std::error::Error>> {
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = process_run;
        Err("cached-native-loop requires native Apple Silicon macOS execution".into())
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    run_native(process_run).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn preflight_native() -> Result<PreflightReport, BenchError> {
    if cfg!(debug_assertions) {
        return Err(err("cached-native-loop preflight requires a release build"));
    }
    let mut reports = Vec::new();
    for spec in strata() {
        let case = prepare(spec)?;
        reports.push(check_case(&case)?);
    }
    Ok(PreflightReport {
        schema: 1,
        profile_id: PROFILE_ID,
        measure_host: HostMetadata::current(),
        source_fingerprints: fingerprints(),
        timing_started: false,
        strata: reports,
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn run_native(process_run: u8) -> Result<Report, BenchError> {
    if !(1..=3).contains(&process_run) {
        return Err(err("process-run must be in 1..=3"));
    }
    if cfg!(debug_assertions) {
        return Err(err("cached-native-loop requires a release build"));
    }
    let started = Instant::now();
    let mut reports = Vec::new();
    for spec in strata() {
        if started.elapsed() > MAX_RUN_TIME {
            return Err(err("30 minute benchmark run limit exceeded"));
        }
        let case = prepare(spec)?;
        check_case(&case)?;
        reports.push(measure_stratum(&case, started)?);
    }
    Ok(Report {
        schema: 1,
        profile_id: PROFILE_ID,
        process_run,
        measure_host: HostMetadata::current(),
        source_fingerprints: fingerprints(),
        timed_scope: "full cached NativeRegisterV1 dispatcher invocation, including validation, capacity-one cache lookup and published native block execution; interpreter is measured by full run call",
        excluded_from_timing: "profile setup/admission, cache allocation, state construction/reset, fixed native warmup, interpreter calibration, result/counter validation and serialization; cold_first_dispatch_ns separately covers the first full dispatch call including compile, publication and execution",
        sampling_protocol: "20 paired samples per stratum; cached and interpreter order alternates; calibrated common calls per side; each individual side must exceed 200 ms; interpreter calibration is its first workload and is recorded separately from 32 native warmup calls; three separate release processes are required",
        strata: reports,
        total_elapsed_ns: started.elapsed().as_nanos(),
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn check_case(case: &Prepared) -> Result<StratumPreflight, BenchError> {
    use od_host::PublishedCode;
    let s = case.stratum;
    let budget = s.iterations * s.instructions_per_iteration;
    let mut cache = new_cache::<PublishedCode>()?;
    let mut state = case.initial.clone();
    let zero = execute_native(case, &mut state, 0, &mut cache)?;
    if zero.exit != DispatchExit::BudgetExhausted
        || zero.remaining_budget != 0
        || zero.retired_native + zero.retired_interpreter != 0
        || state != case.initial
    {
        return Err(err(format!("{} zero-work native boundary failed", s.id)));
    }
    let mut zero_interpreter_state = case.initial.clone();
    let mut zero_memory = crate::dispatch_cache::FetchMemory(&case.fetch_bytes);
    let zero_interpreter = interpreter_run(
        &mut zero_interpreter_state,
        &mut zero_memory,
        RunConfig {
            instruction_budget: 0,
            stop_rip: None,
            context: AccessContext::default(),
        },
        || false,
    );
    if zero_interpreter.exit != RunExit::InstructionBudgetExhausted
        || zero_interpreter.instructions_executed != 0
        || zero_interpreter_state != case.initial
    {
        return Err(err(format!(
            "{} zero-work interpreter boundary failed",
            s.id
        )));
    }
    // The exact measured workload is first compiled/published outside timing.
    state.clone_from(&case.initial);
    let first = execute_native(case, &mut state, budget, &mut cache)?;
    validate_observation(case, &state, first, budget)?;
    let cold = cache_counts(&cache);
    if cold.hits != s.iterations.saturating_sub(1)
        || cold.misses != 1
        || cold.compiles != 1
        || cold.publications != 1
        || cold.evictions != 0
        || cache.resident_entries() != 1
        || cache.resident_code_bytes() == 0
        || cache.resident_logical_bytes() == 0
        || cache.resident_code_bytes() > MAX_SINGLE_ENTRY_CACHE_BYTES
        || cache.resident_logical_bytes() > MAX_SINGLE_ENTRY_CACHE_BYTES
    {
        return Err(err(format!(
            "{} failed single cold publication: {cold:?}",
            s.id
        )));
    }
    // Fixed warmup, checked as a hit-only set of full-work calls.
    let before = cache_counts(&cache);
    for _ in 0..WARMUP_CALLS {
        state.clone_from(&case.initial);
        let result = execute_native(case, &mut state, budget, &mut cache)?;
        validate_observation(case, &state, result, budget)?;
    }
    let after = cache_counts(&cache);
    validate_hit_delta(s.id, before, after, WARMUP_CALLS * s.iterations)?;
    let mut interp_state = case.initial.clone();
    let mut memory = crate::dispatch_cache::FetchMemory(&case.fetch_bytes);
    let interp = interpreter_run(
        &mut interp_state,
        &mut memory,
        RunConfig {
            instruction_budget: budget,
            stop_rip: None,
            context: AccessContext::default(),
        },
        || false,
    );
    let expected = recurrence(case.initial.clone(), s);
    if interp_state != expected
        || StateSnapshot::from(&interp_state) != case.oracle.state
        || interp.instructions_executed != case.oracle.retired
    {
        return Err(err(format!(
            "{} independent recurrence formula disagrees with interpreter oracle",
            s.id
        )));
    }
    let limited_budget = budget - 1;
    let mut limited_interpreter_state = case.initial.clone();
    let mut limited_memory = crate::dispatch_cache::FetchMemory(&case.fetch_bytes);
    let limited_interpreter = interpreter_run(
        &mut limited_interpreter_state,
        &mut limited_memory,
        RunConfig {
            instruction_budget: limited_budget,
            stop_rip: None,
            context: AccessContext::default(),
        },
        || false,
    );
    if limited_interpreter.exit != RunExit::InstructionBudgetExhausted
        || limited_interpreter.instructions_executed != limited_budget
        || limited_interpreter_state.rip != BASE_RIP + s.code.len() as u64 - 2
    {
        return Err(err(format!(
            "{} interpreter did not stop at the mid-loop budget boundary: {:?}, retired {}, rip {:#x}",
            s.id,
            limited_interpreter.exit,
            limited_interpreter.instructions_executed,
            limited_interpreter_state.rip
        )));
    }
    let before_limited = cache_counts(&cache);
    let mut limited_state = case.initial.clone();
    let limited_native = execute_native(case, &mut limited_state, limited_budget, &mut cache)?;
    validate_partial_result(
        case,
        &limited_state,
        limited_native,
        &limited_interpreter_state,
        limited_budget,
        0,
        DispatchExit::BudgetExhausted,
    )?;
    validate_control_hit_delta(s.id, before_limited, cache_counts(&cache))?;
    let mid_loop_budget = Observation {
        state: StateSnapshot::from(&limited_state),
        exit: format!("{:?}", limited_native.exit),
        retired: limited_native.retired_native + limited_native.retired_interpreter,
        remaining_budget: limited_native.remaining_budget,
    };

    let stop_rip = BASE_RIP + 4; // after LEA, before the XCHG in either form
    let mut stop_interpreter_state = case.initial.clone();
    let mut stop_memory = crate::dispatch_cache::FetchMemory(&case.fetch_bytes);
    let stop_interpreter = interpreter_run(
        &mut stop_interpreter_state,
        &mut stop_memory,
        RunConfig {
            instruction_budget: budget,
            stop_rip: Some(stop_rip),
            context: AccessContext::default(),
        },
        || false,
    );
    let before_stop = cache_counts(&cache);
    let mut stop_state = case.initial.clone();
    let stop_native =
        execute_native_with_stop(case, &mut stop_state, budget, &mut cache, Some(stop_rip))?;
    if stop_interpreter.exit != RunExit::StopAddress || stop_interpreter.instructions_executed != 1
    {
        return Err(err(format!(
            "{} interpreter did not stop at the internal instruction boundary",
            s.id
        )));
    }
    validate_partial_result(
        case,
        &stop_state,
        stop_native,
        &stop_interpreter_state,
        1,
        budget - 1,
        DispatchExit::Runtime(RuntimeExitTag::StopAddress),
    )?;
    validate_control_hit_delta(s.id, before_stop, cache_counts(&cache))?;
    let mid_loop_stop = Observation {
        state: StateSnapshot::from(&stop_state),
        exit: format!("{:?}", stop_native.exit),
        retired: stop_native.retired_native + stop_native.retired_interpreter,
        remaining_budget: stop_native.remaining_budget,
    };
    let native = Observation {
        state: StateSnapshot::from(&state),
        exit: "BudgetExhausted".into(),
        retired: budget,
        remaining_budget: 0,
    };
    let interpreter = Observation {
        state: StateSnapshot::from(&interp_state),
        exit: format!("{:?}", interp.exit),
        retired: interp.instructions_executed,
        remaining_budget: budget - interp.instructions_executed,
    };
    Ok(StratumPreflight {
        id: s.id,
        code_hex: hex(s.code),
        code_sha256: od_testkit::sha256_hex(s.code),
        resident_entries: cache.resident_entries(),
        resident_code_bytes: cache.resident_code_bytes(),
        resident_logical_bytes: cache.resident_logical_bytes(),
        seed: s.seed,
        iterations: s.iterations,
        budget,
        zero_work_checked: true,
        boundary_checked: true,
        mid_loop_budget_checked: true,
        mid_loop_stop_checked: true,
        recurrence_checked: true,
        interpreter,
        cached_dispatch: native,
        mid_loop_budget,
        mid_loop_stop,
        final_cache_counts: cache_counts(&cache),
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_stratum(case: &Prepared, run_started: Instant) -> Result<StratumReport, BenchError> {
    use od_host::PublishedCode;
    let mut cache = new_cache::<PublishedCode>()?;
    let mut state = case.initial.clone();
    let budget = case.stratum.iterations * case.stratum.instructions_per_iteration;
    let cold_started = Instant::now();
    let warm = execute_native(case, &mut state, budget, &mut cache)?;
    let cold_first_dispatch_ns = cold_started.elapsed().as_nanos();
    validate_observation(case, &state, warm, budget)?;
    for _ in 0..WARMUP_CALLS {
        state.clone_from(&case.initial);
        let result = execute_native(case, &mut state, budget, &mut cache)?;
        validate_observation(case, &state, result, budget)?;
    }
    let interpreter_probe =
        sample_interpreter(case, CALIBRATION_CALLS, 0, "calibration_interpreter_first")?;
    let native_probe = sample_native(
        case,
        &mut cache,
        CALIBRATION_CALLS,
        0,
        "calibration_native_second",
    )?;
    validate_sample_floor(&interpreter_probe, &native_probe)?;
    let calls = CALIBRATION_CALLS.max(calls_for(
        interpreter_probe.elapsed_ns,
        native_probe.elapsed_ns,
    )?);
    if calls > MAX_CALLS {
        return Err(err(format!(
            "{} calibrated calls exceed safety cap",
            case.stratum.id
        )));
    }
    let calibration = PairSample {
        interpreter: interpreter_probe.clone(),
        native: native_probe.clone(),
        next_calls: calls,
        interpreter_floor_met: interpreter_probe.elapsed_ns >= MIN_SAMPLE.as_nanos(),
        native_floor_met: native_probe.elapsed_ns >= MIN_SAMPLE.as_nanos(),
    };
    let mut restarts = Vec::new();
    let mut final_i = Vec::new();
    let mut final_n = Vec::new();
    let mut sample_calls = calls;
    for attempt in 0..6 {
        let mut is = Vec::with_capacity(PAIRS);
        let mut ns = Vec::with_capacity(PAIRS);
        let mut shortest = u128::MAX;
        for pair in 0..PAIRS {
            if run_started.elapsed() > MAX_RUN_TIME {
                return Err(err(
                    "30 minute benchmark run limit exceeded during sampling",
                ));
            }
            let interp_first = pair % 2 == 0;
            let (i, n) = if interp_first {
                (
                    sample_interpreter(case, sample_calls, pair, "interpreter_first")?,
                    sample_native(case, &mut cache, sample_calls, pair, "native_second")?,
                )
            } else {
                let n = sample_native(case, &mut cache, sample_calls, pair, "native_first")?;
                let i = sample_interpreter(case, sample_calls, pair, "interpreter_second")?;
                (i, n)
            };
            shortest = shortest.min(i.elapsed_ns).min(n.elapsed_ns);
            is.push(i);
            ns.push(n);
        }
        if shortest >= MIN_SAMPLE.as_nanos() {
            final_i = is;
            final_n = ns;
            break;
        }
        let next = calls_for_sample(
            sample_calls,
            is.iter().map(|x| x.elapsed_ns).min().unwrap_or(0),
            ns.iter().map(|x| x.elapsed_ns).min().unwrap_or(0),
        )?;
        if next <= sample_calls || next > MAX_CALLS {
            return Err(err(format!(
                "{} sample floor calibration could not safely increase repetitions",
                case.stratum.id
            )));
        }
        restarts.push(Restart {
            attempt,
            previous_calls: sample_calls,
            next_calls: next,
            discarded_interpreter: is,
            discarded_native: ns,
        });
        sample_calls = next;
    }
    if final_i.len() != PAIRS || final_n.len() != PAIRS {
        return Err(err(format!(
            "{} could not retain 20 pairs above the 200 ms floor",
            case.stratum.id
        )));
    }
    let ratios = final_i
        .iter()
        .zip(&final_n)
        .map(|(i, n)| n.elapsed_ns as f64 / i.elapsed_ns as f64)
        .collect();
    let c = cache_counts(&cache);
    if c.misses != 1 || c.compiles != 1 || c.publications != 1 || c.evictions != 0 {
        return Err(err(format!(
            "{} cache counters changed outside expected single prewarm: {c:?}",
            case.stratum.id
        )));
    }
    Ok(StratumReport {
        id: case.stratum.id,
        code_hex: hex(case.stratum.code),
        code_sha256: od_testkit::sha256_hex(case.stratum.code),
        seed: case.stratum.seed,
        iterations: case.stratum.iterations,
        guest_work_per_call: budget,
        resident_entries: cache.resident_entries(),
        resident_code_bytes: cache.resident_code_bytes(),
        resident_logical_bytes: cache.resident_logical_bytes(),
        cold_first_dispatch_ns,
        calibrated_calls_per_side: sample_calls,
        native_warmup_calls_per_side: WARMUP_CALLS,
        interpreter_warmup_calls_per_side: 0,
        calibration,
        restarts,
        interpreter_samples: final_i,
        native_samples: final_n,
        native_over_interpreter_ratios: ratios,
    })
}

pub(crate) fn strata() -> Vec<Stratum> {
    const A: &[u8] = &[0x48, 0x8D, 0x04, 0x0B, 0x48, 0x87, 0xD8, 0x75, 0xF7];
    const B: &[u8] = &[
        0x48, 0x8D, 0x04, 0x0B, 0x48, 0x87, 0xD8, 0x48, 0x8D, 0x04, 0x0B, 0x48, 0x87, 0xD8, 0x75,
        0xF0,
    ];
    let mut result = Vec::new();
    for (code, per_iter) in [(A, 3), (B, 5)] {
        for (iterations, seed) in [(1, 0x36_0001), (64, 0x36_0040), (4096, 0x36_1000)] {
            result.push(Stratum {
                id: if iterations == 1 {
                    if per_iter == 3 { "a_1" } else { "b_1" }
                } else if iterations == 64 {
                    if per_iter == 3 { "a_64" } else { "b_64" }
                } else if per_iter == 3 {
                    "a_4096"
                } else {
                    "b_4096"
                },
                code,
                instructions_per_iteration: per_iter,
                iterations,
                seed,
            });
        }
    }
    result
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
struct Prepared {
    stratum: Stratum,
    view_bytes: Vec<u8>,
    fetch_bytes: Vec<u8>,
    admission: NativeRegisterV1Admission,
    context: ExecutionContextSnapshot,
    dependencies: [CodeDependencySpan; 1],
    initial: LongModeState,
    oracle: Observation,
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn prepare(s: Stratum) -> Result<Prepared, BenchError> {
    let view =
        CodeView::new(BASE_RIP, s.code).map_err(|e| err(format!("{} code view: {e:?}", s.id)))?;
    let admission = NativeRegisterV1Admission::from_code(view)
        .map_err(|e| err(format!("{} admission: {e:?}", s.id)))?;
    let budget = s.iterations * s.instructions_per_iteration;
    let mut initial = initial_state(s.seed);
    let mut oracle_state = initial.clone();
    let fetch_bytes = padded_code(s.code);
    let mut memory = crate::dispatch_cache::FetchMemory(&fetch_bytes);
    let oracle_result = interpreter_run(
        &mut oracle_state,
        &mut memory,
        RunConfig {
            instruction_budget: budget,
            stop_rip: None,
            context: AccessContext::default(),
        },
        || false,
    );
    let expected_rip = BASE_RIP;
    if oracle_result.exit != RunExit::InstructionBudgetExhausted
        || oracle_result.instructions_executed != budget
        || oracle_state.rip != expected_rip
        || oracle_state.rflags & 0x40 != 0
    {
        return Err(err(format!(
            "{} interpreter recurrence oracle invalid: {:?}, retired {}, rip {:#x}, flags {:#x}",
            s.id,
            oracle_result.exit,
            oracle_result.instructions_executed,
            oracle_state.rip,
            oracle_state.rflags
        )));
    }
    initial.rflags &= !0x40; // initial ZF clear; both loop bodies preserve it.
    let context = ExecutionContextSnapshot::native_register_v1_fixture(
        ExecutionContextId::from_u64(1).expect("nonzero context"),
    );
    let dependencies = [CodeDependencySpan {
        start_rip: BASE_RIP,
        end_rip: view.code_end(),
        identity: CodeIdentity {
            address_space: AddressSpaceId::new(NonZeroU64::new(1).unwrap()),
            mapping: MappingId::new(NonZeroU64::new(1).unwrap()),
            mapping_version: MappingVersion::new(NonZeroU64::new(1).unwrap()),
            backing: BackingId::new(NonZeroU64::new(1).unwrap()),
            backing_version: BackingVersion::new(NonZeroU64::new(1).unwrap()),
            executable: true,
        },
    }];
    Ok(Prepared {
        stratum: s,
        view_bytes: s.code.to_vec(),
        fetch_bytes,
        admission,
        context,
        dependencies,
        initial,
        oracle: Observation {
            state: StateSnapshot::from(&oracle_state),
            exit: format!("{:?}", oracle_result.exit),
            retired: oracle_result.instructions_executed,
            remaining_budget: budget - oracle_result.instructions_executed,
        },
    })
}

pub(crate) fn initial_state(seed: u64) -> LongModeState {
    let mut s = LongModeState::default();
    s.rip = BASE_RIP;
    s.rflags = 0x202; // reserved bit plus ZF clear
    s.gpr[0] = seed ^ 0xa55a_1234_9876_0001;
    s.gpr[3] = seed.rotate_left(17) ^ 0x55aa_fedc_0123_4567;
    s.gpr[1] = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    s
}

pub(crate) fn padded_code(code: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0; BASE_RIP as usize];
    bytes.extend_from_slice(code);
    bytes
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn new_cache<P>() -> Result<SingleEntryBlockCache<P>, BenchError> {
    SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES)
        .map_err(|e| err(format!("cache setup: {e:?}")))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn execute_native(
    case: &Prepared,
    state: &mut LongModeState,
    budget: u64,
    cache: &mut SingleEntryBlockCache<od_host::PublishedCode>,
) -> Result<FallbackDispatchResult, BenchError> {
    let view = CodeView::new(BASE_RIP, &case.view_bytes)
        .map_err(|e| err(format!("{} code view: {e:?}", case.stratum.id)))?;
    let stop = AtomicU8::new(0);
    execute_native_view(case, view, state, budget, cache, &stop, None)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn execute_native_with_stop(
    case: &Prepared,
    state: &mut LongModeState,
    budget: u64,
    cache: &mut SingleEntryBlockCache<od_host::PublishedCode>,
    stop_rip: Option<u64>,
) -> Result<FallbackDispatchResult, BenchError> {
    let view = CodeView::new(BASE_RIP, &case.view_bytes)
        .map_err(|e| err(format!("{} code view: {e:?}", case.stratum.id)))?;
    let stop = AtomicU8::new(0);
    od_jit::dispatch::run_cached_native_register_v1_controlled(
        view,
        &case.admission,
        case.context,
        &case.dependencies,
        state,
        budget,
        DispatchControl {
            stop_rip,
            stop_requested: &stop,
            event_requested: None,
        },
        cache,
        &mut od_host::PublishedBlockExecutor,
    )
    .map_err(|e| {
        err(format!(
            "{} controlled cached dispatch: {e:?}",
            case.stratum.id
        ))
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn execute_native_view(
    case: &Prepared,
    view: CodeView<'_>,
    state: &mut LongModeState,
    budget: u64,
    cache: &mut SingleEntryBlockCache<od_host::PublishedCode>,
    stop: &AtomicU8,
    stop_rip: Option<u64>,
) -> Result<FallbackDispatchResult, BenchError> {
    od_jit::dispatch::run_cached_native_register_v1_controlled(
        view,
        &case.admission,
        case.context,
        &case.dependencies,
        state,
        budget,
        DispatchControl {
            stop_rip,
            stop_requested: stop,
            event_requested: None,
        },
        cache,
        &mut od_host::PublishedBlockExecutor,
    )
    .map_err(|e| err(format!("{} cached dispatch: {e:?}", case.stratum.id)))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn validate_observation(
    case: &Prepared,
    state: &LongModeState,
    result: FallbackDispatchResult,
    budget: u64,
) -> Result<(), BenchError> {
    let retired = result.retired_native + result.retired_interpreter;
    let expected = recurrence(case.initial.clone(), case.stratum);
    if result.exit != DispatchExit::BudgetExhausted
        || result.retired_interpreter != 0
        || retired != budget
        || result.remaining_budget != 0
        || result.blocks_executed != case.stratum.iterations
        || *state != expected
        || state.rip != BASE_RIP
        || state.rflags & 0x40 != 0
    {
        return Err(err(format!(
            "{} cached dispatch mismatch: exit={:?}, retired={retired}/{budget}, remaining={}, state_equal={}, rip={:#x}, flags={:#x}",
            case.stratum.id,
            result.exit,
            result.remaining_budget,
            *state == expected,
            state.rip,
            state.rflags
        )));
    }
    Ok(())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn validate_partial_result(
    case: &Prepared,
    state: &LongModeState,
    result: FallbackDispatchResult,
    expected_state: &LongModeState,
    expected_retired: u64,
    expected_remaining: u64,
    expected_exit: DispatchExit,
) -> Result<(), BenchError> {
    if result.exit != expected_exit
        || result.retired_native != expected_retired
        || result.retired_interpreter != 0
        || result.remaining_budget != expected_remaining
        || result.blocks_executed == 0
        || *state != *expected_state
    {
        return Err(err(format!(
            "{} partial boundary mismatch: exit={:?}, native_retired={}, interpreter_retired={}, remaining={}, blocks={}, state_equal={}",
            case.stratum.id,
            result.exit,
            result.retired_native,
            result.retired_interpreter,
            result.remaining_budget,
            result.blocks_executed,
            *state == *expected_state
        )));
    }
    Ok(())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn validate_control_hit_delta(
    id: &str,
    before: CacheCounts,
    after: CacheCounts,
) -> Result<(), BenchError> {
    let delta = CacheCounts {
        hits: after.hits - before.hits,
        misses: after.misses - before.misses,
        compiles: after.compiles - before.compiles,
        publications: after.publications - before.publications,
        evictions: after.evictions - before.evictions,
    };
    if delta.hits == 0
        || delta.misses != 0
        || delta.compiles != 0
        || delta.publications != 0
        || delta.evictions != 0
    {
        return Err(err(format!(
            "{id} boundary check was not resident hit-only: {delta:?}"
        )));
    }
    Ok(())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn recurrence(mut state: LongModeState, spec: Stratum) -> LongModeState {
    for _ in 0..spec.iterations {
        let pairs = if spec.instructions_per_iteration == 3 {
            1
        } else {
            2
        };
        for _ in 0..pairs {
            state.gpr[0] = state.gpr[3].wrapping_add(state.gpr[1]);
            let previous_rax = state.gpr[0];
            state.gpr[0] = state.gpr[3];
            state.gpr[3] = previous_rax;
        }
        state.rip = BASE_RIP;
    }
    state
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn cache_counts<P>(cache: &SingleEntryBlockCache<P>) -> CacheCounts {
    let c = cache.counters();
    CacheCounts {
        hits: c.hits,
        misses: c.misses,
        compiles: c.successful_compiles,
        publications: c.successful_publications,
        evictions: c.replacements,
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn validate_hit_delta(
    id: &str,
    before: CacheCounts,
    after: CacheCounts,
    expected_hits: u64,
) -> Result<(), BenchError> {
    let d = CacheCounts {
        hits: after.hits - before.hits,
        misses: after.misses - before.misses,
        compiles: after.compiles - before.compiles,
        publications: after.publications - before.publications,
        evictions: after.evictions - before.evictions,
    };
    if d.hits != expected_hits
        || d.misses != 0
        || d.compiles != 0
        || d.publications != 0
        || d.evictions != 0
    {
        return Err(err(format!("{id} warm cache was not hit-only: {d:?}")));
    }
    Ok(())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn sample_native(
    case: &Prepared,
    cache: &mut SingleEntryBlockCache<od_host::PublishedCode>,
    calls: u64,
    pair: usize,
    order: &'static str,
) -> Result<Sample, BenchError> {
    let before = cache_counts(cache);
    let mut state = case.initial.clone();
    let mut elapsed_ns = 0u128;
    let mut last = None;
    let mut retired = 0u64;
    let stop = AtomicU8::new(0);
    let budget = case.stratum.iterations * case.stratum.instructions_per_iteration;
    for _ in 0..calls {
        state.clone_from(&case.initial);
        let view = CodeView::new(BASE_RIP, &case.view_bytes)
            .map_err(|e| err(format!("{} code view: {e:?}", case.stratum.id)))?;
        let start = Instant::now();
        let result = execute_native_view(case, view, &mut state, budget, cache, &stop, None)?;
        elapsed_ns += start.elapsed().as_nanos();
        validate_observation(case, &state, result, budget)?;
        retired = retired
            .checked_add(result.retired_native + result.retired_interpreter)
            .ok_or_else(|| err("native sample retirement count overflow"))?;
        last = Some(result);
    }
    validate_observation(
        case,
        &state,
        last.ok_or_else(|| err("empty native sample"))?,
        budget,
    )?;
    let after = cache_counts(cache);
    let delta = CacheCounts {
        hits: after.hits - before.hits,
        misses: after.misses - before.misses,
        compiles: after.compiles - before.compiles,
        publications: after.publications - before.publications,
        evictions: after.evictions - before.evictions,
    };
    if delta.hits != calls * case.stratum.iterations
        || delta.misses != 0
        || delta.compiles != 0
        || delta.publications != 0
        || delta.evictions != 0
    {
        return Err(err(format!(
            "{} measured native sample not hit-only: {delta:?}",
            case.stratum.id
        )));
    }
    Ok(Sample {
        pair,
        order,
        seed: case.stratum.seed,
        calls,
        elapsed_ns,
        guest_instructions: calls
            * case.stratum.iterations
            * case.stratum.instructions_per_iteration,
        retired,
        cache: delta,
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn sample_interpreter(
    case: &Prepared,
    calls: u64,
    pair: usize,
    order: &'static str,
) -> Result<Sample, BenchError> {
    let mut elapsed_ns = 0u128;
    let mut retired = 0u64;
    let mut last_state = case.initial.clone();
    let mut last_exit = RunExit::InstructionBudgetExhausted;
    let expected_state = recurrence(case.initial.clone(), case.stratum);
    let budget = case.stratum.iterations * case.stratum.instructions_per_iteration;
    for _ in 0..calls {
        last_state.clone_from(&case.initial);
        let mut memory = crate::dispatch_cache::FetchMemory(&case.fetch_bytes);
        let start = Instant::now();
        let result = interpreter_run(
            &mut last_state,
            &mut memory,
            RunConfig {
                instruction_budget: budget,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        elapsed_ns += start.elapsed().as_nanos();
        retired = retired.saturating_add(result.instructions_executed);
        if result.exit != RunExit::InstructionBudgetExhausted
            || result.instructions_executed != budget
            || budget - result.instructions_executed != 0
            || last_state != expected_state
        {
            return Err(err(format!(
                "{} interpreter timed work violated budget",
                case.stratum.id
            )));
        }
        last_exit = result.exit;
    }
    if last_state != expected_state || last_exit != RunExit::InstructionBudgetExhausted {
        return Err(err(format!(
            "{} interpreter timed state mismatch",
            case.stratum.id
        )));
    }
    let work = calls * budget;
    Ok(Sample {
        pair,
        order,
        seed: case.stratum.seed,
        calls,
        elapsed_ns,
        guest_instructions: work,
        retired,
        cache: CacheCounts::default(),
    })
}

fn calls_for(interpreter_ns: u128, native_ns: u128) -> Result<u64, BenchError> {
    if interpreter_ns == 0 || native_ns == 0 {
        return Err(err("zero-duration calibration sample"));
    }
    let per_call = interpreter_ns
        .min(native_ns)
        .div_ceil(u128::from(CALIBRATION_CALLS));
    let calls = MIN_SAMPLE
        .as_nanos()
        .div_ceil(per_call)
        .saturating_mul(6)
        .div_ceil(5)
        .max(u128::from(CALIBRATION_CALLS));
    u64::try_from(calls).map_err(|_| err("calibrated call count overflow"))
}

fn calls_for_sample(
    current_calls: u64,
    interpreter_ns: u128,
    native_ns: u128,
) -> Result<u64, BenchError> {
    let shortest = interpreter_ns.min(native_ns);
    if shortest == 0 {
        return Err(err("zero-duration sample during recalibration"));
    }
    let calls = u128::from(current_calls)
        .saturating_mul(MIN_SAMPLE.as_nanos())
        .div_ceil(shortest)
        .saturating_mul(6)
        .div_ceil(5);
    u64::try_from(calls).map_err(|_| err("sample restart call count overflow"))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn validate_sample_floor(i: &Sample, n: &Sample) -> Result<(), BenchError> {
    if i.elapsed_ns == 0 || n.elapsed_ns == 0 {
        return Err(err("zero-duration calibration"));
    }
    Ok(())
}

fn fingerprints() -> Vec<(&'static str, String)> {
    [
        ("tools/bench/src/cached_native_loop.rs", SOURCE),
        ("tools/bench/src/main.rs", CLI_SOURCE),
        ("tools/bench/src/lib.rs", LIB_SOURCE),
        ("crates/od-jit/src/dispatch.rs", DISPATCH_SOURCE),
        ("crates/od-jit/src/block_compile.rs", COMPILER_SOURCE),
        ("crates/od-isa/src/execute.rs", INTERPRETER_SOURCE),
    ]
    .into_iter()
    .map(|(path, bytes)| (path, od_testkit::sha256_hex(bytes)))
    .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
