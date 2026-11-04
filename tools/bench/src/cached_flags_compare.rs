//! End-to-end eager/candidate comparison for cached NativeRegisterV1 dispatch.
//!
//! Fixed workloads are admitted and checked against the interpreter before
//! timing. Cold samples start with an empty single-entry cache; resident
//! samples require a verified prewarm and hit-only measured counters.

use std::num::NonZeroU64;
use std::sync::atomic::AtomicU8;
use std::time::{Duration, Instant};

use od_isa::LongModeState;
use od_isa::execute::{RunConfig, RunExit, RunResult, run as interpreter_run};
use od_isa::memory::AccessContext;
use od_jit::block::{CodeView, NativeRegisterV1Admission};
use od_jit::block_compile::{
    BlockCompileOutcome, compile_block_for_native_register_v1,
    compile_block_for_native_register_v1_demand_flags_candidate,
};
use od_jit::dispatch::{
    DispatchControl, DispatchExit, FallbackDispatchResult, MAX_SINGLE_ENTRY_CACHE_BYTES,
    SingleEntryBlockCache,
};
use od_jit::translation::{
    AddressSpaceId, BackingId, BackingVersion, CodeDependencySpan, CodeIdentity,
    ExecutionContextId, ExecutionContextSnapshot, MappingId, MappingVersion,
};
use serde::Serialize;

use crate::dispatch_cache::FetchMemory;
use crate::{HostMetadata, MAX_RUN_TIME};

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use od_host::{PublishedBlockExecutor, PublishedCode};

const PROFILE_ID: &str = "s3-29g-native-register-v1-cached-dispatch-v1";
const BASE_RIP: u64 = 0;
const SAMPLE_PAIRS: usize = 20;
const CALIBRATION_CALLS: u64 = 100;
const MIN_SAMPLE: Duration = Duration::from_millis(200);
const MAX_CALLS_PER_SAMPLE: u64 = 10_000_000;
const MAX_CACHE_BYTES: usize = MAX_SINGLE_ENTRY_CACHE_BYTES;
const BENCH_SOURCE: &[u8] = include_bytes!("cached_flags_compare.rs");
const CLI_SOURCE: &[u8] = include_bytes!("main.rs");
const BENCH_LIBRARY_SOURCE: &[u8] = include_bytes!("lib.rs");
const CACHE_IO_SOURCE: &[u8] = include_bytes!("dispatch_cache.rs");
const BUILD_SCRIPT_SOURCE: &[u8] = include_bytes!("../build.rs");
const JIT_DISPATCH_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/dispatch.rs");
const JIT_COMPILER_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/block_compile.rs");
const JIT_BLOCK_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/block.rs");
const JIT_EMITTER_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/block_emitter.rs");
const TRANSLATION_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/translation.rs");
const HOST_EXECUTION_SOURCE: &[u8] = include_bytes!("../../../crates/od-host/src/lib.rs");
const INTERPRETER_SOURCE: &[u8] = include_bytes!("../../../crates/od-isa/src/execute.rs");
const DECODER_SOURCE: &[u8] = include_bytes!("../../../crates/od-isa/src/long_decode.rs");

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
    budget: u64,
}

struct PreparedCase {
    spec: CaseSpec,
    admission: NativeRegisterV1Admission,
    execution_context: ExecutionContextSnapshot,
    dependencies: [CodeDependencySpan; 1],
    interpreter_state: LongModeState,
    interpreter_result: RunResult,
    compile_probe: CompileProbe,
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
pub struct Report {
    schema: u32,
    profile_id: &'static str,
    profile_sha256: String,
    process_run: u8,
    measure_host: HostMetadata,
    cargo_version: String,
    source_fingerprints: Vec<SourceFingerprint>,
    timed_scope: &'static str,
    excluded_from_timing: &'static str,
    mapping_policy: &'static str,
    sampling_protocol: &'static str,
    cases: Vec<CaseReport>,
    total_elapsed_ns: u128,
}

#[derive(Serialize)]
pub struct PreflightReport {
    schema: u32,
    profile_id: &'static str,
    profile_sha256: String,
    measure_host: HostMetadata,
    source_fingerprints: Vec<SourceFingerprint>,
    timing_started: bool,
    cases: Vec<PreflightCase>,
}

#[derive(Serialize)]
struct PreflightCase {
    id: &'static str,
    code_sha256: String,
    interpreter_instructions: u64,
    eager_code_bytes: usize,
    candidate_code_bytes: usize,
    candidate_has_pending_flag_statepoint: bool,
    eager_dispatch_matches_interpreter: bool,
    candidate_dispatch_matches_interpreter: bool,
}

#[derive(Serialize)]
struct SourceFingerprint {
    path: &'static str,
    sha256: String,
}

#[derive(Serialize)]
struct CaseReport {
    id: &'static str,
    code_hex: String,
    code_sha256: String,
    initial_gpr: [u64; 16],
    initial_rip: u64,
    initial_rflags: u64,
    instruction_budget: u64,
    interpreter_exit: String,
    interpreter_state: StateSnapshot,
    interpreter_instructions: u64,
    compile_probe: CompileProbe,
    cold: ModeReport,
    resident: ModeReport,
}

#[derive(Clone, Serialize)]
struct CompileProbe {
    eager_code_bytes: usize,
    eager_code_sha256: String,
    candidate_code_bytes: usize,
    candidate_code_sha256: String,
    candidate_has_pending_flag_statepoint: bool,
}

#[derive(Serialize)]
struct ModeReport {
    calls_per_sample: u64,
    calibration_calls: u64,
    calibration_eager_elapsed_ns: u128,
    calibration_candidate_elapsed_ns: u128,
    eager: VariantReport,
    candidate: VariantReport,
    candidate_over_eager_ratios: Vec<f64>,
}

#[derive(Serialize)]
struct VariantReport {
    samples: Vec<Sample>,
    median_ns_per_dispatch: u128,
    min_ns_per_dispatch: u128,
    max_ns_per_dispatch: u128,
    iqr_ns_per_dispatch: u128,
}

#[derive(Serialize)]
struct Sample {
    pair: usize,
    order: &'static str,
    calls: u64,
    elapsed_ns: u128,
    native_retired: u64,
    interpreter_retired: u64,
    blocks_executed: u64,
    cache_counters: CounterSnapshot,
    warmup_counters: Option<CounterSnapshot>,
    measured_counter_delta: Option<CounterDelta>,
    resident_code_bytes: usize,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct CounterSnapshot {
    hits: u64,
    misses: u64,
    compiles: u64,
    publications: u64,
    replacements: u64,
    resident_entries: usize,
    resident_code_bytes: usize,
    resident_logical_bytes: usize,
    high_water_code_bytes: usize,
    high_water_logical_bytes: usize,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct CounterDelta {
    hits: u64,
    misses: u64,
    compiles: u64,
    publications: u64,
    replacements: u64,
}

#[derive(Default)]
struct CounterTotals {
    hits: u64,
    misses: u64,
    compiles: u64,
    publications: u64,
    replacements: u64,
    resident_code_bytes: usize,
    high_water_code_bytes: usize,
    resident_logical_bytes: usize,
    high_water_logical_bytes: usize,
}

#[derive(Default)]
struct WorkTotals {
    elapsed_ns: u128,
    native_retired: u64,
    interpreter_retired: u64,
    blocks_executed: u64,
}

pub fn run(process_run: u8) -> Result<Report, Box<dyn std::error::Error>> {
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = process_run;
        Err("cached-flags-compare requires native Apple Silicon macOS execution".into())
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    run_native(process_run).map_err(|error| Box::new(error) as Box<dyn std::error::Error>)
}

pub fn preflight() -> Result<PreflightReport, Box<dyn std::error::Error>> {
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        Err("cached-flags-compare requires native Apple Silicon macOS execution".into())
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    preflight_native().map_err(|error| Box::new(error) as Box<dyn std::error::Error>)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn preflight_native() -> Result<PreflightReport, BenchError> {
    if cfg!(debug_assertions) {
        return Err(err(
            "cached-flags-compare preflight requires a release build",
        ));
    }
    let prepared = fixed_profile()
        .into_iter()
        .map(prepare_case)
        .collect::<Result<Vec<_>, _>>()?;
    let mut cases = Vec::with_capacity(prepared.len());
    for case in &prepared {
        validate_strategy(case, false)?;
        validate_strategy(case, true)?;
        cases.push(PreflightCase {
            id: case.spec.id,
            code_sha256: od_testkit::sha256_hex(&case.spec.bytes),
            interpreter_instructions: case.interpreter_result.instructions_executed,
            eager_code_bytes: case.compile_probe.eager_code_bytes,
            candidate_code_bytes: case.compile_probe.candidate_code_bytes,
            candidate_has_pending_flag_statepoint: case
                .compile_probe
                .candidate_has_pending_flag_statepoint,
            eager_dispatch_matches_interpreter: true,
            candidate_dispatch_matches_interpreter: true,
        });
    }
    Ok(PreflightReport {
        schema: 1,
        profile_id: PROFILE_ID,
        profile_sha256: profile_hash(&prepared),
        measure_host: HostMetadata::current(),
        source_fingerprints: source_fingerprints(),
        timing_started: false,
        cases,
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn run_native(process_run: u8) -> Result<Report, BenchError> {
    if !(1..=3).contains(&process_run) {
        return Err(err("process-run must be in 1..=3"));
    }
    if cfg!(debug_assertions) {
        return Err(err("cached-flags-compare requires a release build"));
    }
    let started = Instant::now();
    let prepared = fixed_profile()
        .into_iter()
        .map(prepare_case)
        .collect::<Result<Vec<_>, _>>()?;

    // Complete the semantic preflight globally before calibrating any case.
    for case in &prepared {
        validate_strategy(case, false)?;
        validate_strategy(case, true)?;
    }
    let mut cases = Vec::with_capacity(prepared.len());
    for case in &prepared {
        if started.elapsed() > MAX_RUN_TIME {
            return Err(err("30 minute benchmark run limit exceeded"));
        }
        cases.push(measure_case(case, started)?);
    }
    Ok(Report {
        schema: 1,
        profile_id: PROFILE_ID,
        profile_sha256: profile_hash(&prepared),
        process_run,
        measure_host: HostMetadata::current(),
        cargo_version: command_text("cargo", &["--version"]),
        source_fingerprints: source_fingerprints(),
        timed_scope: "summed per-call duration of the full cached NativeRegisterV1 dispatcher, including key lookup, cold compile/publication on empty-cache calls, native execution, branch/back-edge redispatch and NOP interpreter fallback; input state reset, CodeView construction and result validation occur outside each timed call",
        excluded_from_timing: "workload/profile preparation, interpreter correctness oracle, direct compiler probes, SingleEntryBlockCache construction, state reset, prewarm, per-call cache/PublishedCode drop, validation, result serialization",
        mapping_policy: "one PublishedCode mapping at a time; cold calls drop their cache after the timed dispatch; resident eager and candidate caches are separately prewarmed, sampled, and dropped sequentially before the next strategy is published",
        sampling_protocol: "20 paired samples per case/mode; eager-first and candidate-first order alternate; both sides perform the same guest input and call count; calibration chooses a common count whose summed dispatch durations are at least 200 ms; resident samples require hit deltas and zero miss/compile/publication/replacement deltas",
        cases,
        total_elapsed_ns: started.elapsed().as_nanos(),
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn fixed_profile() -> Vec<CaseSpec> {
    let mut cases = Vec::new();
    let add_cmp = [0x48, 0x03, 0xC3, 0x48, 0x83, 0xF8, 0x05, 0x75, 0x01, 0x90];
    cases.push(case("writer_replace_jnz_taken", &add_cmp, state(4, 2), 16));
    cases.push(case("writer_replace_jnz_nop", &add_cmp, state(4, 1), 16));

    let partial = [0x48, 0x83, 0xE8, 0x01, 0x48, 0xFF, 0xC0, 0x75, 0x01, 0x90];
    cases.push(case("partial_writer_jnz_taken", &partial, state(1, 0), 16));
    cases.push(case("partial_writer_jnz_nop", &partial, state(0, 0), 16));

    // CMP RAX,imm8 is in the admitted NativeRegisterV1 subset; LEA changes a
    // register while preserving the pending arithmetic flags before JZ.
    let preserving = [0x48, 0x83, 0xF8, 0x07, 0x48, 0x8D, 0x03, 0x74, 0x01, 0x90];
    cases.push(case(
        "preserving_lea_jz_taken",
        &preserving,
        state(7, 7),
        16,
    ));
    cases.push(case("preserving_lea_jz_nop", &preserving, state(8, 7), 16));

    let loop_code = [0x48, 0x83, 0xE8, 0x01, 0x75, 0xFA];
    cases.push(case("bounded_sub_jnz_loop", &loop_code, state(16, 0), 32));
    cases
}

fn state(rax: u64, rbx: u64) -> LongModeState {
    let mut state = LongModeState::default();
    state.gpr[0] = rax;
    state.gpr[3] = rbx;
    state.rip = BASE_RIP;
    state.rflags = 0x202;
    state
}

fn case(id: &'static str, bytes: &[u8], initial: LongModeState, budget: u64) -> CaseSpec {
    CaseSpec {
        id,
        bytes: bytes.to_vec(),
        initial,
        budget,
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn prepare_case(spec: CaseSpec) -> Result<PreparedCase, BenchError> {
    let view = CodeView::new(BASE_RIP, &spec.bytes)
        .map_err(|error| err(format!("{} code view: {error:?}", spec.id)))?;
    let admission = NativeRegisterV1Admission::from_code(view)
        .map_err(|error| err(format!("{} admission: {error:?}", spec.id)))?;
    let mut interpreter_state = spec.initial.clone();
    let mut memory = FetchMemory(&spec.bytes);
    let interpreter_result = interpreter_run(
        &mut interpreter_state,
        &mut memory,
        RunConfig {
            instruction_budget: spec.budget,
            stop_rip: Some(view.code_end()),
            context: AccessContext::default(),
        },
        || false,
    );
    if !matches!(
        interpreter_result.exit,
        RunExit::StopAddress | RunExit::InstructionBudgetExhausted
    ) || interpreter_result.instructions_executed == 0
        || interpreter_state.rip != view.code_end()
    {
        return Err(err(format!(
            "{} interpreter preflight ended unexpectedly: {:?} RIP {:#x}",
            spec.id, interpreter_result.exit, interpreter_state.rip
        )));
    }

    let eager = compile_probe(view, &admission, false)?;
    let candidate = compile_probe(view, &admission, true)?;
    if candidate.2 && eager.2 {
        return Err(err(format!(
            "{} eager probe unexpectedly carries pending-flag statepoints",
            spec.id
        )));
    }
    let compile_probe = CompileProbe {
        eager_code_bytes: eager.0.len(),
        eager_code_sha256: od_testkit::sha256_hex(&eager.0),
        candidate_code_bytes: candidate.0.len(),
        candidate_code_sha256: od_testkit::sha256_hex(&candidate.0),
        candidate_has_pending_flag_statepoint: candidate.2,
    };

    let execution_context = ExecutionContextSnapshot::native_register_v1_fixture(
        ExecutionContextId::from_u64(1).expect("non-zero benchmark context"),
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
    Ok(PreparedCase {
        spec,
        admission,
        execution_context,
        dependencies,
        interpreter_state,
        interpreter_result,
        compile_probe,
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn compile_probe(
    view: CodeView<'_>,
    admission: &NativeRegisterV1Admission,
    candidate: bool,
) -> Result<(Vec<u8>, usize, bool), BenchError> {
    use od_jit::statepoints::SnapshotAdmissibility::InadmissiblePendingFlags;

    let outcome = if candidate {
        compile_block_for_native_register_v1_demand_flags_candidate(view, BASE_RIP, admission)
    } else {
        compile_block_for_native_register_v1(view, BASE_RIP, admission)
    }
    .map_err(|error| err(format!("native-register compile probe: {error:?}")))?;
    let BlockCompileOutcome::Compiled(block) = outcome else {
        return Err(err("fixed profile did not produce a native compiled block"));
    };
    let bytes = block
        .emitted()
        .code()
        .to_le_bytes()
        .map_err(|error| err(format!("compiled code bytes: {error:?}")))?;
    if bytes.is_empty() || bytes.len() > 4 * 1024 {
        return Err(err(
            "fixed profile compiled code exceeds the existing 4 KiB cap",
        ));
    }
    let pending = block
        .emitted()
        .pc_map()
        .ranges()
        .iter()
        .any(|range| range.snapshot == InadmissiblePendingFlags);
    if candidate && !pending {
        return Err(err(
            "candidate compile probe has no pending-flags PC-map witness",
        ));
    }
    Ok((bytes, block.emitted().code().words().len() * 4, pending))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn validate_strategy(case: &PreparedCase, candidate: bool) -> Result<(), BenchError> {
    let mut state = case.spec.initial.clone();
    let stop = AtomicU8::new(0);
    let control = DispatchControl {
        stop_rip: None,
        stop_requested: &stop,
        event_requested: None,
    };
    let view = CodeView::new(BASE_RIP, &case.spec.bytes).unwrap();
    let mut cache = SingleEntryBlockCache::<PublishedCode>::new(1, MAX_CACHE_BYTES)
        .map_err(|error| err(format!("cache config: {error:?}")))?;
    let result = dispatch_once(case, candidate, view, &mut state, control, &mut cache)?;
    validate_result(case, &state, result)?;
    let cold_counters = cache.counters();
    if cold_counters.misses != 1
        || cold_counters.successful_compiles != 1
        || cold_counters.successful_publications != 1
        || cache.resident_entries() != 1
        || cache.resident_code_bytes() == 0
    {
        return Err(err(format!(
            "{} preflight did not compile/publish exactly one cached block: {cold_counters:?}",
            case.spec.id
        )));
    }
    let before_resident = counters(&cache);
    state.clone_from(&case.spec.initial);
    let resident_result = dispatch_once(case, candidate, view, &mut state, control, &mut cache)?;
    validate_result(case, &state, resident_result)?;
    let after_resident = counters(&cache);
    let resident_delta = delta(before_resident, after_resident);
    if resident_delta.hits == 0
        || resident_delta.misses != 0
        || resident_delta.compiles != 0
        || resident_delta.publications != 0
        || resident_delta.replacements != 0
        || after_resident.resident_code_bytes != before_resident.resident_code_bytes
    {
        return Err(err(format!(
            "{} preflight resident dispatch was not hit-only: delta={resident_delta:?}",
            case.spec.id
        )));
    }
    Ok(())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn dispatch_once(
    case: &PreparedCase,
    candidate: bool,
    view: CodeView<'_>,
    state: &mut LongModeState,
    control: DispatchControl<'_>,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
) -> Result<FallbackDispatchResult, BenchError> {
    let result = if candidate {
        od_jit::dispatch::run_cached_native_register_v1_demand_flags_candidate_controlled(
            view,
            &case.admission,
            case.execution_context,
            &case.dependencies,
            state,
            case.spec.budget,
            control,
            cache,
            &mut PublishedBlockExecutor,
        )
    } else {
        od_jit::dispatch::run_cached_native_register_v1_controlled(
            view,
            &case.admission,
            case.execution_context,
            &case.dependencies,
            state,
            case.spec.budget,
            control,
            cache,
            &mut PublishedBlockExecutor,
        )
    };
    result.map_err(|error| {
        err(format!(
            "{} cached dispatch failed: {error:?}",
            case.spec.id
        ))
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn validate_result(
    case: &PreparedCase,
    state: &LongModeState,
    result: FallbackDispatchResult,
) -> Result<(), BenchError> {
    let exit_matches = matches!(result.exit, DispatchExit::CodeEnd)
        && matches!(case.interpreter_result.exit, RunExit::StopAddress);
    let retired = result.retired_native + result.retired_interpreter;
    if !exit_matches
        || *state != case.interpreter_state
        || retired != case.interpreter_result.instructions_executed
        || result.remaining_budget != case.spec.budget - retired
    {
        return Err(err(format!(
            "{} cached/interpreter mismatch: dispatch={:?}, interpreter={:?}, state_equal={}, retired={}/{}, remaining={}",
            case.spec.id,
            result.exit,
            case.interpreter_result.exit,
            *state == case.interpreter_state,
            retired,
            case.interpreter_result.instructions_executed,
            result.remaining_budget
        )));
    }
    Ok(())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn counters(cache: &SingleEntryBlockCache<PublishedCode>) -> CounterSnapshot {
    let c = cache.counters();
    CounterSnapshot {
        hits: c.hits,
        misses: c.misses,
        compiles: c.successful_compiles,
        publications: c.successful_publications,
        replacements: c.replacements,
        resident_entries: cache.resident_entries(),
        resident_code_bytes: cache.resident_code_bytes(),
        resident_logical_bytes: cache.resident_logical_bytes(),
        high_water_code_bytes: c.resident_code_bytes_high_water,
        high_water_logical_bytes: c.resident_logical_metadata_bytes_high_water,
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn delta(before: CounterSnapshot, after: CounterSnapshot) -> CounterDelta {
    CounterDelta {
        hits: after.hits.saturating_sub(before.hits),
        misses: after.misses.saturating_sub(before.misses),
        compiles: after.compiles.saturating_sub(before.compiles),
        publications: after.publications.saturating_sub(before.publications),
        replacements: after.replacements.saturating_sub(before.replacements),
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_case(case: &PreparedCase, run_started: Instant) -> Result<CaseReport, BenchError> {
    let cold_eager_calibration = measure_cold(case, false, CALIBRATION_CALLS, 0, "calibration")?;
    let cold_candidate_calibration = measure_cold(case, true, CALIBRATION_CALLS, 0, "calibration")?;
    let resident_eager_calibration =
        measure_resident(case, false, CALIBRATION_CALLS, 0, "calibration")?;
    let resident_candidate_calibration =
        measure_resident(case, true, CALIBRATION_CALLS, 0, "calibration")?;
    let cold_calls = common_calls(
        cold_eager_calibration.elapsed_ns,
        cold_candidate_calibration.elapsed_ns,
    )?;
    let resident_calls = common_calls(
        resident_eager_calibration.elapsed_ns,
        resident_candidate_calibration.elapsed_ns,
    )?;

    let cold = measure_mode(
        case,
        false,
        cold_calls,
        cold_eager_calibration.elapsed_ns,
        cold_candidate_calibration.elapsed_ns,
        run_started,
    )?;
    let resident = measure_mode(
        case,
        true,
        resident_calls,
        resident_eager_calibration.elapsed_ns,
        resident_candidate_calibration.elapsed_ns,
        run_started,
    )?;

    Ok(CaseReport {
        id: case.spec.id,
        code_hex: hex(&case.spec.bytes),
        code_sha256: od_testkit::sha256_hex(&case.spec.bytes),
        initial_gpr: case.spec.initial.gpr,
        initial_rip: case.spec.initial.rip,
        initial_rflags: case.spec.initial.rflags,
        instruction_budget: case.spec.budget,
        interpreter_exit: format!("{:?}", case.interpreter_result.exit),
        interpreter_state: StateSnapshot::from(&case.interpreter_state),
        interpreter_instructions: case.interpreter_result.instructions_executed,
        compile_probe: case.compile_probe.clone(),
        cold,
        resident,
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn common_calls(eager_elapsed_ns: u128, candidate_elapsed_ns: u128) -> Result<u64, BenchError> {
    let target = MIN_SAMPLE.as_nanos();
    let eager = calls_for_target(eager_elapsed_ns, target)?;
    let candidate = calls_for_target(candidate_elapsed_ns, target)?;
    let calls = eager.max(candidate).max(CALIBRATION_CALLS);
    if calls > MAX_CALLS_PER_SAMPLE {
        return Err(err(format!(
            "calibrated sample requires {calls} calls; cap is {MAX_CALLS_PER_SAMPLE}"
        )));
    }
    Ok(calls)
}

fn calls_for_target(elapsed_ns: u128, target_ns: u128) -> Result<u64, BenchError> {
    if elapsed_ns == 0 {
        return Err(err("calibration elapsed time is zero"));
    }
    let scaled = target_ns
        .checked_mul(u128::from(CALIBRATION_CALLS))
        .ok_or_else(|| err("calibration count overflow"))?;
    let calls = scaled.div_ceil(elapsed_ns).saturating_mul(6).div_ceil(5);
    u64::try_from(calls).map_err(|_| err("calibrated call count exceeds u64"))
}

fn scale_calls(current_calls: u64, elapsed_ns: u128, target_ns: u128) -> Result<u64, BenchError> {
    if elapsed_ns == 0 {
        return Err(err("sample elapsed time is zero"));
    }
    let scaled = u128::from(current_calls)
        .checked_mul(target_ns)
        .ok_or_else(|| err("sample restart count overflow"))?;
    let calls = scaled.div_ceil(elapsed_ns).saturating_mul(6).div_ceil(5);
    u64::try_from(calls).map_err(|_| err("sample restart count exceeds u64"))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_mode(
    case: &PreparedCase,
    resident: bool,
    mut calls: u64,
    eager_calibration_ns: u128,
    candidate_calibration_ns: u128,
    run_started: Instant,
) -> Result<ModeReport, BenchError> {
    for restart in 0..8 {
        let mut eager_samples = Vec::with_capacity(SAMPLE_PAIRS);
        let mut candidate_samples = Vec::with_capacity(SAMPLE_PAIRS);
        let mut too_short = None::<u128>;
        for pair in 0..SAMPLE_PAIRS {
            if run_started.elapsed() > MAX_RUN_TIME {
                return Err(err(
                    "30 minute benchmark run limit exceeded during paired samples",
                ));
            }
            let eager_first = pair % 2 == 0;
            let (first_candidate, first_order, second_candidate, second_order) = if eager_first {
                (false, "eager_first", true, "candidate_second")
            } else {
                (true, "candidate_first", false, "eager_second")
            };
            let first = if resident {
                measure_resident(case, first_candidate, calls, pair, first_order)?
            } else {
                measure_cold(case, first_candidate, calls, pair, first_order)?
            };
            let second = if resident {
                measure_resident(case, second_candidate, calls, pair, second_order)?
            } else {
                measure_cold(case, second_candidate, calls, pair, second_order)?
            };
            let shortest = first.elapsed_ns.min(second.elapsed_ns);
            if shortest < MIN_SAMPLE.as_nanos() {
                too_short = Some(shortest);
                break;
            }
            if eager_first {
                eager_samples.push(first);
                candidate_samples.push(second);
            } else {
                candidate_samples.push(first);
                eager_samples.push(second);
            }
        }
        if let Some(shortest) = too_short {
            if restart == 7 {
                return Err(err(format!(
                    "{} {} could not reach the 200 ms paired-sample floor",
                    case.spec.id,
                    if resident { "resident" } else { "cold" }
                )));
            }
            calls = scale_calls(calls, shortest, MIN_SAMPLE.as_nanos())?.max(calls + 1);
            if calls > MAX_CALLS_PER_SAMPLE {
                return Err(err("sample restart exceeded the per-sample call cap"));
            }
            continue;
        }
        let ratios = eager_samples
            .iter()
            .zip(&candidate_samples)
            .map(|(eager, candidate)| {
                (candidate.elapsed_ns as f64 / candidate.calls as f64)
                    / (eager.elapsed_ns as f64 / eager.calls as f64)
            })
            .collect();
        return Ok(ModeReport {
            calls_per_sample: calls,
            calibration_calls: CALIBRATION_CALLS,
            calibration_eager_elapsed_ns: eager_calibration_ns,
            calibration_candidate_elapsed_ns: candidate_calibration_ns,
            eager: summarize(eager_samples),
            candidate: summarize(candidate_samples),
            candidate_over_eager_ratios: ratios,
        });
    }
    Err(err("unreachable calibration restart state"))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_cold(
    case: &PreparedCase,
    candidate: bool,
    calls: u64,
    pair: usize,
    order: &'static str,
) -> Result<Sample, BenchError> {
    let mut state = case.spec.initial.clone();
    let stop = AtomicU8::new(0);
    let control = DispatchControl {
        stop_rip: None,
        stop_requested: &stop,
        event_requested: None,
    };
    let mut totals = WorkTotals::default();
    let mut counters_total = CounterTotals::default();
    for _ in 0..calls {
        state.clone_from(&case.spec.initial);
        let view = CodeView::new(BASE_RIP, &case.spec.bytes).unwrap();
        let mut cache = SingleEntryBlockCache::<PublishedCode>::new(1, MAX_CACHE_BYTES)
            .map_err(|error| err(format!("cache config: {error:?}")))?;
        let started = Instant::now();
        let result = dispatch_once(case, candidate, view, &mut state, control, &mut cache);
        totals.elapsed_ns += started.elapsed().as_nanos();
        let result = result?;
        validate_result(case, &state, result)?;
        let snapshot = counters(&cache);
        if snapshot.misses != 1
            || snapshot.compiles != 1
            || snapshot.publications != 1
            || snapshot.replacements != 0
            || snapshot.resident_entries != 1
        {
            return Err(err(format!(
                "{} cold dispatch was not one fresh-cache miss/compile/publication: {snapshot:?}",
                case.spec.id
            )));
        }
        totals.native_retired += result.retired_native;
        totals.interpreter_retired += result.retired_interpreter;
        totals.blocks_executed += result.blocks_executed;
        counters_total.add(snapshot);
        // Drop the resident PublishedCode mapping before constructing the next
        // empty cache, and before the other strategy can publish its mapping.
        drop(cache);
    }
    verify_batch(case, calls, &totals)?;
    Ok(Sample {
        pair,
        order,
        calls,
        elapsed_ns: totals.elapsed_ns,
        native_retired: totals.native_retired,
        interpreter_retired: totals.interpreter_retired,
        blocks_executed: totals.blocks_executed,
        cache_counters: counters_total.snapshot(calls),
        warmup_counters: None,
        measured_counter_delta: None,
        resident_code_bytes: counters_total.high_water_code_bytes,
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_resident(
    case: &PreparedCase,
    candidate: bool,
    calls: u64,
    pair: usize,
    order: &'static str,
) -> Result<Sample, BenchError> {
    let mut cache = SingleEntryBlockCache::<PublishedCode>::new(1, MAX_CACHE_BYTES)
        .map_err(|error| err(format!("cache config: {error:?}")))?;
    let stop = AtomicU8::new(0);
    let control = DispatchControl {
        stop_rip: None,
        stop_requested: &stop,
        event_requested: None,
    };
    let view = CodeView::new(BASE_RIP, &case.spec.bytes).unwrap();
    let mut state = case.spec.initial.clone();
    let prewarm_result = dispatch_once(case, candidate, view, &mut state, control, &mut cache)?;
    validate_result(case, &state, prewarm_result)?;
    let warmup_counters = counters(&cache);
    if warmup_counters.misses != 1
        || warmup_counters.compiles != 1
        || warmup_counters.publications != 1
        || warmup_counters.resident_entries != 1
        || warmup_counters.resident_code_bytes == 0
    {
        return Err(err(format!(
            "{} resident prewarm did not publish one resident block: {warmup_counters:?}",
            case.spec.id
        )));
    }

    let before = counters(&cache);
    let mut totals = WorkTotals::default();
    for _ in 0..calls {
        state.clone_from(&case.spec.initial);
        let started = Instant::now();
        let result = dispatch_once(case, candidate, view, &mut state, control, &mut cache);
        totals.elapsed_ns += started.elapsed().as_nanos();
        let result = result?;
        validate_result(case, &state, result)?;
        totals.native_retired += result.retired_native;
        totals.interpreter_retired += result.retired_interpreter;
        totals.blocks_executed += result.blocks_executed;
    }
    let after = counters(&cache);
    let counter_delta = delta(before, after);
    if counter_delta.hits < calls
        || counter_delta.misses != 0
        || counter_delta.compiles != 0
        || counter_delta.publications != 0
        || counter_delta.replacements != 0
        || after.resident_entries != 1
        || after.resident_code_bytes != warmup_counters.resident_code_bytes
    {
        return Err(err(format!(
            "{} resident sample was not hit-only: delta={counter_delta:?}, after={after:?}",
            case.spec.id
        )));
    }
    verify_batch(case, calls, &totals)?;
    Ok(Sample {
        pair,
        order,
        calls,
        elapsed_ns: totals.elapsed_ns,
        native_retired: totals.native_retired,
        interpreter_retired: totals.interpreter_retired,
        blocks_executed: totals.blocks_executed,
        cache_counters: after,
        warmup_counters: Some(warmup_counters),
        measured_counter_delta: Some(counter_delta),
        resident_code_bytes: after.resident_code_bytes,
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl CounterTotals {
    fn add(&mut self, snapshot: CounterSnapshot) {
        self.hits += snapshot.hits;
        self.misses += snapshot.misses;
        self.compiles += snapshot.compiles;
        self.publications += snapshot.publications;
        self.replacements += snapshot.replacements;
        self.resident_code_bytes = self.resident_code_bytes.max(snapshot.resident_code_bytes);
        self.high_water_code_bytes = self
            .high_water_code_bytes
            .max(snapshot.high_water_code_bytes);
        self.resident_logical_bytes = self
            .resident_logical_bytes
            .max(snapshot.resident_logical_bytes);
        self.high_water_logical_bytes = self
            .high_water_logical_bytes
            .max(snapshot.high_water_logical_bytes);
    }

    fn snapshot(&self, calls: u64) -> CounterSnapshot {
        CounterSnapshot {
            hits: self.hits,
            misses: self.misses,
            compiles: self.compiles,
            publications: self.publications,
            replacements: self.replacements,
            resident_entries: usize::from(calls > 0),
            resident_code_bytes: self.resident_code_bytes,
            resident_logical_bytes: self.resident_logical_bytes,
            high_water_code_bytes: self.high_water_code_bytes,
            high_water_logical_bytes: self.high_water_logical_bytes,
        }
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn verify_batch(case: &PreparedCase, calls: u64, totals: &WorkTotals) -> Result<(), BenchError> {
    let retired_per_call = case.interpreter_result.instructions_executed;
    let expected = retired_per_call
        .checked_mul(calls)
        .ok_or_else(|| err("batch retirement overflow"))?;
    if totals.native_retired + totals.interpreter_retired != expected {
        return Err(err(format!(
            "{} aggregate retirement mismatch: {} expected {expected}",
            case.spec.id,
            totals.native_retired + totals.interpreter_retired
        )));
    }
    Ok(())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn summarize(samples: Vec<Sample>) -> VariantReport {
    let mut per_call = samples
        .iter()
        .map(|sample| sample.elapsed_ns / u128::from(sample.calls))
        .collect::<Vec<_>>();
    per_call.sort_unstable();
    let median = percentile(&per_call, 50);
    let iqr = percentile(&per_call, 75).saturating_sub(percentile(&per_call, 25));
    VariantReport {
        min_ns_per_dispatch: *per_call.first().unwrap_or(&0),
        max_ns_per_dispatch: *per_call.last().unwrap_or(&0),
        median_ns_per_dispatch: median,
        iqr_ns_per_dispatch: iqr,
        samples,
    }
}

fn percentile(sorted: &[u128], percentile: usize) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let index = (sorted.len() - 1) * percentile / 100;
    sorted[index]
}

fn profile_hash(cases: &[PreparedCase]) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(PROFILE_ID.as_bytes());
    for case in cases {
        bytes.extend_from_slice(case.spec.id.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&case.spec.bytes);
        bytes.extend_from_slice(&case.spec.budget.to_le_bytes());
        bytes.extend_from_slice(&case.spec.initial.rip.to_le_bytes());
        bytes.extend_from_slice(&case.spec.initial.rflags.to_le_bytes());
        for register in case.spec.initial.gpr {
            bytes.extend_from_slice(&register.to_le_bytes());
        }
    }
    od_testkit::sha256_hex(&bytes)
}

fn source_fingerprints() -> Vec<SourceFingerprint> {
    [
        ("tools/bench/src/cached_flags_compare.rs", BENCH_SOURCE),
        ("tools/bench/src/main.rs", CLI_SOURCE),
        ("tools/bench/src/lib.rs", BENCH_LIBRARY_SOURCE),
        ("tools/bench/src/dispatch_cache.rs", CACHE_IO_SOURCE),
        ("tools/bench/build.rs", BUILD_SCRIPT_SOURCE),
        ("crates/od-jit/src/dispatch.rs", JIT_DISPATCH_SOURCE),
        ("crates/od-jit/src/block_compile.rs", JIT_COMPILER_SOURCE),
        ("crates/od-jit/src/block.rs", JIT_BLOCK_SOURCE),
        ("crates/od-jit/src/block_emitter.rs", JIT_EMITTER_SOURCE),
        ("crates/od-jit/src/translation.rs", TRANSLATION_SOURCE),
        ("crates/od-host/src/lib.rs", HOST_EXECUTION_SOURCE),
        ("crates/od-isa/src/execute.rs", INTERPRETER_SOURCE),
        ("crates/od-isa/src/long_decode.rs", DECODER_SOURCE),
    ]
    .into_iter()
    .map(|(path, bytes)| SourceFingerprint {
        path,
        sha256: od_testkit::sha256_hex(bytes),
    })
    .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn command_text(program: &str, args: &[&str]) -> String {
    std::process::Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unavailable".into())
}
