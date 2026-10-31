//! Cached eager/demand-flags comparison over the frozen S3-34p composed corpus.

use std::io::Read;
use std::num::NonZeroU64;
use std::path::Path;
use std::sync::atomic::AtomicU8;
use std::time::{Duration, Instant};

use od_host::{PublishedBlockExecutor, PublishedCode};
use od_isa::LongModeState;
use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError};
use od_jit::block::{CodeView, NativeRegisterV1Admission};
use od_jit::dispatch::{
    DispatchControl, DispatchExit, FallbackDispatchResult, MAX_SINGLE_ENTRY_CACHE_BYTES,
    SingleEntryBlockCache,
};
use od_jit::translation::{
    AddressSpaceId, BackingId, BackingVersion, CodeDependencySpan, CodeIdentity,
    ExecutionContextId, ExecutionContextSnapshot, MappingId, MappingVersion,
};
use serde::{Deserialize, Serialize};

use crate::{HostMetadata, MAX_RUN_TIME};

const PROFILE_ID: &str = "s3-40d-native-register-v1-cached-composed-flags-v1";
const CORPUS_PROFILE: &str = "NativeRegisterV1";
const EXPECTED_CASES: usize = 1_200;
const FROZEN_INPUTS_DIGEST: &str = "fnv1a64:172fb17b01c502df";
const FROZEN_SEEDS: [u64; 3] = [
    0x5333_346e_0000_0001,
    0x5333_346e_0000_0002,
    0x5333_346e_0000_0003,
];
const MIN_SAMPLE: Duration = Duration::from_millis(200);
const MAX_SWEEPS_PER_SAMPLE: u64 = 100_000;
const SAMPLE_PAIRS: usize = 20;
const SOURCE: &[u8] = include_bytes!("cached_native_flags_compare.rs");
const CLI_SOURCE: &[u8] = include_bytes!("main.rs");
const LIB_SOURCE: &[u8] = include_bytes!("lib.rs");
const BENCH_MANIFEST_SOURCE: &[u8] = include_bytes!("../Cargo.toml");
const JIT_MANIFEST_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/Cargo.toml");
const DISPATCH_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/dispatch.rs");
const COMPILER_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/block_compile.rs");
const BLOCK_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/block.rs");
const EMITTER_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/block_emitter.rs");
const TRANSLATION_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/translation.rs");
const STATEPOINT_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/statepoints.rs");
const HOST_SOURCE: &[u8] = include_bytes!("../../../crates/od-host/src/lib.rs");
const ISA_SOURCE: &[u8] = include_bytes!("../../../crates/od-isa/src/execute.rs");
const DECODE_SOURCE: &[u8] = include_bytes!("../../../crates/od-isa/src/long_decode.rs");
const STATE_SOURCE: &[u8] = include_bytes!("../../../crates/od-isa/src/state.rs");
const CORPUS_SOURCE: &[u8] =
    include_bytes!("../../../crates/od-host/src/tests/native_register_v1_corpus.rs");

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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Input {
    version: u32,
    profile: String,
    seed: u64,
    index: u32,
    code: Vec<u8>,
    guest_base_rip: u64,
    initial_gpr: [u64; 16],
    initial_rip: u64,
    initial_rflags: u64,
    initial_auxiliary_state: String,
    stop_rip: Option<u64>,
    stop_requested: bool,
    budget: u64,
    input_digest: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    gpr: [u64; 16],
    rip: u64,
    rflags: u64,
    auxiliary_state: String,
    exit: String,
    retired_native: u64,
    retired_interpreter: u64,
    remaining_budget: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BackendRuns {
    interpreter: [Option<Observation>; 2],
    eager_native: [Option<Observation>; 2],
    demand_candidate: [Option<Observation>; 2],
    errors: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CorpusObservation {
    input: Input,
    backend_runs: BackendRuns,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CorpusEnvelope {
    version: u32,
    profile: String,
    inputs_digest: String,
    cases: Vec<CorpusObservation>,
    envelope_digest: String,
}

#[derive(Serialize)]
struct CanonicalInput<'a> {
    code: &'a [u8],
    guest_base_rip: u64,
    initial_gpr: &'a [u64; 16],
    initial_rip: u64,
    initial_rflags: u64,
    initial_auxiliary_state: &'a str,
    stop_rip: Option<u64>,
    stop_requested: bool,
    budget: u64,
}

#[derive(Serialize)]
struct EnvelopePayload<'a> {
    version: u32,
    profile: &'a str,
    inputs_digest: &'a str,
    cases: &'a [CorpusObservation],
}

struct PreparedCase {
    input: Input,
    admission: NativeRegisterV1Admission,
    context: ExecutionContextSnapshot,
    dependencies: [CodeDependencySpan; 1],
    expected_state: LongModeState,
    expected_exit: &'static str,
    expected_retired: u64,
    eager_retired_native: u64,
    eager_retired_interpreter: u64,
    candidate_retired_native: u64,
    candidate_retired_interpreter: u64,
    eager_cache: Option<CacheBehavior>,
    candidate_cache: Option<CacheBehavior>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
struct CacheCounts {
    hits: u64,
    misses: u64,
    compiles: u64,
    publications: u64,
    replacements: u64,
    published_code_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct CacheBehavior {
    cold: CacheCounts,
    repeat: CacheCounts,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct CaseCacheBaseline {
    seed: u64,
    index: u32,
    eager: CacheBehavior,
    candidate: CacheBehavior,
}

#[derive(Clone, Copy, Debug, Default)]
struct CacheSnapshot {
    counts: CacheCounts,
    resident_entries: usize,
    resident_code_bytes: usize,
    resident_logical_bytes: usize,
    peak_resident_entries: usize,
    max_resident_code_bytes: usize,
    max_resident_logical_bytes: usize,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct CacheCounters {
    hits: u64,
    misses: u64,
    compiles: u64,
    publications: u64,
    replacements: u64,
    published_code_bytes_total: u64,
    peak_resident_entries: usize,
    max_resident_code_bytes: usize,
    max_resident_logical_bytes: usize,
    final_resident_code_bytes_per_sweep: u128,
    min_final_resident_code_bytes: usize,
    max_final_resident_code_bytes: usize,
}

#[derive(Clone, Serialize)]
struct SourceFingerprint {
    path: &'static str,
    sha256: String,
}

#[derive(Serialize)]
pub struct PreflightReport {
    schema: u32,
    profile_id: &'static str,
    corpus_path: String,
    corpus_sha256: String,
    inputs_digest: String,
    case_count: usize,
    source_fingerprints: Vec<SourceFingerprint>,
    measure_host: HostMetadata,
    timing_started: bool,
    parity_checked: usize,
    stored_interpreter_observations_checked: usize,
    cache_baselines: Vec<CaseCacheBaseline>,
}

#[derive(Serialize)]
pub struct Report {
    schema: u32,
    profile_id: &'static str,
    process_run: u8,
    corpus_path: String,
    corpus_sha256: String,
    inputs_digest: String,
    source_fingerprints: Vec<SourceFingerprint>,
    measure_host: HostMetadata,
    cargo_version: String,
    timed_scope: &'static str,
    excluded_from_timing: &'static str,
    cache_policy: &'static str,
    cache_counter_instrumentation: &'static str,
    sampling_protocol: &'static str,
    case_count: usize,
    cache_baselines: Vec<CaseCacheBaseline>,
    cold: ModeReport,
    repeat_dispatch: ModeReport,
    total_elapsed_ns: u128,
}

#[derive(Serialize)]
struct ModeReport {
    sweeps_per_sample: u64,
    sample_pairs: usize,
    calibration: CalibrationEvidence,
    restarts: Vec<SampleRestart>,
    eager_preflight_counts_per_sweep: CacheCounts,
    candidate_preflight_counts_per_sweep: CacheCounts,
    repeat_dispatch_description: &'static str,
    eager: VariantReport,
    candidate: VariantReport,
    candidate_over_eager_ratios: Vec<f64>,
}

#[derive(Serialize)]
struct CalibrationEvidence {
    eager_sample: Sample,
    candidate_sample: Sample,
    initial_sweeps_per_sample: u64,
}

#[derive(Serialize)]
struct SampleRestart {
    attempt: usize,
    trigger_pair: usize,
    shortest_elapsed_ns: u128,
    previous_sweeps_per_sample: u64,
    next_sweeps_per_sample: u64,
    discarded_eager_samples: Vec<Sample>,
    discarded_candidate_samples: Vec<Sample>,
}

#[derive(Serialize)]
struct VariantReport {
    samples: Vec<Sample>,
    median_ns_per_case: u128,
    min_ns_per_case: u128,
    max_ns_per_case: u128,
    iqr_ns_per_case: u128,
    peak_resident_entries: usize,
}

#[derive(Clone, Serialize)]
struct Sample {
    pair: usize,
    order: &'static str,
    sweeps: u64,
    cases: u64,
    elapsed_ns: u128,
    native_retired: u64,
    interpreter_retired: u64,
    blocks_executed: u64,
    counters: CacheCounters,
}

#[derive(Default)]
struct Totals {
    elapsed_ns: u128,
    native_retired: u64,
    interpreter_retired: u64,
    blocks_executed: u64,
    hits: u64,
    misses: u64,
    compiles: u64,
    publications: u64,
    replacements: u64,
    published_code_bytes_total: u64,
    code_bytes_high_water: usize,
    logical_bytes_high_water: usize,
    final_resident_code_bytes: u128,
    min_final_resident_code_bytes: usize,
    max_final_resident_code_bytes: usize,
    peak_resident_entries: usize,
}

pub fn preflight(path: &Path) -> Result<PreflightReport, Box<dyn std::error::Error>> {
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = path;
        Err("cached-native-flags-compare requires native Apple Silicon macOS execution".into())
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    preflight_native(path).map_err(|error| Box::new(error) as Box<dyn std::error::Error>)
}

pub fn run(path: &Path, process_run: u8) -> Result<Report, Box<dyn std::error::Error>> {
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (path, process_run);
        Err("cached-native-flags-compare requires native Apple Silicon macOS execution".into())
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    run_native(path, process_run).map_err(|error| Box::new(error) as Box<dyn std::error::Error>)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn load_profile(path: &Path) -> Result<(Vec<PreparedCase>, String, String), BenchError> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| err(format!("open corpus: {e}")))?
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| err(format!("read corpus: {e}")))?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(err("frozen corpus JSON exceeds 64 MiB limit"));
    }
    let corpus_sha256 = od_testkit::sha256_hex(&bytes);
    let envelope: CorpusEnvelope = serde_json::from_slice(&bytes)
        .map_err(|e| err(format!("decode S3-34p corpus envelope: {e}")))?;
    validate_envelope(&envelope)?;
    let mut prepared = Vec::with_capacity(envelope.cases.len());
    for case in envelope.cases {
        prepared.push(prepare_case(case)?);
    }
    Ok((prepared, corpus_sha256, envelope.inputs_digest))
}

fn validate_envelope(envelope: &CorpusEnvelope) -> Result<(), BenchError> {
    if envelope.version != 1 || envelope.profile != CORPUS_PROFILE {
        return Err(err("unsupported frozen corpus version/profile"));
    }
    if envelope.cases.len() != EXPECTED_CASES {
        return Err(err(format!(
            "expected exactly {EXPECTED_CASES} frozen inputs"
        )));
    }
    if envelope.inputs_digest != FROZEN_INPUTS_DIGEST {
        return Err(err("input set does not match the frozen S3-34p corpus"));
    }
    let mut input_hash = 0xcbf2_9ce4_8422_2325u64;
    let mut seen = std::collections::HashSet::with_capacity(envelope.cases.len());
    for (sequence, case) in envelope.cases.iter().enumerate() {
        let input = &case.input;
        let expected_seed = FROZEN_SEEDS[sequence / 400];
        let expected_index = (sequence % 400) as u32;
        if input.version != 1
            || input.profile != CORPUS_PROFILE
            || input.code.is_empty()
            || input.code.len() > 96
            || input.budget > 16
            || input.initial_rip != input.guest_base_rip
            || input.seed != expected_seed
            || input.index != expected_index
            || input.initial_auxiliary_state != default_auxiliary_state()
            || input.input_digest != digest(&canonical_input(input))
        {
            return Err(err(format!("invalid frozen input at sequence {sequence}")));
        }
        let canonical = canonical_input(input);
        if !seen.insert(canonical.clone()) {
            return Err(err(format!(
                "duplicate frozen input at sequence {sequence}"
            )));
        }
        for byte in canonical {
            input_hash = (input_hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
        }
        if case.backend_runs.errors.len() != 0
            || case.backend_runs.interpreter[0].is_none()
            || case.backend_runs.interpreter[0] != case.backend_runs.interpreter[1]
        {
            return Err(err(format!(
                "invalid frozen oracle observations at sequence {sequence}"
            )));
        }
    }
    let inputs_digest = format!("fnv1a64:{input_hash:016x}");
    if envelope.inputs_digest != inputs_digest {
        return Err(err("frozen corpus input digest mismatch"));
    }
    let payload = serde_json::to_vec(&EnvelopePayload {
        version: envelope.version,
        profile: &envelope.profile,
        inputs_digest: &envelope.inputs_digest,
        cases: &envelope.cases,
    })
    .map_err(|e| err(format!("serialize frozen corpus envelope: {e}")))?;
    if envelope.envelope_digest != digest(&payload) {
        return Err(err("frozen corpus envelope digest mismatch"));
    }
    Ok(())
}

fn canonical_input(input: &Input) -> Vec<u8> {
    serde_json::to_vec(&CanonicalInput {
        code: &input.code,
        guest_base_rip: input.guest_base_rip,
        initial_gpr: &input.initial_gpr,
        initial_rip: input.initial_rip,
        initial_rflags: input.initial_rflags,
        initial_auxiliary_state: &input.initial_auxiliary_state,
        stop_rip: input.stop_rip,
        stop_requested: input.stop_requested,
        budget: input.budget,
    })
    .expect("canonical corpus input serialization")
}

fn digest(bytes: &[u8]) -> String {
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
    });
    format!("fnv1a64:{hash:016x}")
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn preflight_native(path: &Path) -> Result<PreflightReport, BenchError> {
    if cfg!(debug_assertions) {
        return Err(err("preflight requires a release build"));
    }
    let (mut cases, corpus_sha256, inputs_digest) = load_profile(path)?;
    for case in &mut cases {
        case.eager_cache = Some(validate_strategy(case, false)?);
        case.candidate_cache = Some(validate_strategy(case, true)?);
    }
    Ok(PreflightReport {
        schema: 1,
        profile_id: PROFILE_ID,
        corpus_path: path.display().to_string(),
        corpus_sha256,
        inputs_digest,
        case_count: cases.len(),
        source_fingerprints: source_fingerprints(),
        measure_host: HostMetadata::current(),
        timing_started: false,
        parity_checked: cases.len() * 2,
        stored_interpreter_observations_checked: cases.len(),
        cache_baselines: cache_baselines(&cases)?,
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn run_native(path: &Path, process_run: u8) -> Result<Report, BenchError> {
    if !(1..=3).contains(&process_run) {
        return Err(err("process-run must be in 1..=3"));
    }
    if cfg!(debug_assertions) {
        return Err(err("requires a release build"));
    }
    let started = Instant::now();
    let (mut cases, corpus_sha256, inputs_digest) = load_profile(path)?;
    for case in &mut cases {
        case.eager_cache = Some(validate_strategy(case, false)?);
        case.candidate_cache = Some(validate_strategy(case, true)?);
    }
    let cold = measure_mode(&cases, false, started)?;
    let repeat_dispatch = measure_mode(&cases, true, started)?;
    Ok(Report {
        schema: 1,
        profile_id: PROFILE_ID,
        process_run,
        corpus_path: path.display().to_string(),
        corpus_sha256,
        inputs_digest,
        source_fingerprints: source_fingerprints(),
        measure_host: HostMetadata::current(),
        cargo_version: command_text("cargo", &["--version"]),
        timed_scope: "sum of intervals placed immediately around the cached eager or demand-candidate dispatcher entrypoint over the same 1,200 ordered inputs; first-dispatch mode starts with a fresh cache; repeat-dispatch mode measures a second complete call after one excluded first call on the same per-input cache",
        excluded_from_timing: "corpus validation, interpreter oracle, state reset, stop/control setup, cache construction, CodeView/admission/context/dependency setup, repeat-mode prewarm, per-case cache drop, result validation, report serialization",
        cache_policy: "one fresh SingleEntryBlockCache (capacity 1, existing byte cap) per input and strategy; first and second complete dispatches may have multiple hits, misses, compiles, publications or replacements due composed control flow; each per-input counter delta is frozen in preflight and rechecked; each PublishedCode is dropped before the next input or strategy",
        cache_counter_instrumentation: "od-jit phase-observation benchmark feature adds one checked cumulative published-code-byte addition after each successful publication; no hit-path work; its publication-path cost is included identically for eager and candidate and was not separately latency-measured",
        sampling_protocol: "20 paired eager/candidate samples per mode; strategy order alternates; each pair uses the same frozen ordered inputs and sweep count; calibration/restarts enforce at least 200 ms summed dispatcher intervals on both sides; each external input/state/stop setup is identical and outside timing; every result validates against the recomputed interpreter oracle and frozen eager/candidate retirement split",
        case_count: cases.len(),
        cache_baselines: cache_baselines(&cases)?,
        cold,
        repeat_dispatch,
        total_elapsed_ns: started.elapsed().as_nanos(),
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn prepare_case(case: CorpusObservation) -> Result<PreparedCase, BenchError> {
    let CorpusObservation {
        input,
        backend_runs,
    } = case;
    let view = CodeView::new(input.guest_base_rip, &input.code)
        .map_err(|e| err(format!("case {} CodeView: {e:?}", input.index)))?;
    let admission = NativeRegisterV1Admission::from_code(view)
        .map_err(|e| err(format!("case {} admission: {e:?}", input.index)))?;
    let mut expected_state = state_from_input(&input);
    let mut memory = CodeMemory {
        base: input.guest_base_rip,
        bytes: &input.code,
    };
    let stop = AtomicU8::new(u8::from(input.stop_requested));
    let mut retired = 0;
    let expected_exit = loop {
        if input.stop_rip == Some(expected_state.rip) {
            break "StopAddress";
        }
        if stop.load(std::sync::atomic::Ordering::Acquire) != 0 {
            break "StopRequested";
        }
        if expected_state.rip == view.code_end() {
            break "CodeEnd";
        }
        if retired >= input.budget {
            break "BudgetExhausted";
        }
        match od_isa::execute::step(&mut expected_state, &mut memory, AccessContext::default())
            .map_err(|e| err(format!("case {} interpreter: {e:?}", input.index)))?
        {
            od_isa::execute::StepOutcome::Advanced | od_isa::execute::StepOutcome::Branched => {
                retired += 1
            }
            other => {
                return Err(err(format!(
                    "case {} unexpected interpreter outcome {other:?}",
                    input.index
                )));
            }
        }
    };
    let interpreter = backend_runs.interpreter[0].as_ref().ok_or_else(|| {
        err(format!(
            "case {} is missing interpreter observation",
            input.index
        ))
    })?;
    validate_stored_observation(&input, interpreter, &expected_state, expected_exit, retired)?;
    let eager = backend_runs.eager_native[0]
        .as_ref()
        .ok_or_else(|| err(format!("case {} is missing eager observation", input.index)))?;
    let candidate = backend_runs.demand_candidate[0].as_ref().ok_or_else(|| {
        err(format!(
            "case {} is missing candidate observation",
            input.index
        ))
    })?;
    if backend_runs.eager_native[1].as_ref() != Some(eager)
        || backend_runs.demand_candidate[1].as_ref() != Some(candidate)
    {
        return Err(err(format!(
            "case {} stored JIT repeats differ",
            input.index
        )));
    }
    validate_stored_observation(&input, eager, &expected_state, expected_exit, retired)?;
    validate_stored_observation(&input, candidate, &expected_state, expected_exit, retired)?;
    if eager.retired_native != interpreter.retired_native
        || eager.retired_interpreter != interpreter.retired_interpreter
        || candidate.retired_native != interpreter.retired_native
        || candidate.retired_interpreter != interpreter.retired_interpreter
    {
        return Err(err(format!(
            "case {} stored interpreter/eager/candidate retirement splits differ",
            input.index
        )));
    }
    let nz = |value| NonZeroU64::new(value).unwrap();
    let context = ExecutionContextSnapshot::native_register_v1_fixture(
        ExecutionContextId::from_u64(1).unwrap(),
    );
    let dependencies = [CodeDependencySpan {
        start_rip: input.guest_base_rip,
        end_rip: view.code_end(),
        identity: CodeIdentity {
            address_space: AddressSpaceId::new(nz(1)),
            mapping: MappingId::new(nz(1)),
            mapping_version: MappingVersion::new(nz(1)),
            backing: BackingId::new(nz(1)),
            backing_version: BackingVersion::new(nz(1)),
            executable: true,
        },
    }];
    Ok(PreparedCase {
        input,
        admission,
        context,
        dependencies,
        expected_state,
        expected_exit,
        expected_retired: retired,
        eager_retired_native: eager.retired_native,
        eager_retired_interpreter: eager.retired_interpreter,
        candidate_retired_native: candidate.retired_native,
        candidate_retired_interpreter: candidate.retired_interpreter,
        eager_cache: None,
        candidate_cache: None,
    })
}

fn validate_stored_observation(
    input: &Input,
    observation: &Observation,
    expected_state: &LongModeState,
    expected_exit: &str,
    retired: u64,
) -> Result<(), BenchError> {
    if observation.gpr != expected_state.gpr
        || observation.rip != expected_state.rip
        || observation.rflags != expected_state.rflags
        || observation.auxiliary_state != auxiliary_state(expected_state)
        || observation.exit != expected_exit
        || observation.retired_native + observation.retired_interpreter != retired
        || observation.remaining_budget != input.budget.saturating_sub(retired)
    {
        return Err(err(format!(
            "case {} stored observation differs from recomputed interpreter oracle",
            input.index
        )));
    }
    Ok(())
}

fn state_from_input(input: &Input) -> LongModeState {
    let mut state = LongModeState::default();
    state.gpr = input.initial_gpr;
    state.rip = input.initial_rip;
    state.rflags = input.initial_rflags;
    state
}

fn auxiliary_state(state: &LongModeState) -> String {
    format!(
        "fs_base={:#x};gs_base={:#x};fs_selector={:#x};gs_selector={:#x};gdtr={:?};fs_cache={:?};gs_cache={:?};x87={:?}",
        state.fs_base,
        state.gs_base,
        state.fs_selector,
        state.gs_selector,
        state.gdtr,
        state.fs_cache,
        state.gs_cache,
        state.x87
    )
}

fn default_auxiliary_state() -> String {
    auxiliary_state(&LongModeState::default())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
struct CodeMemory<'a> {
    base: u64,
    bytes: &'a [u8],
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl GuestMemory for CodeMemory<'_> {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        address
            .checked_sub(self.base)
            .and_then(|offset| self.bytes.get(offset as usize).copied())
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }
    fn read(&mut self, address: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }
    fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Write,
        })
    }
    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }
    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }
    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }
    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn dispatch(
    case: &PreparedCase,
    view: CodeView<'_>,
    candidate: bool,
    state: &mut LongModeState,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
    control: DispatchControl<'_>,
) -> Result<(FallbackDispatchResult, u128), BenchError> {
    let (result, elapsed_ns) = if candidate {
        let call_started = Instant::now();
        let result =
            od_jit::dispatch::run_cached_native_register_v1_demand_flags_candidate_controlled(
                view,
                &case.admission,
                case.context,
                &case.dependencies,
                state,
                case.input.budget,
                control,
                cache,
                &mut PublishedBlockExecutor,
            );
        (result, call_started.elapsed().as_nanos())
    } else {
        let call_started = Instant::now();
        let result = od_jit::dispatch::run_cached_native_register_v1_controlled(
            view,
            &case.admission,
            case.context,
            &case.dependencies,
            state,
            case.input.budget,
            control,
            cache,
            &mut PublishedBlockExecutor,
        );
        (result, call_started.elapsed().as_nanos())
    };
    let result = result.map_err(|e| err(format!("case {} dispatch: {e:?}", case.input.index)))?;
    Ok((result, elapsed_ns))
}

impl PreparedCase {
    fn input_state(&self) -> LongModeState {
        state_from_input(&self.input)
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn validate_result(
    case: &PreparedCase,
    candidate: bool,
    state: &LongModeState,
    exit: &DispatchExit,
    native: u64,
    interpreter: u64,
    remaining: u64,
) -> Result<(), BenchError> {
    let (expected_native, expected_interpreter) = if candidate {
        (
            case.candidate_retired_native,
            case.candidate_retired_interpreter,
        )
    } else {
        (case.eager_retired_native, case.eager_retired_interpreter)
    };
    let label = match exit {
        DispatchExit::CodeEnd => "CodeEnd",
        DispatchExit::BudgetExhausted => "BudgetExhausted",
        DispatchExit::Runtime(od_jit::runtime::RuntimeExitTag::StopAddress) => "StopAddress",
        DispatchExit::Runtime(od_jit::runtime::RuntimeExitTag::StopRequested) => "StopRequested",
        DispatchExit::EventPending => "EventPending",
        other => return Err(err(format!("unexpected dispatch exit {other:?}"))),
    };
    if *state != case.expected_state
        || label != case.expected_exit
        || native + interpreter != case.expected_retired
        || native != expected_native
        || interpreter != expected_interpreter
        || remaining != case.input.budget.saturating_sub(case.expected_retired)
    {
        return Err(err(format!(
            "case {} differs from interpreter: exit={label}, state_equal={}, retired={}/{}, remaining={}",
            case.input.index,
            *state == case.expected_state,
            native + interpreter,
            case.expected_retired,
            remaining
        )));
    }
    Ok(())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn snapshot(cache: &SingleEntryBlockCache<PublishedCode>) -> CacheSnapshot {
    let c = cache.counters();
    CacheSnapshot {
        counts: CacheCounts {
            hits: c.hits,
            misses: c.misses,
            compiles: c.successful_compiles,
            publications: c.successful_publications,
            replacements: c.replacements,
            published_code_bytes: c.published_code_bytes,
        },
        resident_entries: cache.resident_entries(),
        resident_code_bytes: cache.resident_code_bytes(),
        resident_logical_bytes: cache.resident_logical_bytes(),
        peak_resident_entries: c.resident_entries_high_water,
        max_resident_code_bytes: c.resident_code_bytes_high_water,
        max_resident_logical_bytes: c.resident_logical_metadata_bytes_high_water,
    }
}

fn counts(snapshot: CacheSnapshot) -> CacheCounts {
    snapshot.counts
}

fn validate_cache_bounds(snapshot: CacheSnapshot, input_index: u32) -> Result<(), BenchError> {
    if snapshot.resident_entries > 1
        || snapshot.resident_code_bytes > MAX_SINGLE_ENTRY_CACHE_BYTES
        || snapshot.resident_logical_bytes > MAX_SINGLE_ENTRY_CACHE_BYTES
    {
        return Err(err(format!(
            "case {input_index} exceeds the one-entry cache bounds: {snapshot:?}"
        )));
    }
    Ok(())
}

fn counter_delta(before: CacheSnapshot, after: CacheSnapshot) -> Result<CacheCounts, BenchError> {
    let delta = |old: u64, new: u64| {
        new.checked_sub(old)
            .ok_or_else(|| err("cache counter regressed"))
    };
    Ok(CacheCounts {
        hits: delta(before.counts.hits, after.counts.hits)?,
        misses: delta(before.counts.misses, after.counts.misses)?,
        compiles: delta(before.counts.compiles, after.counts.compiles)?,
        publications: delta(before.counts.publications, after.counts.publications)?,
        replacements: delta(before.counts.replacements, after.counts.replacements)?,
        published_code_bytes: delta(
            before.counts.published_code_bytes,
            after.counts.published_code_bytes,
        )?,
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn validate_strategy(case: &PreparedCase, candidate: bool) -> Result<CacheBehavior, BenchError> {
    let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES)
        .map_err(|e| err(format!("cache config: {e:?}")))?;
    let mut state = case.input_state();
    let view = CodeView::new(case.input.guest_base_rip, &case.input.code).unwrap();
    let first_stop = AtomicU8::new(u8::from(case.input.stop_requested));
    let first_control = control(case, &first_stop);
    let (result, _) = dispatch(case, view, candidate, &mut state, &mut cache, first_control)?;
    validate_result(
        case,
        candidate,
        &state,
        &result.exit,
        result.retired_native,
        result.retired_interpreter,
        result.remaining_budget,
    )?;
    let before = snapshot(&cache);
    validate_cache_bounds(before, case.input.index)?;
    state.clone_from(&case.input_state());
    let second_stop = AtomicU8::new(u8::from(case.input.stop_requested));
    let second_control = control(case, &second_stop);
    let (result, _) = dispatch(
        case,
        view,
        candidate,
        &mut state,
        &mut cache,
        second_control,
    )?;
    validate_result(
        case,
        candidate,
        &state,
        &result.exit,
        result.retired_native,
        result.retired_interpreter,
        result.remaining_budget,
    )?;
    let after = snapshot(&cache);
    validate_cache_bounds(after, case.input.index)?;
    Ok(CacheBehavior {
        cold: counts(before),
        repeat: counter_delta(before, after)?,
    })
}

fn control<'a>(case: &PreparedCase, stop: &'a AtomicU8) -> DispatchControl<'a> {
    DispatchControl {
        stop_rip: case.input.stop_rip,
        stop_requested: stop,
        event_requested: None,
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_mode(
    cases: &[PreparedCase],
    repeat: bool,
    started: Instant,
) -> Result<ModeReport, BenchError> {
    let calibration_eager =
        measure_sweep(cases, repeat, false, 1, 0, "calibration_eager", started)?;
    let calibration_candidate =
        measure_sweep(cases, repeat, true, 1, 0, "calibration_candidate", started)?;
    let mut sweeps = calls_for_target(
        calibration_eager
            .elapsed_ns
            .min(calibration_candidate.elapsed_ns),
    )?
    .max(1);
    let initial_sweeps = sweeps;
    let calibration = CalibrationEvidence {
        eager_sample: calibration_eager,
        candidate_sample: calibration_candidate,
        initial_sweeps_per_sample: initial_sweeps,
    };
    let mut restarts = Vec::new();
    if sweeps > MAX_SWEEPS_PER_SAMPLE {
        return Err(err("calibrated sweep count exceeds limit"));
    }
    for restart in 0..8 {
        let mut eager = Vec::with_capacity(SAMPLE_PAIRS);
        let mut candidate = Vec::with_capacity(SAMPLE_PAIRS);
        let mut too_short = None;
        for pair in 0..SAMPLE_PAIRS {
            if started.elapsed() > MAX_RUN_TIME {
                return Err(err("30 minute run limit exceeded"));
            }
            let eager_first = pair % 2 == 0;
            let (first_candidate, first_order, second_candidate, second_order) = if eager_first {
                (false, "eager_first", true, "candidate_second")
            } else {
                (true, "candidate_first", false, "eager_second")
            };
            let first = measure_sweep(
                cases,
                repeat,
                first_candidate,
                sweeps,
                pair,
                first_order,
                started,
            )?;
            let second = measure_sweep(
                cases,
                repeat,
                second_candidate,
                sweeps,
                pair,
                second_order,
                started,
            )?;
            if eager_first {
                eager.push(first.clone());
                candidate.push(second.clone());
            } else {
                candidate.push(first.clone());
                eager.push(second.clone());
            }
            let shortest = first.elapsed_ns.min(second.elapsed_ns);
            if shortest < MIN_SAMPLE.as_nanos() {
                too_short = Some((pair, shortest));
                break;
            }
        }
        if let Some((trigger_pair, shortest)) = too_short {
            if restart == 7 {
                return Err(err("could not reach 200 ms sample floor"));
            }
            let previous_sweeps = sweeps;
            let scaled = u128::from(sweeps)
                .checked_mul(MIN_SAMPLE.as_nanos())
                .ok_or_else(|| err("calibration overflow"))?
                .div_ceil(shortest)
                .saturating_mul(6)
                .div_ceil(5);
            sweeps = u64::try_from(scaled)
                .map_err(|_| err("calibration overflow"))?
                .max(sweeps + 1);
            if sweeps > MAX_SWEEPS_PER_SAMPLE {
                return Err(err("sample restart exceeds sweep limit"));
            }
            restarts.push(SampleRestart {
                attempt: restart + 1,
                trigger_pair,
                shortest_elapsed_ns: shortest,
                previous_sweeps_per_sample: previous_sweeps,
                next_sweeps_per_sample: sweeps,
                discarded_eager_samples: eager,
                discarded_candidate_samples: candidate,
            });
            continue;
        }
        let ratios = eager
            .iter()
            .zip(&candidate)
            .map(|(a, b)| {
                (b.elapsed_ns as f64 / b.cases as f64) / (a.elapsed_ns as f64 / a.cases as f64)
            })
            .collect();
        return Ok(ModeReport {
            sweeps_per_sample: sweeps,
            sample_pairs: SAMPLE_PAIRS,
            calibration,
            restarts,
            eager_preflight_counts_per_sweep: preflight_counts(cases, false, repeat)?,
            candidate_preflight_counts_per_sweep: preflight_counts(cases, true, repeat)?,
            repeat_dispatch_description: if repeat {
                "second complete dispatch with the same per-input one-entry cache retained from the first; exact hits, misses, compiles, publications, replacements and published code bytes are checked against preflight"
            } else {
                "first complete dispatch from a fresh per-input one-entry cache; exact cache counters are checked against preflight"
            },
            eager: summarize(eager),
            candidate: summarize(candidate),
            candidate_over_eager_ratios: ratios,
        });
    }
    Err(err("unreachable sample calibration"))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn calls_for_target(elapsed: u128) -> Result<u64, BenchError> {
    if elapsed == 0 {
        return Err(err("calibration duration is zero"));
    }
    let target = MIN_SAMPLE
        .as_nanos()
        .checked_mul(6)
        .ok_or_else(|| err("calibration overflow"))?;
    u64::try_from(target.div_ceil(elapsed)).map_err(|_| err("calibration overflow"))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_sweep(
    cases: &[PreparedCase],
    repeat: bool,
    candidate: bool,
    sweeps: u64,
    pair: usize,
    order: &'static str,
    run_started: Instant,
) -> Result<Sample, BenchError> {
    let mut totals = Totals {
        min_final_resident_code_bytes: usize::MAX,
        ..Totals::default()
    };
    for _ in 0..sweeps {
        for case in cases {
            check_deadline(run_started)?;
            let behavior = cache_behavior(case, candidate)?;
            let mut cache = SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES)
                .map_err(|e| err(format!("cache config: {e:?}")))?;
            let view = CodeView::new(case.input.guest_base_rip, &case.input.code).unwrap();
            let mut state = case.input_state();
            if repeat {
                let prewarm_stop = AtomicU8::new(u8::from(case.input.stop_requested));
                let prewarm_control = control(case, &prewarm_stop);
                let (prewarm, _) = dispatch(
                    case,
                    view,
                    candidate,
                    &mut state,
                    &mut cache,
                    prewarm_control,
                )?;
                validate_result(
                    case,
                    candidate,
                    &state,
                    &prewarm.exit,
                    prewarm.retired_native,
                    prewarm.retired_interpreter,
                    prewarm.remaining_budget,
                )?;
                let prewarm_counts = counts(snapshot(&cache));
                if prewarm_counts != behavior.cold {
                    return Err(err(format!(
                        "case {} prewarm differs from frozen cold counters: {prewarm_counts:?} != {:?}",
                        case.input.index, behavior.cold
                    )));
                }
            }
            let before = snapshot(&cache);
            validate_cache_bounds(before, case.input.index)?;
            state.clone_from(&case.input_state());
            let timed_stop = AtomicU8::new(u8::from(case.input.stop_requested));
            let timed_control = control(case, &timed_stop);
            check_deadline(run_started)?;
            let (result, elapsed_ns) =
                dispatch(case, view, candidate, &mut state, &mut cache, timed_control)?;
            totals.elapsed_ns += elapsed_ns;
            validate_result(
                case,
                candidate,
                &state,
                &result.exit,
                result.retired_native,
                result.retired_interpreter,
                result.remaining_budget,
            )?;
            let after = snapshot(&cache);
            validate_cache_bounds(after, case.input.index)?;
            let actual = if repeat {
                counter_delta(before, after)?
            } else {
                counts(after)
            };
            let expected = if repeat {
                behavior.repeat
            } else {
                behavior.cold
            };
            if actual != expected {
                return Err(err(format!(
                    "case {} {} counters differ from preflight: {actual:?} != {expected:?}",
                    case.input.index,
                    if repeat { "repeat" } else { "cold" }
                )));
            }
            totals.hits += actual.hits;
            totals.misses += actual.misses;
            totals.compiles += actual.compiles;
            totals.publications += actual.publications;
            totals.replacements += actual.replacements;
            totals.published_code_bytes_total = totals
                .published_code_bytes_total
                .checked_add(actual.published_code_bytes)
                .ok_or_else(|| err("published code byte total overflow"))?;
            totals.code_bytes_high_water = totals
                .code_bytes_high_water
                .max(after.max_resident_code_bytes);
            totals.logical_bytes_high_water = totals
                .logical_bytes_high_water
                .max(after.max_resident_logical_bytes);
            totals.peak_resident_entries = totals
                .peak_resident_entries
                .max(after.peak_resident_entries);
            let code_bytes = after.resident_code_bytes;
            totals.final_resident_code_bytes += code_bytes as u128;
            totals.min_final_resident_code_bytes =
                totals.min_final_resident_code_bytes.min(code_bytes);
            totals.max_final_resident_code_bytes =
                totals.max_final_resident_code_bytes.max(code_bytes);
            totals.native_retired += result.retired_native;
            totals.interpreter_retired += result.retired_interpreter;
            totals.blocks_executed += result.blocks_executed;
            drop(cache);
        }
    }
    let cases_count = sweeps
        .checked_mul(cases.len() as u64)
        .ok_or_else(|| err("work count overflow"))?;
    let expected = scale_counts(preflight_counts(cases, candidate, repeat)?, sweeps)?;
    let observed = CacheCounters {
        hits: totals.hits,
        misses: totals.misses,
        compiles: totals.compiles,
        publications: totals.publications,
        replacements: totals.replacements,
        published_code_bytes_total: totals.published_code_bytes_total,
        peak_resident_entries: totals.peak_resident_entries,
        max_resident_code_bytes: totals.code_bytes_high_water,
        max_resident_logical_bytes: totals.logical_bytes_high_water,
        final_resident_code_bytes_per_sweep: totals.final_resident_code_bytes / u128::from(sweeps),
        min_final_resident_code_bytes: if totals.min_final_resident_code_bytes == usize::MAX {
            0
        } else {
            totals.min_final_resident_code_bytes
        },
        max_final_resident_code_bytes: totals.max_final_resident_code_bytes,
    };
    let observed_counts = CacheCounts {
        hits: observed.hits,
        misses: observed.misses,
        compiles: observed.compiles,
        publications: observed.publications,
        replacements: observed.replacements,
        published_code_bytes: observed.published_code_bytes_total,
    };
    if observed_counts != expected {
        return Err(err(format!(
            "aggregate cache counters mismatch: {observed:?} expected {expected:?}"
        )));
    }
    Ok(Sample {
        pair,
        order,
        sweeps,
        cases: cases_count,
        elapsed_ns: totals.elapsed_ns,
        native_retired: totals.native_retired,
        interpreter_retired: totals.interpreter_retired,
        blocks_executed: totals.blocks_executed,
        counters: observed,
    })
}

fn cache_behavior(case: &PreparedCase, candidate: bool) -> Result<CacheBehavior, BenchError> {
    if candidate {
        case.candidate_cache
    } else {
        case.eager_cache
    }
    .ok_or_else(|| {
        err(format!(
            "case {} is missing cache preflight",
            case.input.index
        ))
    })
}

fn cache_baselines(cases: &[PreparedCase]) -> Result<Vec<CaseCacheBaseline>, BenchError> {
    cases
        .iter()
        .map(|case| {
            Ok(CaseCacheBaseline {
                seed: case.input.seed,
                index: case.input.index,
                eager: cache_behavior(case, false)?,
                candidate: cache_behavior(case, true)?,
            })
        })
        .collect()
}

fn preflight_counts(
    cases: &[PreparedCase],
    candidate: bool,
    repeat: bool,
) -> Result<CacheCounts, BenchError> {
    let mut total = CacheCounts::default();
    for case in cases {
        let behavior = cache_behavior(case, candidate)?;
        let counts = if repeat {
            behavior.repeat
        } else {
            behavior.cold
        };
        total.hits = total
            .hits
            .checked_add(counts.hits)
            .ok_or_else(|| err("hit count overflow"))?;
        total.misses = total
            .misses
            .checked_add(counts.misses)
            .ok_or_else(|| err("miss count overflow"))?;
        total.compiles = total
            .compiles
            .checked_add(counts.compiles)
            .ok_or_else(|| err("compile count overflow"))?;
        total.publications = total
            .publications
            .checked_add(counts.publications)
            .ok_or_else(|| err("publication count overflow"))?;
        total.replacements = total
            .replacements
            .checked_add(counts.replacements)
            .ok_or_else(|| err("replacement count overflow"))?;
        total.published_code_bytes = total
            .published_code_bytes
            .checked_add(counts.published_code_bytes)
            .ok_or_else(|| err("published code byte total overflow"))?;
    }
    Ok(total)
}

fn scale_counts(counts: CacheCounts, multiplier: u64) -> Result<CacheCounts, BenchError> {
    let scale = |value: u64| {
        value
            .checked_mul(multiplier)
            .ok_or_else(|| err("cache count scaling overflow"))
    };
    Ok(CacheCounts {
        hits: scale(counts.hits)?,
        misses: scale(counts.misses)?,
        compiles: scale(counts.compiles)?,
        publications: scale(counts.publications)?,
        replacements: scale(counts.replacements)?,
        published_code_bytes: scale(counts.published_code_bytes)?,
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn check_deadline(started: Instant) -> Result<(), BenchError> {
    if started.elapsed() > MAX_RUN_TIME {
        Err(err("30 minute run limit exceeded during corpus sweep"))
    } else {
        Ok(())
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn summarize(samples: Vec<Sample>) -> VariantReport {
    let mut values = samples
        .iter()
        .map(|sample| sample.elapsed_ns / u128::from(sample.cases))
        .collect::<Vec<_>>();
    values.sort_unstable();
    let peak_resident_entries = samples
        .iter()
        .map(|sample| sample.counters.peak_resident_entries)
        .max()
        .unwrap_or(0);
    VariantReport {
        min_ns_per_case: *values.first().unwrap_or(&0),
        max_ns_per_case: *values.last().unwrap_or(&0),
        median_ns_per_case: percentile(&values, 50),
        iqr_ns_per_case: percentile(&values, 75).saturating_sub(percentile(&values, 25)),
        peak_resident_entries,
        samples,
    }
}

fn percentile(values: &[u128], percentile: usize) -> u128 {
    if values.is_empty() {
        return 0;
    }
    values[(values.len() - 1) * percentile / 100]
}

fn source_fingerprints() -> Vec<SourceFingerprint> {
    [
        ("tools/bench/src/cached_native_flags_compare.rs", SOURCE),
        ("tools/bench/src/main.rs", CLI_SOURCE),
        ("tools/bench/src/lib.rs", LIB_SOURCE),
        ("tools/bench/Cargo.toml", BENCH_MANIFEST_SOURCE),
        ("crates/od-jit/Cargo.toml", JIT_MANIFEST_SOURCE),
        ("crates/od-jit/src/dispatch.rs", DISPATCH_SOURCE),
        ("crates/od-jit/src/block_compile.rs", COMPILER_SOURCE),
        ("crates/od-jit/src/block.rs", BLOCK_SOURCE),
        ("crates/od-jit/src/block_emitter.rs", EMITTER_SOURCE),
        ("crates/od-jit/src/translation.rs", TRANSLATION_SOURCE),
        ("crates/od-jit/src/statepoints.rs", STATEPOINT_SOURCE),
        ("crates/od-host/src/lib.rs", HOST_SOURCE),
        ("crates/od-isa/src/execute.rs", ISA_SOURCE),
        ("crates/od-isa/src/long_decode.rs", DECODE_SOURCE),
        ("crates/od-isa/src/state.rs", STATE_SOURCE),
        (
            "crates/od-host/src/tests/native_register_v1_corpus.rs",
            CORPUS_SOURCE,
        ),
    ]
    .into_iter()
    .map(|(path, bytes)| SourceFingerprint {
        path,
        sha256: od_testkit::sha256_hex(bytes),
    })
    .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibration_targets_both_sides_above_sample_floor() {
        let elapsed = 100_000_000u128;
        let sweeps = calls_for_target(elapsed).unwrap();
        assert!(sweeps > 1);
        assert!(sweeps <= MAX_SWEEPS_PER_SAMPLE);
    }

    #[test]
    fn percentile_uses_lower_order_statistic() {
        assert_eq!(percentile(&[10, 20, 30, 40], 50), 20);
        assert_eq!(percentile(&[], 50), 0);
    }

    #[test]
    fn cache_accounting_keeps_miss_and_hit_phases_distinct() {
        let cold = CacheCounts {
            hits: 2,
            misses: 5,
            compiles: 3,
            publications: 3,
            replacements: 2,
            published_code_bytes: 420,
        };
        assert_eq!(
            scale_counts(cold, 2).unwrap(),
            CacheCounts {
                hits: 4,
                misses: 10,
                compiles: 6,
                publications: 6,
                replacements: 4,
                published_code_bytes: 840,
            }
        );
    }

    #[test]
    fn repeat_delta_preserves_replacement_and_published_byte_counts() {
        let before = CacheSnapshot {
            counts: CacheCounts {
                misses: 2,
                compiles: 2,
                publications: 2,
                replacements: 1,
                published_code_bytes: 96,
                ..CacheCounts::default()
            },
            ..CacheSnapshot::default()
        };
        let after = CacheSnapshot {
            counts: CacheCounts {
                hits: 3,
                misses: 4,
                compiles: 3,
                publications: 3,
                replacements: 2,
                published_code_bytes: 144,
            },
            ..CacheSnapshot::default()
        };
        assert_eq!(
            counter_delta(before, after).unwrap(),
            CacheCounts {
                hits: 3,
                misses: 2,
                compiles: 1,
                publications: 1,
                replacements: 1,
                published_code_bytes: 48,
            }
        );
    }
}
