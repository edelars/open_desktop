use super::*;
use od_isa::memory::{AccessContext, AccessKind, GuestMemory};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

const PROFILE: &str = "NativeRegisterV1";
const ENVELOPE_VERSION: u32 = 1;
const CACHE_TRACE_VERSION: u32 = 1;
const RETAINED_HIT_REDUCTION_VERSION: u32 = 1;
const MAX_BUDGET: u64 = 16;
const SEEDS: [u64; 3] = [
    0x5333_346e_0000_0001,
    0x5333_346e_0000_0002,
    0x5333_346e_0000_0003,
];
const CASES_PER_SEED: usize = 400;
const REDUCER_MAX_EVALUATIONS: usize = 256;
const REDUCER_FLAG_MASK: u64 = 0x0cd5; // arithmetic flags plus DF
const REPLAY_GATE: &str = "tests::native_register_v1_corpus::saved_case_round_trip_and_replay";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackendRuns {
    interpreter: [Option<Observation>; 2],
    eager_native: [Option<Observation>; 2],
    demand_candidate: [Option<Observation>; 2],
    errors: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusObservation {
    input: Input,
    backend_runs: BackendRuns,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusObservationsEnvelope {
    version: u32,
    profile: String,
    inputs_digest: String,
    cases: Vec<CorpusObservation>,
    envelope_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheBindings {
    execution_context_id: u64,
    address_space_id: u64,
    mapping_id: u64,
    mapping_version: u64,
    backing_id: u64,
    backing_version: u64,
    dependency_start_rip: u64,
    dependency_end_rip: u64,
    code_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheCounterSnapshot {
    hits: u64,
    misses: u64,
    successful_compiles: u64,
    successful_publications: u64,
    replacements: u64,
    resident_entries_high_water: usize,
    resident_code_bytes_high_water: usize,
    resident_logical_metadata_bytes_high_water: usize,
    resident_entries: usize,
    resident_code_bytes: usize,
    resident_logical_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheTraceStep {
    sequence_index: usize,
    input: Input,
    bindings: CacheBindings,
    interpreter: Observation,
    uncached: Observation,
    cached: Observation,
    cache: CacheCounterSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheTraceSegment {
    kind: String,
    strategy: String,
    steps: Vec<CacheTraceStep>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheTraceEnvelope {
    version: u32,
    profile: String,
    source_inputs_digest: String,
    segments: Vec<CacheTraceSegment>,
    trace_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheTraceFailureEnvelope {
    version: u32,
    profile: String,
    source_inputs_digest: String,
    segment_kind: String,
    strategy: String,
    sequence_index: usize,
    successful_prefix: Vec<CacheTraceStep>,
    failing_input: Input,
    interpreter: Observation,
    uncached: Observation,
    cached: Option<Observation>,
    cache_before: CacheCounterSnapshot,
    cache_after: CacheCounterSnapshot,
    failure_kind: String,
    failure_detail: String,
    reproduction_command: String,
    failure_digest: String,
}

#[derive(Serialize)]
struct CacheTraceFailurePayload<'a> {
    version: u32,
    profile: &'a str,
    source_inputs_digest: &'a str,
    segment_kind: &'a str,
    strategy: &'a str,
    sequence_index: usize,
    successful_prefix: &'a [CacheTraceStep],
    failing_input: &'a Input,
    interpreter: &'a Observation,
    uncached: &'a Observation,
    cached: &'a Option<Observation>,
    cache_before: &'a CacheCounterSnapshot,
    cache_after: &'a CacheCounterSnapshot,
    failure_kind: &'a str,
    failure_detail: &'a str,
    reproduction_command: &'a str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedHitReductionArtifact {
    version: u32,
    profile: String,
    source_inputs_digest: String,
    source_failure_file_digest: String,
    source_failure_digest: String,
    source_failure_path: String,
    strategy: String,
    warmup: CacheTraceStep,
    original_failing_input: Input,
    reduced_failing_input: Input,
    mismatch_signature: String,
    original_state_metric: u32,
    reduced_state_metric: u32,
    original_replay_runs: u8,
    reduced_replay_runs: u8,
    artifact_digest: String,
}

#[derive(Serialize)]
struct RetainedHitReductionPayload<'a> {
    version: u32,
    profile: &'a str,
    source_inputs_digest: &'a str,
    source_failure_file_digest: &'a str,
    source_failure_digest: &'a str,
    source_failure_path: &'a str,
    strategy: &'a str,
    warmup: &'a CacheTraceStep,
    original_failing_input: &'a Input,
    reduced_failing_input: &'a Input,
    mismatch_signature: &'a str,
    original_state_metric: u32,
    reduced_state_metric: u32,
    original_replay_runs: u8,
    reduced_replay_runs: u8,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedHitReductionStatus {
    version: u32,
    profile: String,
    source_failure_digest: String,
    outcome: String,
    evaluations: usize,
    reduced_artifact: Option<String>,
    status_digest: String,
}

#[derive(Serialize)]
struct RetainedHitReductionStatusPayload<'a> {
    version: u32,
    profile: &'a str,
    source_failure_digest: &'a str,
    outcome: &'a str,
    evaluations: usize,
    reduced_artifact: &'a Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedCase {
    input: Input,
    backend_runs: BackendRuns,
    failure_signature: Option<Vec<String>>,
    reproduction_command: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReductionAttempt {
    digest: String,
    metric_before: u64,
    metric_after: u64,
    accepted: bool,
    reason: String,
    signature: Vec<String>,
    backend_runs: BackendRuns,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReductionReport {
    kind: String,
    source_path: String,
    source_digest: String,
    source_signature: Vec<String>,
    source_replay_signature: Vec<String>,
    source_backend_runs: BackendRuns,
    source_replay_command: String,
    reduced_digest: Option<String>,
    reduced_input: Option<Input>,
    reduced_signature: Option<Vec<String>>,
    reduced_backend_runs: Option<BackendRuns>,
    reduced_replay_command: Option<String>,
    metric_definition: String,
    metric_before: u64,
    metric_after: Option<u64>,
    evaluated_count: usize,
    accepted_count: usize,
    rejected_count: usize,
    stop_reason: String,
    attempts: Vec<ReductionAttempt>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Reference {
    state: od_isa::LongModeState,
    exit: String,
    retired_native: u64,
    retired_interpreter: u64,
    remaining_budget: u64,
}

struct CodeOnly<'a> {
    base: u64,
    bytes: &'a [u8],
}

impl GuestMemory for CodeOnly<'_> {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        address
            .checked_sub(self.base)
            .and_then(|offset| self.bytes.get(offset as usize))
            .copied()
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

#[derive(Clone, Copy)]
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }
}

fn digest(bytes: &[u8]) -> String {
    // A stable, version-local digest over canonical JSON input bytes. Uniqueness
    // is separately checked against those complete canonical bytes.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3);
    }
    format!("fnv1a64:{hash:016x}")
}

fn canonical_input(input: &Input) -> Vec<u8> {
    #[derive(Serialize)]
    struct CompleteInput<'a> {
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
    serde_json::to_vec(&CompleteInput {
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
    .expect("input serialization")
}

fn finish_input(mut input: Input) -> Input {
    input.input_digest = digest(&canonical_input(&input));
    input
}

fn validate_input(input: &Input) -> Result<(), String> {
    if input.version != ENVELOPE_VERSION || input.profile != PROFILE {
        return Err("unsupported envelope version/profile".into());
    }
    if input.initial_rip != input.guest_base_rip {
        return Err("entry RIP differs from guest base".into());
    }
    if input.initial_auxiliary_state != default_auxiliary_state() {
        return Err("unsupported non-default auxiliary initial state".into());
    }
    if input.code.is_empty() || input.code.len() > 96 {
        return Err("invalid code length".into());
    }
    if input.budget > MAX_BUDGET {
        return Err("instruction budget exceeds bounded replay limit".into());
    }
    if input.input_digest != digest(&canonical_input(input)) {
        return Err("input digest mismatch".into());
    }
    let allowed = od_jit::block::CodeView::new(input.guest_base_rip, &input.code)
        .map_err(|_| "malformed code view")?;
    let end = allowed.code_end();
    let _admission = od_jit::block::NativeRegisterV1Admission::from_code(allowed)
        .map_err(|_| "unsupported instruction form")?;
    if end != input.guest_base_rip + input.code.len() as u64 {
        return Err("admission end mismatch".into());
    }
    Ok(())
}

fn generate_case(seed: u64, index: usize) -> Input {
    let mut random = Random(seed ^ (index as u64).wrapping_mul(0xd1b5_4a32_d192_ed03));
    let base = 0x0000_0001_0000_0000u64 + (random.next() & 0x0000_0000_ffff_f000);
    let mut code = Vec::with_capacity(64);
    let form_count = 2 + (random.next() as usize % 2);
    for position in 0..form_count {
        let form = if position == 0 {
            index % 13
        } else if position == 1 {
            // Guarantees RIP-relative LEA follows a predecessor across all
            // variable lengths selected by the first-form rotation.
            12
        } else {
            (index + position * 5 + random.next() as usize) % 13
        };
        match form {
            0 => code.extend_from_slice(&[0x48, 0xff, 0xc0]),
            1 => code.extend_from_slice(&[0x48, 0xff, 0xc8]),
            2 => code.extend_from_slice(&[0x48, 0xf7, 0xd8]),
            3 => code.extend_from_slice(&[0x48, 0xf7, 0xd0]),
            4 => code.extend_from_slice(&[0x48, 0x83, 0xc0, random.next() as u8]),
            5 => code.extend_from_slice(&[0x48, 0x83, 0xf8, random.next() as u8]),
            6 => code.extend_from_slice(&[0x48, 0x83, 0xe8, random.next() as u8]),
            7 => code.extend_from_slice(&[
                0x48,
                0xc7,
                0xc0,
                random.next() as u8,
                random.next() as u8,
                random.next() as u8,
                random.next() as u8,
            ]),
            8 => code.extend_from_slice(&[0x48, 0x87, 0xd8]),
            9 => code.extend_from_slice(&[0x48, 0x8d, 0x03]),
            10 => code.extend_from_slice(&[0x48, 0x8d, 0x43, random.next() as u8]),
            11 => code.extend_from_slice(&[0x48, 0x8d, 0x04, 0x0b]),
            _ => {
                // Place RIP-relative LEA after a predecessor whose length varies.
                if position == 0 {
                    code.extend_from_slice(&[0x48, 0xff, 0xc0]);
                }
                code.extend_from_slice(&[0x48, 0x8d, 0x05]);
                code.extend_from_slice(&(random.next() as i32).to_le_bytes());
            }
        }
    }
    if index % 3 == 0 {
        code.push(0x90);
    }
    // JZ skips a distinct MOV; JNZ then reaches the code-end boundary.
    code.extend_from_slice(&[0x74, 0x0a, 0x48, 0xb8]);
    code.extend_from_slice(&random.next().to_le_bytes());
    code.extend_from_slice(&[0x75, 0x00]);
    // Every eighth case uses a bounded self-loop controlled by budget.
    if index % 8 == 0 {
        code.extend_from_slice(&[0x75, 0xfe]);
    }

    let mut initial_gpr = [0; 16];
    for (register, value) in initial_gpr.iter_mut().enumerate() {
        *value = random.next() ^ ((register as u64) << 52);
    }
    if index == 0 {
        // Preserve the full -> partial -> preserving -> XCHG -> branch
        // composition as a named bounded anchor. Generated position-1 LEAs
        // separately exercise RIP-relative LEA after variable-length forms.
        code.clear();
        code.extend_from_slice(&[0x48, 0x83, 0xc0, 0x01]); // ADD full writer
        code.extend_from_slice(&[0x48, 0xff, 0xc8]); // DEC partial writer
        code.extend_from_slice(&[0x48, 0x8d, 0x04, 0x0b]); // LEA preserves flags
        code.extend_from_slice(&[0x48, 0x87, 0xd8]); // XCHG preserves flags
        code.extend_from_slice(&[0x74, 0x0a, 0x48, 0xb8]);
        code.extend_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes());
        code.extend_from_slice(&[0x75, 0x00]);
        initial_gpr[0] = if seed % 3 == 0 { u64::MAX } else { 0 };
        initial_gpr[3] = 0;
        initial_gpr[1] = 0x0102_0304_0506_0708;
    }
    let initial_rflags = 0x202 | (random.next() & 0x8d5);
    let budget = if index == 0 {
        MAX_BUDGET
    } else {
        random.next() % (MAX_BUDGET + 1)
    };
    let stop_requested = index != 0 && index % 11 == 0;
    let mut stop_rip = None;
    if index != 0 && !stop_requested && index % 5 == 0 {
        // Entry STOP is valid and checks the zero-retirement boundary.
        stop_rip = Some(base);
    } else if index != 0 && !stop_requested && index % 5 == 1 {
        // A code-end stop checks post-instruction boundary ordering.
        stop_rip = Some(base + code.len() as u64);
    }
    finish_input(Input {
        version: ENVELOPE_VERSION,
        profile: PROFILE.into(),
        seed,
        index: index as u32,
        code,
        guest_base_rip: base,
        initial_gpr,
        initial_rip: base,
        initial_rflags,
        initial_auxiliary_state: default_auxiliary_state(),
        stop_rip,
        stop_requested,
        budget,
        input_digest: String::new(),
    })
}

fn state_from_input(input: &Input) -> od_isa::LongModeState {
    let mut state = od_isa::LongModeState::default();
    state.gpr = input.initial_gpr;
    state.rip = input.initial_rip;
    state.rflags = input.initial_rflags;
    state
}

fn auxiliary_state(state: &od_isa::LongModeState) -> String {
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
    auxiliary_state(&od_isa::LongModeState::default())
}

fn observation(
    state: &od_isa::LongModeState,
    exit: &str,
    native: u64,
    interpreter: u64,
    remaining: u64,
) -> Observation {
    Observation {
        gpr: state.gpr,
        rip: state.rip,
        rflags: state.rflags,
        auxiliary_state: auxiliary_state(state),
        exit: exit.into(),
        retired_native: native,
        retired_interpreter: interpreter,
        remaining_budget: remaining,
    }
}

fn reference(input: &Input) -> Reference {
    validate_input(input).unwrap_or_else(|error| panic!("invalid generated input: {error}"));
    let view = od_jit::block::CodeView::new(input.guest_base_rip, &input.code).unwrap();
    let end = view.code_end();
    let stop = AtomicU8::new(u8::from(input.stop_requested));
    let mut state = state_from_input(input);
    let mut memory = CodeOnly {
        base: input.guest_base_rip,
        bytes: &input.code,
    };
    let mut native = 0;
    let mut interpreter = 0;
    let mut retired = 0;
    let exit = loop {
        if input.stop_rip == Some(state.rip) {
            break "StopAddress";
        }
        if stop.load(std::sync::atomic::Ordering::Acquire) != 0 {
            break "StopRequested";
        }
        if state.rip == end {
            break "CodeEnd";
        }
        if retired >= input.budget {
            break "BudgetExhausted";
        }
        let offset = (state.rip - input.guest_base_rip) as usize;
        let fallback = input.code.get(offset) == Some(&0x90);
        match od_isa::execute::step(&mut state, &mut memory, AccessContext::default()).unwrap() {
            od_isa::execute::StepOutcome::Advanced | od_isa::execute::StepOutcome::Branched => {}
            other => panic!("unexpected interpreter outcome {other:?}"),
        }
        if fallback {
            interpreter += 1;
        } else {
            native += 1;
        }
        retired += 1;
    };
    Reference {
        state,
        exit: exit.into(),
        retired_native: native,
        retired_interpreter: interpreter,
        remaining_budget: input.budget.saturating_sub(retired),
    }
}

fn run_backend(input: &Input, candidate: bool) -> Result<Observation, String> {
    validate_input(input).unwrap_or_else(|error| panic!("invalid input: {error}"));
    let view = od_jit::block::CodeView::new(input.guest_base_rip, &input.code).unwrap();
    let admission = od_jit::block::NativeRegisterV1Admission::from_code(view).unwrap();
    let stop = AtomicU8::new(u8::from(input.stop_requested));
    let control = od_jit::dispatch::DispatchControl {
        stop_rip: input.stop_rip,
        stop_requested: &stop,
        event_requested: None,
    };
    let mut state = state_from_input(input);
    let result = if candidate {
        od_jit::dispatch::run_with_native_register_v1_demand_flags_candidate(
            view,
            &admission,
            &mut state,
            input.budget,
            control,
            &mut PublishedBlockExecutor,
        )
    } else {
        od_jit::dispatch::run_with_native_register_v1(
            view,
            &admission,
            &mut state,
            input.budget,
            control,
            &mut PublishedBlockExecutor,
        )
    }
    .map_err(|error| format!("backend error candidate={candidate}: {error:?}"))?;
    let label = match result.exit {
        od_jit::dispatch::DispatchExit::CodeEnd => "CodeEnd",
        od_jit::dispatch::DispatchExit::BudgetExhausted => "BudgetExhausted",
        od_jit::dispatch::DispatchExit::EventPending => "EventPending",
        od_jit::dispatch::DispatchExit::Runtime(od_jit::runtime::RuntimeExitTag::StopAddress) => {
            "StopAddress"
        }
        od_jit::dispatch::DispatchExit::Runtime(od_jit::runtime::RuntimeExitTag::StopRequested) => {
            "StopRequested"
        }
        other => panic!("unexpected backend exit {other:?}"),
    };
    Ok(observation(
        &state,
        label,
        result.retired_native,
        result.retired_interpreter,
        result.remaining_budget,
    ))
}

fn cache_bindings(
    input: &Input,
) -> (
    od_jit::translation::ExecutionContextSnapshot,
    [od_jit::translation::CodeDependencySpan; 1],
    CacheBindings,
) {
    use od_jit::translation::{
        AddressSpaceId, BackingId, BackingVersion, CodeDependencySpan, CodeIdentity,
        ExecutionContextId, ExecutionContextSnapshot, MappingId, MappingVersion,
    };
    use std::num::NonZeroU64;

    let nz = |value| NonZeroU64::new(value).expect("cache identity is non-zero");
    let execution_context_id = 1;
    let address_space_id = 1;
    let mapping_id = 1;
    let mapping_version = 1;
    let backing_id = 1;
    let backing_version = 1;
    let context = ExecutionContextSnapshot::native_register_v1_fixture(
        ExecutionContextId::from_u64(execution_context_id).expect("valid context id"),
    );
    let dependencies = [CodeDependencySpan {
        start_rip: input.guest_base_rip,
        end_rip: input.guest_base_rip + input.code.len() as u64,
        identity: CodeIdentity {
            address_space: AddressSpaceId::new(nz(address_space_id)),
            mapping: MappingId::new(nz(mapping_id)),
            mapping_version: MappingVersion::new(nz(mapping_version)),
            backing: BackingId::new(nz(backing_id)),
            backing_version: BackingVersion::new(nz(backing_version)),
            executable: true,
        },
    }];
    let bindings = CacheBindings {
        execution_context_id,
        address_space_id,
        mapping_id,
        mapping_version,
        backing_id,
        backing_version,
        dependency_start_rip: dependencies[0].start_rip,
        dependency_end_rip: dependencies[0].end_rip,
        code_digest: digest(&input.code),
    };
    (context, dependencies, bindings)
}

fn cache_counter_snapshot(
    cache: &od_jit::dispatch::SingleEntryBlockCache<PublishedCode>,
) -> CacheCounterSnapshot {
    let counters = cache.counters();
    CacheCounterSnapshot {
        hits: counters.hits,
        misses: counters.misses,
        successful_compiles: counters.successful_compiles,
        successful_publications: counters.successful_publications,
        replacements: counters.replacements,
        resident_entries_high_water: counters.resident_entries_high_water,
        resident_code_bytes_high_water: counters.resident_code_bytes_high_water,
        resident_logical_metadata_bytes_high_water: counters
            .resident_logical_metadata_bytes_high_water,
        resident_entries: cache.resident_entries(),
        resident_code_bytes: cache.resident_code_bytes(),
        resident_logical_bytes: cache.resident_logical_bytes(),
    }
}

fn run_cached_backend(
    input: &Input,
    candidate: bool,
    cache: &mut od_jit::dispatch::SingleEntryBlockCache<PublishedCode>,
) -> Result<(Observation, CacheCounterSnapshot), String> {
    validate_input(input).map_err(|error| format!("invalid cached input: {error}"))?;
    let view = od_jit::block::CodeView::new(input.guest_base_rip, &input.code).unwrap();
    let admission = od_jit::block::NativeRegisterV1Admission::from_code(view).unwrap();
    let (context, dependencies, _) = cache_bindings(input);
    let stop = AtomicU8::new(u8::from(input.stop_requested));
    let control = od_jit::dispatch::DispatchControl {
        stop_rip: input.stop_rip,
        stop_requested: &stop,
        event_requested: None,
    };
    let mut state = state_from_input(input);
    let result = if candidate {
        od_jit::dispatch::run_cached_native_register_v1_demand_flags_candidate_controlled(
            view,
            &admission,
            context,
            &dependencies,
            &mut state,
            input.budget,
            control,
            cache,
            &mut PublishedBlockExecutor,
        )
    } else {
        od_jit::dispatch::run_cached_native_register_v1_controlled(
            view,
            &admission,
            context,
            &dependencies,
            &mut state,
            input.budget,
            control,
            cache,
            &mut PublishedBlockExecutor,
        )
    }
    .map_err(|error| format!("cached backend error candidate={candidate}: {error:?}"))?;
    let label = match result.exit {
        od_jit::dispatch::DispatchExit::CodeEnd => "CodeEnd",
        od_jit::dispatch::DispatchExit::BudgetExhausted => "BudgetExhausted",
        od_jit::dispatch::DispatchExit::EventPending => "EventPending",
        od_jit::dispatch::DispatchExit::Runtime(od_jit::runtime::RuntimeExitTag::StopAddress) => {
            "StopAddress"
        }
        od_jit::dispatch::DispatchExit::Runtime(od_jit::runtime::RuntimeExitTag::StopRequested) => {
            "StopRequested"
        }
        other => return Err(format!("unexpected cached backend exit {other:?}")),
    };
    let actual = observation(
        &state,
        label,
        result.retired_native,
        result.retired_interpreter,
        result.remaining_budget,
    );
    let counters = cache_counter_snapshot(cache);
    if counters.resident_code_bytes > cache.max_bytes() {
        return Err("cached code exceeded configured byte capacity".into());
    }
    Ok((actual, counters))
}

#[derive(Serialize)]
struct CorpusObservationsPayload<'a> {
    version: u32,
    profile: &'a str,
    inputs_digest: &'a str,
    cases: &'a [CorpusObservation],
}

fn corpus_observations_digest(
    version: u32,
    profile: &str,
    inputs_digest: &str,
    cases: &[CorpusObservation],
) -> String {
    let bytes = serde_json::to_vec(&CorpusObservationsPayload {
        version,
        profile,
        inputs_digest,
        cases,
    })
    .expect("corpus observation serialization");
    digest(&bytes)
}

#[derive(Serialize)]
struct CacheTracePayload<'a> {
    version: u32,
    profile: &'a str,
    source_inputs_digest: &'a str,
    segments: &'a [CacheTraceSegment],
}

fn cache_trace_digest(trace: &CacheTraceEnvelope) -> String {
    let bytes = serde_json::to_vec(&CacheTracePayload {
        version: trace.version,
        profile: &trace.profile,
        source_inputs_digest: &trace.source_inputs_digest,
        segments: &trace.segments,
    })
    .expect("cache trace serialization");
    digest(&bytes)
}

fn cache_trace_failure_digest(failure: &CacheTraceFailureEnvelope) -> String {
    let bytes = serde_json::to_vec(&CacheTraceFailurePayload {
        version: failure.version,
        profile: &failure.profile,
        source_inputs_digest: &failure.source_inputs_digest,
        segment_kind: &failure.segment_kind,
        strategy: &failure.strategy,
        sequence_index: failure.sequence_index,
        successful_prefix: &failure.successful_prefix,
        failing_input: &failure.failing_input,
        interpreter: &failure.interpreter,
        uncached: &failure.uncached,
        cached: &failure.cached,
        cache_before: &failure.cache_before,
        cache_after: &failure.cache_after,
        failure_kind: &failure.failure_kind,
        failure_detail: &failure.failure_detail,
        reproduction_command: &failure.reproduction_command,
    })
    .expect("cache failure serialization");
    digest(&bytes)
}

fn retained_hit_reduction_digest(artifact: &RetainedHitReductionArtifact) -> String {
    let bytes = serde_json::to_vec(&RetainedHitReductionPayload {
        version: artifact.version,
        profile: &artifact.profile,
        source_inputs_digest: &artifact.source_inputs_digest,
        source_failure_file_digest: &artifact.source_failure_file_digest,
        source_failure_digest: &artifact.source_failure_digest,
        source_failure_path: &artifact.source_failure_path,
        strategy: &artifact.strategy,
        warmup: &artifact.warmup,
        original_failing_input: &artifact.original_failing_input,
        reduced_failing_input: &artifact.reduced_failing_input,
        mismatch_signature: &artifact.mismatch_signature,
        original_state_metric: artifact.original_state_metric,
        reduced_state_metric: artifact.reduced_state_metric,
        original_replay_runs: artifact.original_replay_runs,
        reduced_replay_runs: artifact.reduced_replay_runs,
    })
    .expect("retained-hit reduction serialization");
    digest(&bytes)
}

fn retained_hit_reduction_status_digest(status: &RetainedHitReductionStatus) -> String {
    let bytes = serde_json::to_vec(&RetainedHitReductionStatusPayload {
        version: status.version,
        profile: &status.profile,
        source_failure_digest: &status.source_failure_digest,
        outcome: &status.outcome,
        evaluations: status.evaluations,
        reduced_artifact: &status.reduced_artifact,
    })
    .expect("retained-hit reduction status serialization");
    digest(&bytes)
}

fn retained_hit_state_metric(input: &Input) -> u32 {
    input
        .initial_gpr
        .iter()
        .map(|value| value.count_ones())
        .sum::<u32>()
        + (input.initial_rflags & REDUCER_FLAG_MASK).count_ones()
}

fn retained_hit_state_only(original: &Input, candidate: &Input) -> bool {
    validate_input(candidate).is_ok()
        && original.version == candidate.version
        && original.profile == candidate.profile
        && original.seed == candidate.seed
        && original.index == candidate.index
        && original.code == candidate.code
        && original.guest_base_rip == candidate.guest_base_rip
        && original.initial_rip == candidate.initial_rip
        && original.initial_auxiliary_state == candidate.initial_auxiliary_state
        && original.stop_rip == candidate.stop_rip
        && original.stop_requested == candidate.stop_requested
        && original.budget == candidate.budget
        && original
            .initial_gpr
            .iter()
            .zip(candidate.initial_gpr)
            .all(|(before, after)| before == &after || after == 0)
        && ((original.initial_rflags ^ candidate.initial_rflags) & !REDUCER_FLAG_MASK) == 0
        && (candidate.initial_rflags & REDUCER_FLAG_MASK & !original.initial_rflags) == 0
        && candidate.input_digest == digest(&canonical_input(candidate))
}

fn retained_hit_state_candidates(input: &Input) -> Vec<Input> {
    let mut candidates = Vec::new();
    for register in 0..input.initial_gpr.len() {
        if input.initial_gpr[register] != 0 {
            let mut candidate = input.clone();
            candidate.initial_gpr[register] = 0;
            candidates.push(finish_input(candidate));
        }
    }
    if input.initial_rflags & REDUCER_FLAG_MASK != 0 {
        let mut candidate = input.clone();
        candidate.initial_rflags &= !REDUCER_FLAG_MASK;
        candidates.push(finish_input(candidate));
    }
    candidates
}

fn replay_cache_trace_failure(
    path: &Path,
    injected_cached_observation: Option<Observation>,
) -> Result<(), String> {
    let failure: CacheTraceFailureEnvelope = serde_json::from_slice(
        &fs::read(path).map_err(|error| format!("cannot read failure trace: {error}"))?,
    )
    .map_err(|error| format!("malformed failure trace: {error}"))?;
    if failure.version != CACHE_TRACE_VERSION
        || failure.profile != PROFILE
        || !matches!(
            failure.segment_kind.as_str(),
            "generated_corpus" | "retained_hit"
        )
        || !matches!(failure.strategy.as_str(), "eager" | "demand_candidate")
        || failure.sequence_index != failure.successful_prefix.len()
        || failure.sequence_index
            >= if failure.segment_kind == "generated_corpus" {
                SEEDS.len() * CASES_PER_SEED
            } else {
                2
            }
        || failure.failure_digest != cache_trace_failure_digest(&failure)
    {
        return Err("failure trace version, bounds, strategy, or digest differs".into());
    }
    let source_path = artifact_dir().join("native-register-v1-observations.json");
    let source: CorpusObservationsEnvelope = serde_json::from_slice(
        &fs::read(&source_path)
            .map_err(|error| format!("cannot read failure source corpus: {error}"))?,
    )
    .map_err(|error| format!("malformed failure source corpus: {error}"))?;
    validate_corpus_observations(&source)?;
    let candidate = failure.strategy == "demand_candidate";
    let retained_baselines: Vec<_> = if failure.segment_kind == "retained_hit" {
        retained_hit_inputs()
            .iter()
            .map(|input| {
                Ok::<_, String>((
                    reference_observation(input),
                    run_backend(input, candidate)
                        .map_err(|error| format!("retained failure baseline error: {error}"))?,
                ))
            })
            .collect::<Result<_, _>>()?
    } else {
        Vec::new()
    };
    let (expected_input, expected_interpreter, expected_uncached) =
        if failure.segment_kind == "generated_corpus" {
            let source_case = source
                .cases
                .get(failure.sequence_index)
                .ok_or_else(|| "failure trace sequence is outside source corpus".to_owned())?;
            let source_uncached = if candidate {
                &source_case.backend_runs.demand_candidate[0]
            } else {
                &source_case.backend_runs.eager_native[0]
            };
            (
                source_case.input.clone(),
                source_case.backend_runs.interpreter[0]
                    .as_ref()
                    .unwrap()
                    .clone(),
                source_uncached.as_ref().unwrap().clone(),
            )
        } else {
            let input = retained_hit_inputs()[failure.sequence_index].clone();
            let (interpreter, uncached) = &retained_baselines[failure.sequence_index];
            (input, interpreter.clone(), uncached.clone())
        };
    if failure.source_inputs_digest != source.inputs_digest
        || failure.failing_input != expected_input
        || failure.interpreter != expected_interpreter
        || failure.uncached != expected_uncached
    {
        return Err("failure trace source identity or baselines differ".into());
    }
    let candidate = failure.strategy == "demand_candidate";
    let mut cache = od_jit::dispatch::SingleEntryBlockCache::new(
        1,
        od_jit::dispatch::MAX_SINGLE_ENTRY_CACHE_BYTES,
    )
    .map_err(|error| format!("cannot create failure replay cache: {error:?}"))?;
    for (sequence, expected) in failure.successful_prefix.iter().enumerate() {
        let (prefix_input, prefix_interpreter, prefix_uncached) =
            if failure.segment_kind == "generated_corpus" {
                let source_case = &source.cases[sequence];
                let source_uncached = if candidate {
                    &source_case.backend_runs.demand_candidate[0]
                } else {
                    &source_case.backend_runs.eager_native[0]
                };
                (
                    source_case.input.clone(),
                    source_case.backend_runs.interpreter[0]
                        .as_ref()
                        .unwrap()
                        .clone(),
                    source_uncached.as_ref().unwrap().clone(),
                )
            } else {
                let input = retained_hit_inputs()[sequence].clone();
                let (interpreter, uncached) = &retained_baselines[sequence];
                (input, interpreter.clone(), uncached.clone())
            };
        if expected.sequence_index != sequence
            || expected.input != prefix_input
            || expected.interpreter != prefix_interpreter
            || expected.uncached != prefix_uncached
        {
            return Err(format!("failure prefix sequence differs at {sequence}"));
        }
        let (_, _, bindings) = cache_bindings(&expected.input);
        if bindings != expected.bindings {
            return Err(format!("failure prefix bindings differ at {sequence}"));
        }
        let (actual, counters) = run_cached_backend(&expected.input, candidate, &mut cache)
            .map_err(|error| format!("failure prefix backend error at {sequence}: {error}"))?;
        if actual != expected.cached || counters != expected.cache {
            return Err(format!(
                "failure prefix observation/counters differ at {sequence}"
            ));
        }
        compare(&expected.interpreter, &actual)
            .map_err(|class| format!("saved prefix interpreter mismatch at {sequence}: {class}"))?;
        compare(&expected.uncached, &actual)
            .map_err(|class| format!("saved prefix uncached mismatch at {sequence}: {class}"))?;
    }
    let before = cache_counter_snapshot(&cache);
    if before != failure.cache_before {
        return Err("failure replay pre-step counters differ".into());
    }
    let replayed = run_cached_backend(&failure.failing_input, candidate, &mut cache);
    let (cached, after, backend_error) = match replayed {
        Ok((actual, counters)) => {
            let actual = injected_cached_observation.unwrap_or(actual);
            (Some(actual), counters, None)
        }
        Err(error) => (None, cache_counter_snapshot(&cache), Some(error)),
    };
    if cached != failure.cached || after != failure.cache_after {
        return Err("saved cache failure did not reproduce: observation/counters differ".into());
    }
    let reproduced = if failure.failure_kind == "cache_counter_mismatch"
        && failure.segment_kind == "retained_hit"
    {
        retained_hit_counter_mismatch(failure.sequence_index, &failure.cache_after)
            .map(|detail| ("cache_counter_mismatch", detail))
    } else if let Some(cached) = cached.as_ref() {
        if let Err(detail) = compare(&failure.interpreter, cached) {
            Some(("interpreter_mismatch", detail))
        } else if let Err(detail) = compare(&failure.uncached, cached) {
            Some(("uncached_mismatch", detail))
        } else {
            None
        }
    } else {
        backend_error.map(|detail| ("backend_error", detail))
    };
    if reproduced
        .as_ref()
        .map(|(kind, detail)| (*kind, detail.as_str()))
        != Some((
            failure.failure_kind.as_str(),
            failure.failure_detail.as_str(),
        ))
    {
        cache.clear();
        return Err("saved cache failure did not reproduce with the same signature".into());
    }
    cache.clear();
    if cache.resident_entries() != 0 {
        return Err("failure replay retained an executable handle after clear".into());
    }
    Ok(())
}

struct RetainedHitTrial {
    warmup: CacheTraceStep,
    signature: Option<String>,
    cached: Option<Observation>,
}

fn retained_hit_trial<F>(
    input: &Input,
    strategy: &str,
    inject: &mut F,
) -> Result<RetainedHitTrial, String>
where
    F: FnMut(&Input, Observation) -> Result<Observation, String>,
{
    let candidate = match strategy {
        "eager" => false,
        "demand_candidate" => true,
        other => return Err(format!("unsupported retained-hit strategy {other}")),
    };
    let canonical = retained_hit_inputs();
    if !retained_hit_state_only(&canonical[1], input)
        || cache_bindings(&canonical[0]).2 != cache_bindings(input).2
    {
        return Err("reduced input changed retained-hit code or cache context".into());
    }
    let warmup_input = &canonical[0];
    let warmup_interpreter = reference_observation(warmup_input);
    let warmup_uncached = run_backend(warmup_input, candidate)
        .map_err(|error| format!("warmup uncached backend error: {error}"))?;
    compare(&warmup_interpreter, &warmup_uncached)
        .map_err(|class| format!("warmup uncached semantic mismatch: {class}"))?;
    let failing_interpreter = reference_observation(input);
    let failing_uncached = run_backend(input, candidate)
        .map_err(|error| format!("failing uncached backend error: {error}"))?;
    compare(&failing_interpreter, &failing_uncached)
        .map_err(|class| format!("failing uncached semantic mismatch: {class}"))?;

    let mut cache = od_jit::dispatch::SingleEntryBlockCache::new(
        1,
        od_jit::dispatch::MAX_SINGLE_ENTRY_CACHE_BYTES,
    )
    .map_err(|error| format!("cannot create retained-hit trial cache: {error:?}"))?;
    let (warmup_cached, warmup_counters) = run_cached_backend(warmup_input, candidate, &mut cache)
        .map_err(|error| format!("warmup cached backend error: {error}"))?;
    if retained_hit_counter_mismatch(0, &warmup_counters).is_some()
        || compare(&warmup_interpreter, &warmup_cached).is_err()
        || compare(&warmup_uncached, &warmup_cached).is_err()
    {
        cache.clear();
        return Err(
            "warmup failed to prove one miss, compile, publication, and semantic match".into(),
        );
    }
    let (_, _, warmup_bindings) = cache_bindings(warmup_input);
    let warmup = CacheTraceStep {
        sequence_index: 0,
        input: warmup_input.clone(),
        bindings: warmup_bindings,
        interpreter: warmup_interpreter,
        uncached: warmup_uncached,
        cached: warmup_cached,
        cache: warmup_counters.clone(),
    };
    let before = cache_counter_snapshot(&cache);
    if before != warmup_counters {
        cache.clear();
        return Err("failing input did not begin immediately after the exact warmup".into());
    }
    let (cached, after) = run_cached_backend(input, candidate, &mut cache)
        .map_err(|error| format!("failing cached backend error: {error}"))?;
    let cached = inject(input, cached)?;
    if after.hits != before.hits + 1
        || after.misses != before.misses
        || after.successful_compiles != before.successful_compiles
        || after.successful_publications != before.successful_publications
        || retained_hit_counter_mismatch(1, &after).is_some()
    {
        cache.clear();
        return Err(
            "failing input did not prove one retained hit without compile/publication".into(),
        );
    }
    let signature = if let Err(class) = compare(&failing_interpreter, &cached) {
        Some(format!("interpreter_mismatch:{class}"))
    } else if let Err(class) = compare(&failing_uncached, &cached) {
        Some(format!("uncached_mismatch:{class}"))
    } else {
        None
    };
    cache.clear();
    if cache.resident_entries() != 0 {
        return Err("retained-hit trial kept a cache handle after clear".into());
    }
    Ok(RetainedHitTrial {
        warmup,
        signature,
        cached: Some(cached),
    })
}

fn semantic_retained_signature(failure: &CacheTraceFailureEnvelope) -> Result<String, String> {
    if failure.segment_kind != "retained_hit"
        || failure.sequence_index != 1
        || failure.successful_prefix.len() != 1
        || !matches!(
            failure.failure_kind.as_str(),
            "interpreter_mismatch" | "uncached_mismatch"
        )
        || !["gpr", "flags", "rip", "auxiliary_state"].contains(&failure.failure_detail.as_str())
    {
        return Err("only retained_hit step 1 semantic state mismatches can be reduced".into());
    }
    Ok(format!(
        "{}:{}",
        failure.failure_kind, failure.failure_detail
    ))
}

fn run_retained_hit_reducer<F>(
    source_path: &Path,
    output_dir: &Path,
    max_evaluations: usize,
    mut inject: F,
) -> Result<Option<RetainedHitReductionArtifact>, String>
where
    F: FnMut(&Input, Observation) -> Result<Observation, String>,
{
    if fs::symlink_metadata(source_path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(true)
    {
        return Err("source failure trace is missing or a symlink".into());
    }
    let source_bytes = fs::read(source_path)
        .map_err(|error| format!("cannot read source failure trace: {error}"))?;
    let failure: CacheTraceFailureEnvelope = serde_json::from_slice(&source_bytes)
        .map_err(|error| format!("malformed source failure trace: {error}"))?;
    let signature = semantic_retained_signature(&failure)?;
    let canonical = retained_hit_inputs();
    if failure.version != CACHE_TRACE_VERSION
        || failure.profile != PROFILE
        || failure.source_inputs_digest != load_source_observations()?.inputs_digest
        || failure.failing_input != canonical[1]
        || failure.successful_prefix[0].input != canonical[0]
        || failure.successful_prefix[0].sequence_index != 0
        || failure.failure_digest != cache_trace_failure_digest(&failure)
    {
        return Err(
            "source failure trace is not the immutable retained-hit step-1 envelope".into(),
        );
    }
    let source_file_digest = digest(&source_bytes);
    let original_a = retained_hit_trial(&failure.failing_input, &failure.strategy, &mut inject)?;
    let original_b = retained_hit_trial(&failure.failing_input, &failure.strategy, &mut inject)?;
    if original_a.signature.as_deref() != Some(&signature)
        || original_b.signature.as_deref() != Some(&signature)
        || original_a.cached != failure.cached
        || original_b.cached != failure.cached
        || original_a.warmup != failure.successful_prefix[0]
        || original_b.warmup != failure.successful_prefix[0]
    {
        return Err("source mismatch was not stable across two exact retained-hit replays".into());
    }
    replay_cache_trace_failure(source_path, original_a.cached.clone())?;

    let mut current = failure.failing_input.clone();
    let mut evaluations = 0usize;
    while evaluations < max_evaluations.min(REDUCER_MAX_EVALUATIONS) {
        let mut accepted = None;
        for candidate in retained_hit_state_candidates(&current) {
            if evaluations >= max_evaluations.min(REDUCER_MAX_EVALUATIONS) {
                break;
            }
            if !retained_hit_state_only(&failure.failing_input, &candidate)
                || retained_hit_state_metric(&candidate) >= retained_hit_state_metric(&current)
            {
                continue;
            }
            evaluations += 1;
            let first = retained_hit_trial(&candidate, &failure.strategy, &mut inject);
            let second = retained_hit_trial(&candidate, &failure.strategy, &mut inject);
            if let (Ok(first), Ok(second)) = (first, second) {
                if first.signature.as_deref() == Some(&signature)
                    && second.signature.as_deref() == Some(&signature)
                    && first.cached == second.cached
                    && first.warmup == failure.successful_prefix[0]
                    && second.warmup == failure.successful_prefix[0]
                {
                    accepted = Some(candidate);
                    break;
                }
            }
        }
        match accepted {
            Some(candidate) => current = candidate,
            None => break,
        }
    }
    let hit_limit = evaluations >= max_evaluations.min(REDUCER_MAX_EVALUATIONS);
    let artifact = if current == failure.failing_input {
        None
    } else {
        let artifact_path = output_dir.join("retained-hit-reduced-input-v1.json");
        let mut artifact = RetainedHitReductionArtifact {
            version: RETAINED_HIT_REDUCTION_VERSION,
            profile: PROFILE.into(),
            source_inputs_digest: failure.source_inputs_digest.clone(),
            source_failure_file_digest: source_file_digest,
            source_failure_digest: failure.failure_digest.clone(),
            source_failure_path: source_path.to_string_lossy().into_owned(),
            strategy: failure.strategy.clone(),
            warmup: failure.successful_prefix[0].clone(),
            original_failing_input: failure.failing_input.clone(),
            reduced_failing_input: current.clone(),
            mismatch_signature: signature,
            original_state_metric: retained_hit_state_metric(&failure.failing_input),
            reduced_state_metric: retained_hit_state_metric(&current),
            original_replay_runs: 2,
            reduced_replay_runs: 2,
            artifact_digest: String::new(),
        };
        artifact.artifact_digest = retained_hit_reduction_digest(&artifact);
        replay_retained_hit_reduction_artifact(&artifact, &mut inject)?;
        write_json_create_new(&artifact_path, &artifact)?;
        Some(artifact)
    };
    let outcome = if hit_limit {
        "evaluation_limit"
    } else if artifact.is_some() {
        "reduced"
    } else {
        "not_reproduced"
    };
    let mut status = RetainedHitReductionStatus {
        version: RETAINED_HIT_REDUCTION_VERSION,
        profile: PROFILE.into(),
        source_failure_digest: failure.failure_digest,
        outcome: outcome.into(),
        evaluations,
        reduced_artifact: artifact
            .as_ref()
            .map(|_| "retained-hit-reduced-input-v1.json".into()),
        status_digest: String::new(),
    };
    status.status_digest = retained_hit_reduction_status_digest(&status);
    write_json_create_new(
        &output_dir.join("retained-hit-reducer-status-v1.json"),
        &status,
    )?;
    Ok(artifact)
}

fn load_source_observations() -> Result<CorpusObservationsEnvelope, String> {
    let path = artifact_dir().join("native-register-v1-observations.json");
    if fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(true)
    {
        return Err("source observation corpus is missing or a symlink".into());
    }
    let source: CorpusObservationsEnvelope = serde_json::from_slice(
        &fs::read(&path).map_err(|error| format!("cannot read source corpus: {error}"))?,
    )
    .map_err(|error| format!("malformed source corpus: {error}"))?;
    validate_corpus_observations(&source)?;
    Ok(source)
}

fn replay_retained_hit_reduction<F>(artifact_path: &Path, inject: &mut F) -> Result<(), String>
where
    F: FnMut(&Input, Observation) -> Result<Observation, String>,
{
    if fs::symlink_metadata(artifact_path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(true)
    {
        return Err("retained-hit reduction artifact is missing or a symlink".into());
    }
    let artifact_bytes = fs::read(artifact_path)
        .map_err(|error| format!("cannot read retained-hit reduction artifact: {error}"))?;
    let artifact: RetainedHitReductionArtifact = serde_json::from_slice(&artifact_bytes)
        .map_err(|error| format!("malformed retained-hit reduction artifact: {error}"))?;
    replay_retained_hit_reduction_artifact(&artifact, inject)
}

fn replay_retained_hit_reduction_artifact<F>(
    artifact: &RetainedHitReductionArtifact,
    inject: &mut F,
) -> Result<(), String>
where
    F: FnMut(&Input, Observation) -> Result<Observation, String>,
{
    if artifact.version != RETAINED_HIT_REDUCTION_VERSION
        || artifact.profile != PROFILE
        || artifact.artifact_digest != retained_hit_reduction_digest(&artifact)
        || artifact.original_replay_runs != 2
        || artifact.reduced_replay_runs != 2
        || artifact.original_state_metric
            != retained_hit_state_metric(&artifact.original_failing_input)
        || artifact.reduced_state_metric
            != retained_hit_state_metric(&artifact.reduced_failing_input)
        || artifact.reduced_state_metric >= artifact.original_state_metric
        || !retained_hit_state_only(
            &artifact.original_failing_input,
            &artifact.reduced_failing_input,
        )
    {
        return Err("retained-hit reduction schema, digest, or state-only binding differs".into());
    }
    let source_path = Path::new(&artifact.source_failure_path);
    if fs::symlink_metadata(source_path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(true)
    {
        return Err("bound source failure is missing or a symlink".into());
    }
    let source_bytes = fs::read(source_path)
        .map_err(|error| format!("cannot read bound source failure: {error}"))?;
    if digest(&source_bytes) != artifact.source_failure_file_digest {
        return Err("bound source failure file digest differs".into());
    }
    let failure: CacheTraceFailureEnvelope = serde_json::from_slice(&source_bytes)
        .map_err(|error| format!("malformed bound source failure: {error}"))?;
    if failure.failure_digest != artifact.source_failure_digest
        || failure.failure_digest != cache_trace_failure_digest(&failure)
        || failure.source_inputs_digest != artifact.source_inputs_digest
        || failure.strategy != artifact.strategy
        || failure.successful_prefix.first() != Some(&artifact.warmup)
        || failure.failing_input != artifact.original_failing_input
        || semantic_retained_signature(&failure)? != artifact.mismatch_signature
    {
        return Err(
            "reduced artifact no longer binds the exact original failure and warmup".into(),
        );
    }
    let source = load_source_observations()?;
    if source.inputs_digest != artifact.source_inputs_digest {
        return Err("reduced artifact source corpus digest differs".into());
    }
    let mut original_cached = None;
    for input in [
        &artifact.original_failing_input,
        &artifact.reduced_failing_input,
    ] {
        let first = retained_hit_trial(input, &artifact.strategy, inject)?;
        let second = retained_hit_trial(input, &artifact.strategy, inject)?;
        if input == &artifact.original_failing_input {
            original_cached = first.cached.clone();
        }
        if first.signature.as_deref() != Some(&artifact.mismatch_signature)
            || second.signature.as_deref() != Some(&artifact.mismatch_signature)
            || first.cached != second.cached
            || first.warmup != artifact.warmup
            || second.warmup != artifact.warmup
        {
            return Err(
                "reduced retained-hit replay was unstable or changed mismatch signature".into(),
            );
        }
    }
    replay_cache_trace_failure(source_path, original_cached)?;
    Ok(())
}

fn save_cache_trace_failure(
    source_inputs_digest: &str,
    segment_kind: &str,
    strategy: &str,
    sequence_index: usize,
    successful_prefix: &[CacheTraceStep],
    input: &Input,
    interpreter: &Observation,
    uncached: &Observation,
    cached: Option<Observation>,
    cache_before: CacheCounterSnapshot,
    cache_after: CacheCounterSnapshot,
    failure_kind: &str,
    failure_detail: &str,
) -> String {
    let artifact = artifact_dir().join(format!(
        "native-register-v1-cache-mismatch-{segment_kind}-{strategy}-{sequence_index:04}.json"
    ));
    let reproduction_command = format!(
        "OD_JIT_NATIVE_REGISTER_V1_CORPUS_DIR={} OD_JIT_NATIVE_REGISTER_V1_FAILURE_TRACE={} cargo test --offline -p od-host --lib tests::native_register_v1_corpus::cached_failure_trace_replays_saved_prefix_and_failure -- --exact --test-threads=1",
        artifact_dir().display(),
        artifact.display()
    );
    let mut failure = CacheTraceFailureEnvelope {
        version: CACHE_TRACE_VERSION,
        profile: PROFILE.into(),
        source_inputs_digest: source_inputs_digest.into(),
        segment_kind: segment_kind.into(),
        strategy: strategy.into(),
        sequence_index,
        successful_prefix: successful_prefix.to_vec(),
        failing_input: input.clone(),
        interpreter: interpreter.clone(),
        uncached: uncached.clone(),
        cached,
        cache_before,
        cache_after,
        failure_kind: failure_kind.into(),
        failure_detail: failure_detail.into(),
        reproduction_command,
        failure_digest: String::new(),
    };
    failure.failure_digest = cache_trace_failure_digest(&failure);
    match write_json_create_new(&artifact, &failure) {
        Ok(()) => format!(
            "{failure_kind} at {segment_kind}/{strategy} step {sequence_index}: {failure_detail}; saved successful prefix and failing input to {} (digest={})",
            artifact.display(),
            failure.failure_digest
        ),
        Err(error) => format!(
            "{failure_kind} at {segment_kind}/{strategy} step {sequence_index}: {failure_detail}; could not save mismatch artifact {}: {error}",
            artifact.display()
        ),
    }
}

fn finish_cache_trace(
    source_inputs_digest: String,
    segments: Vec<CacheTraceSegment>,
) -> CacheTraceEnvelope {
    let mut trace = CacheTraceEnvelope {
        version: CACHE_TRACE_VERSION,
        profile: PROFILE.into(),
        source_inputs_digest,
        segments,
        trace_digest: String::new(),
    };
    trace.trace_digest = cache_trace_digest(&trace);
    trace
}

fn validate_corpus_observations(envelope: &CorpusObservationsEnvelope) -> Result<(), String> {
    if envelope.version != ENVELOPE_VERSION || envelope.profile != PROFILE {
        return Err("unsupported corpus observations version/profile".into());
    }
    if envelope.cases.len() != SEEDS.len() * CASES_PER_SEED {
        return Err("corpus observations case count differs".into());
    }
    let mut unique = std::collections::BTreeSet::new();
    let mut input_hash = 0xcbf2_9ce4_8422_2325u64;
    for (sequence, case) in envelope.cases.iter().enumerate() {
        let seed = SEEDS[sequence / CASES_PER_SEED];
        let index = sequence % CASES_PER_SEED;
        if case.input != generate_case(seed, index) {
            return Err(format!(
                "corpus source input order differs at step {sequence}"
            ));
        }
        validate_input(&case.input)?;
        let canonical = canonical_input(&case.input);
        if !unique.insert(canonical.clone()) {
            return Err(format!("duplicate corpus input at step {sequence}"));
        }
        for byte in canonical {
            input_hash = (input_hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
        }
        let saved = SavedCase {
            input: case.input.clone(),
            backend_runs: case.backend_runs.clone(),
            failure_signature: None,
            reproduction_command: String::new(),
        };
        validate_saved_observations(&saved)
            .map_err(|error| format!("invalid source observations at step {sequence}: {error}"))?;
        if case.backend_runs.interpreter[0] != case.backend_runs.interpreter[1]
            || case.backend_runs.eager_native[0] != case.backend_runs.eager_native[1]
            || case.backend_runs.demand_candidate[0] != case.backend_runs.demand_candidate[1]
        {
            return Err(format!("source backend repeats differ at step {sequence}"));
        }
    }
    let expected_inputs_digest = format!("fnv1a64:{input_hash:016x}");
    if envelope.inputs_digest != expected_inputs_digest {
        return Err("corpus observations input digest mismatch".into());
    }
    if envelope.envelope_digest
        != corpus_observations_digest(
            envelope.version,
            &envelope.profile,
            &envelope.inputs_digest,
            &envelope.cases,
        )
    {
        return Err("corpus observations envelope digest mismatch".into());
    }
    Ok(())
}

fn retained_hit_inputs() -> [Input; 2] {
    let base = 0x0000_0001_0000_8000;
    let code = vec![0x48, 0x83, 0xc0, 0x01]; // ADD RAX, 1; one-block CodeEnd
    let make = |index, rax, rflags| {
        let mut gpr = [0; 16];
        gpr[0] = rax;
        gpr[3] = 0x0102_0304_0506_0708;
        finish_input(Input {
            version: ENVELOPE_VERSION,
            profile: PROFILE.into(),
            seed: 0x5333_3470_7265_7461,
            index,
            code: code.clone(),
            guest_base_rip: base,
            initial_gpr: gpr,
            initial_rip: base,
            initial_rflags: rflags,
            initial_auxiliary_state: default_auxiliary_state(),
            stop_rip: None,
            stop_requested: false,
            budget: 1,
            input_digest: String::new(),
        })
    };
    [make(0, u64::MAX, 0x202), make(1, 0x1234, 0x203)]
}

fn retained_hit_counter_mismatch(
    sequence: usize,
    counters: &CacheCounterSnapshot,
) -> Option<String> {
    let valid = if sequence == 0 {
        counters.hits == 0
            && counters.misses == 1
            && counters.successful_compiles == 1
            && counters.successful_publications == 1
    } else if sequence == 1 {
        counters.hits == 1
            && counters.misses == 1
            && counters.successful_compiles == 1
            && counters.successful_publications == 1
    } else {
        false
    };
    (!valid).then(|| format!("retained-hit counter witness differs at step {sequence}"))
}

fn validate_cache_trace(
    trace: &CacheTraceEnvelope,
    corpus: &CorpusObservationsEnvelope,
) -> Result<(), String> {
    validate_corpus_observations(corpus)?;
    if trace.version != CACHE_TRACE_VERSION || trace.profile != PROFILE {
        return Err("unsupported cache trace version/profile".into());
    }
    if trace.source_inputs_digest != corpus.inputs_digest {
        return Err("cache trace source digest differs from the generated corpus".into());
    }
    if trace.trace_digest != cache_trace_digest(trace) {
        return Err("cache trace digest mismatch".into());
    }
    if trace.segments.len() != 4 {
        return Err("cache trace must contain two corpus and two retained-hit segments".into());
    }
    let expected_segments = [
        ("generated_corpus", "eager"),
        ("generated_corpus", "demand_candidate"),
        ("retained_hit", "eager"),
        ("retained_hit", "demand_candidate"),
    ];
    for (segment_index, (segment, expected_identity)) in
        trace.segments.iter().zip(expected_segments).enumerate()
    {
        if (segment.kind.as_str(), segment.strategy.as_str()) != expected_identity {
            return Err(format!(
                "cache trace segment order differs at {segment_index}"
            ));
        }
        let retained = segment.kind == "retained_hit";
        let expected_count = if retained { 2 } else { corpus.cases.len() };
        if segment.steps.len() != expected_count {
            return Err(format!(
                "cache trace step count differs in segment {segment_index}"
            ));
        }
        let mut previous = CacheCounterSnapshot {
            hits: 0,
            misses: 0,
            successful_compiles: 0,
            successful_publications: 0,
            replacements: 0,
            resident_entries_high_water: 0,
            resident_code_bytes_high_water: 0,
            resident_logical_metadata_bytes_high_water: 0,
            resident_entries: 0,
            resident_code_bytes: 0,
            resident_logical_bytes: 0,
        };
        for (sequence, step) in segment.steps.iter().enumerate() {
            if step.sequence_index != sequence {
                return Err(format!(
                    "cache trace sequence index differs at {segment_index}:{sequence}"
                ));
            }
            validate_input(&step.input)?;
            let (_, dependencies, bindings) = cache_bindings(&step.input);
            if dependencies[0].start_rip != step.bindings.dependency_start_rip
                || dependencies[0].end_rip != step.bindings.dependency_end_rip
                || bindings != step.bindings
            {
                return Err(format!(
                    "cache bindings differ at {segment_index}:{sequence}"
                ));
            }
            let source_case = if retained {
                let retained_inputs = retained_hit_inputs();
                if step.input != retained_inputs[sequence] {
                    return Err(format!(
                        "retained cache input differs at {segment_index}:{sequence}"
                    ));
                }
                None
            } else {
                let case = &corpus.cases[sequence];
                if step.input != case.input {
                    return Err(format!(
                        "cache corpus input differs at {segment_index}:{sequence}"
                    ));
                }
                Some(case)
            };
            let (expected_interpreter, expected_uncached) = if let Some(case) = source_case {
                let uncached = if segment.strategy == "eager" {
                    case.backend_runs.eager_native[0].clone().unwrap()
                } else {
                    case.backend_runs.demand_candidate[0].clone().unwrap()
                };
                (case.backend_runs.interpreter[0].clone().unwrap(), uncached)
            } else {
                let input = &step.input;
                let interpreter = reference_observation(input);
                let uncached = run_backend(input, segment.strategy == "demand_candidate")?;
                (interpreter, uncached)
            };
            if step.interpreter != expected_interpreter || step.uncached != expected_uncached {
                return Err(format!(
                    "cache trace baseline observation differs at {segment_index}:{sequence}"
                ));
            }
            compare(&step.interpreter, &step.cached).map_err(|class| {
                format!("cached trace observation differs at {segment_index}:{sequence}: {class}")
            })?;
            compare(&step.uncached, &step.cached).map_err(|class| {
                format!("cached/uncached parity differs at {segment_index}:{sequence}: {class}")
            })?;
            if step.cache.hits < previous.hits
                || step.cache.misses < previous.misses
                || step.cache.successful_compiles < previous.successful_compiles
                || step.cache.successful_publications < previous.successful_publications
                || step.cache.replacements < previous.replacements
                || step.cache.resident_entries_high_water < previous.resident_entries_high_water
                || step.cache.resident_code_bytes_high_water
                    < previous.resident_code_bytes_high_water
                || step.cache.resident_logical_metadata_bytes_high_water
                    < previous.resident_logical_metadata_bytes_high_water
                || step.cache.resident_entries > 1
                || step.cache.resident_code_bytes > od_jit::dispatch::MAX_SINGLE_ENTRY_CACHE_BYTES
            {
                return Err(format!(
                    "cache counters regress or exceed capacity at {segment_index}:{sequence}"
                ));
            }
            previous = step.cache.clone();
        }
        if retained {
            let first = &segment.steps[0].cache;
            let hit = &segment.steps[1].cache;
            if first.misses != 1
                || first.successful_compiles != 1
                || first.successful_publications != 1
                || hit.hits != 1
                || hit.successful_compiles != first.successful_compiles
                || hit.successful_publications != first.successful_publications
            {
                return Err(format!(
                    "retained one-block cache hit evidence is invalid in segment {segment_index}"
                ));
            }
        }
    }
    let eager_retained = &trace.segments[2].steps;
    let candidate_retained = &trace.segments[3].steps;
    if eager_retained[0].bindings != candidate_retained[0].bindings
        || eager_retained[1].bindings != candidate_retained[1].bindings
    {
        return Err("retained trace strategy bindings differ".into());
    }
    Ok(())
}

fn replay_cache_trace(trace: &CacheTraceEnvelope) -> Result<(), String> {
    let corpus_path = artifact_dir().join("native-register-v1-observations.json");
    let corpus: CorpusObservationsEnvelope = serde_json::from_slice(
        &fs::read(&corpus_path)
            .map_err(|error| format!("cannot read source corpus observations: {error}"))?,
    )
    .map_err(|error| format!("cannot decode source corpus observations: {error}"))?;
    validate_cache_trace(trace, &corpus)?;
    for segment in &trace.segments {
        let mut cache = od_jit::dispatch::SingleEntryBlockCache::new(
            1,
            od_jit::dispatch::MAX_SINGLE_ENTRY_CACHE_BYTES,
        )
        .map_err(|error| format!("cannot create cache trace replay cache: {error:?}"))?;
        let candidate = segment.strategy == "demand_candidate";
        for step in &segment.steps {
            let (actual, counters) = run_cached_backend(&step.input, candidate, &mut cache)?;
            if actual != step.cached {
                return Err(format!(
                    "cached trace replay observation changed at {}:{}",
                    segment.strategy, step.sequence_index
                ));
            }
            if counters != step.cache {
                return Err(format!(
                    "cached trace replay counters changed at {}:{} expected={:?} actual={:?}",
                    segment.strategy, step.sequence_index, step.cache, counters
                ));
            }
        }
        cache.clear();
        if cache.resident_entries() != 0 {
            return Err("cache trace replay did not release resident handle".into());
        }
    }
    Ok(())
}

fn reference_observation(input: &Input) -> Observation {
    let reference = reference(input);
    observation(
        &reference.state,
        &reference.exit,
        reference.retired_native,
        reference.retired_interpreter,
        reference.remaining_budget,
    )
}

fn compare(expected: &Observation, actual: &Observation) -> Result<(), String> {
    if expected.gpr != actual.gpr {
        return Err("gpr".into());
    }
    if expected.rflags != actual.rflags {
        return Err("flags".into());
    }
    if expected.rip != actual.rip {
        return Err("rip".into());
    }
    if expected.exit != actual.exit {
        return Err("exit".into());
    }
    if expected.retired_native != actual.retired_native
        || expected.retired_interpreter != actual.retired_interpreter
    {
        return Err("retirement".into());
    }
    if expected.remaining_budget != actual.remaining_budget {
        return Err("remaining_budget".into());
    }
    if expected.auxiliary_state != actual.auxiliary_state {
        return Err("auxiliary_state".into());
    }
    Ok(())
}

fn artifact_dir() -> PathBuf {
    std::env::var_os("OD_JIT_NATIVE_REGISTER_V1_CORPUS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../work/stage3-native-register-v1/manual")
        })
}

fn write_json(path: &Path, value: &impl Serialize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let bytes = serde_json::to_vec_pretty(value).unwrap();
    fs::write(path, bytes).unwrap();
}

fn write_json_create_new(path: &Path, value: &impl Serialize) -> Result<(), String> {
    fs::create_dir_all(path.parent().unwrap()).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("refusing to overwrite {}: {error}", path.display()))?;
    file.write_all(&bytes).map_err(|error| error.to_string())
}

fn reducer_artifact_dir() -> PathBuf {
    artifact_dir().join("native-register-v1-reducer")
}

fn case_replay_command(case_path: &Path, output_dir: &Path) -> String {
    format!(
        "OD_JIT_NATIVE_REGISTER_V1_CORPUS_DIR={} OD_JIT_NATIVE_REGISTER_V1_REPLAY_CASE={} cargo test --offline -p od-host --lib {REPLAY_GATE} -- --exact --test-threads=1",
        output_dir.display(),
        case_path.display()
    )
}

fn empty_backend_runs(error: Option<String>) -> BackendRuns {
    BackendRuns {
        interpreter: [None, None],
        eager_native: [None, None],
        demand_candidate: [None, None],
        errors: error.into_iter().collect(),
    }
}

fn stable_semantic_mismatch(runs: &BackendRuns, signature: &[String]) -> bool {
    if signature.is_empty()
        || !runs.errors.is_empty()
        || signature.iter().any(|item| {
            item.ends_with(":error")
                || item.ends_with(":repeat")
                || ![
                    "gpr",
                    "flags",
                    "rip",
                    "exit",
                    "retirement",
                    "remaining_budget",
                    "auxiliary_state",
                ]
                .iter()
                .any(|class| item.ends_with(&format!(":{class}")))
        })
    {
        return false;
    }
    let all_repeats_present_and_stable = [
        &runs.interpreter,
        &runs.eager_native,
        &runs.demand_candidate,
    ]
    .iter()
    .all(|pair| pair[0].is_some() && pair[0] == pair[1]);
    all_repeats_present_and_stable
}

fn reducible_code_fields(input: &Input) -> Result<Vec<(usize, usize)>, String> {
    od_jit::block::CodeView::new(input.guest_base_rip, &input.code)
        .map_err(|_| "malformed code view".to_string())?;
    let mut fields = Vec::new();
    let mut offset = 0usize;
    while offset < input.code.len() {
        let mut memory = CodeOnly {
            base: input.guest_base_rip,
            bytes: &input.code,
        };
        let rip = input.guest_base_rip + offset as u64;
        let decoded = od_isa::long_decode::decode(&mut memory, rip, AccessContext::default())
            .map_err(|error| format!("decode failed while locating reducer fields: {error:?}"))?;
        let length = usize::from(decoded.length);
        let bytes = &input.code[offset..offset + length];
        match bytes {
            [0x48, 0x83, opcode @ (0xc0 | 0xf8 | 0xe8), _] if length == 4 => {
                let _ = opcode;
                fields.push((offset + 3, 1));
            }
            [0x48, 0xc7, 0xc0, _, _, _, _] if length == 7 => {
                fields.push((offset + 3, 4));
            }
            [0x48, 0x8d, 0x43, _] if length == 4 => {
                fields.push((offset + 3, 1));
            }
            [0x48, 0x8d, 0x05, _, _, _, _] if length == 7 => {
                fields.push((offset + 3, 4));
            }
            _ => {}
        }
        if length == 0 {
            return Err("decoder produced zero-length instruction".into());
        }
        offset += length;
    }
    Ok(fields)
}

fn reduction_metric(input: &Input) -> Result<u64, String> {
    let mut metric = input
        .initial_gpr
        .iter()
        .map(|value| u64::from(value.count_ones()))
        .sum();
    metric += u64::from((input.initial_rflags & REDUCER_FLAG_MASK).count_ones());
    for (offset, length) in reducible_code_fields(input)? {
        let mut value = 0u64;
        for (index, byte) in input.code[offset..offset + length].iter().enumerate() {
            value |= u64::from(*byte) << (index * 8);
        }
        metric += u64::from(value.count_ones());
    }
    Ok(metric)
}

fn reducer_candidates(input: &Input) -> Result<Vec<Input>, String> {
    let mut candidates = Vec::new();
    for register in 0..input.initial_gpr.len() {
        if input.initial_gpr[register] != 0 {
            let mut next = input.clone();
            next.initial_gpr[register] = 0;
            candidates.push(finish_input(next));
        }
    }
    if input.initial_rflags & REDUCER_FLAG_MASK != 0 {
        let mut next = input.clone();
        next.initial_rflags &= !REDUCER_FLAG_MASK;
        candidates.push(finish_input(next));
    }
    for (offset, length) in reducible_code_fields(input)? {
        if input.code[offset..offset + length]
            .iter()
            .any(|byte| *byte != 0)
        {
            let mut next = input.clone();
            next.code[offset..offset + length].fill(0);
            candidates.push(finish_input(next));
        }
    }
    Ok(candidates)
}

fn is_value_only_candidate(original: &Input, candidate: &Input) -> bool {
    if original.version != candidate.version
        || original.profile != candidate.profile
        || original.seed != candidate.seed
        || original.index != candidate.index
        || original.guest_base_rip != candidate.guest_base_rip
        || original.initial_rip != candidate.initial_rip
        || original.initial_auxiliary_state != candidate.initial_auxiliary_state
        || original.stop_rip != candidate.stop_rip
        || original.stop_requested != candidate.stop_requested
        || original.budget != candidate.budget
        || original.code.len() != candidate.code.len()
        || (original.initial_rflags ^ candidate.initial_rflags) & !REDUCER_FLAG_MASK != 0
        || candidate.initial_rflags & REDUCER_FLAG_MASK & !original.initial_rflags != 0
        || original
            .initial_gpr
            .iter()
            .zip(candidate.initial_gpr)
            .any(|(before, after)| before != &after && after != 0)
    {
        return false;
    }
    let Ok(ranges) = reducible_code_fields(original) else {
        return false;
    };
    if ranges.iter().any(|(start, length)| {
        candidate.code[*start..*start + *length]
            .iter()
            .zip(&original.code[*start..*start + *length])
            .any(|(after, before)| before != after && *after != 0)
    }) {
        return false;
    }
    original
        .code
        .iter()
        .zip(&candidate.code)
        .enumerate()
        .all(|(offset, (before, after))| {
            before == after
                || ranges
                    .iter()
                    .any(|(start, length)| (*start..*start + *length).contains(&offset))
        })
}

fn reduce_saved_case_with<F>(
    source_path: &Path,
    source: &SavedCase,
    output_dir: &Path,
    evaluate: F,
) -> ReductionReport
where
    F: FnMut(&Input) -> Result<(BackendRuns, Vec<String>), String>,
{
    reduce_saved_case_with_limit(
        source_path,
        source,
        output_dir,
        REDUCER_MAX_EVALUATIONS,
        evaluate,
    )
}

fn reduce_saved_case_with_limit<F>(
    source_path: &Path,
    source: &SavedCase,
    output_dir: &Path,
    max_evaluations: usize,
    mut evaluate: F,
) -> ReductionReport
where
    F: FnMut(&Input) -> Result<(BackendRuns, Vec<String>), String>,
{
    let mut current = source.input.clone();
    let source_metric = reduction_metric(&current).expect("validated source metric");
    let source_result = evaluate(&current);
    let (source_runs, source_signature, source_reason) = match source_result {
        Ok((runs, signature))
            if signature == source.failure_signature.clone().unwrap_or_default()
                && stable_semantic_mismatch(&runs, &signature) =>
        {
            (runs, signature, None)
        }
        Ok((runs, signature)) => {
            let reason = if signature.is_empty() {
                "not_reproduced"
            } else if !stable_semantic_mismatch(&runs, &signature) {
                "source_unstable_or_nonsemantic"
            } else {
                "source_signature_changed"
            };
            (runs, signature, Some(reason.to_string()))
        }
        Err(error) => (
            empty_backend_runs(Some(error)),
            Vec::new(),
            Some("source_replay_error".into()),
        ),
    };
    let source_signature_for_report = source_signature.clone();
    let source_command = case_replay_command(source_path, output_dir);
    let mut attempts = Vec::new();
    let mut evaluated_count = 0usize;
    let mut accepted_count = 0usize;
    let mut rejected_count = 0usize;
    let mut current_runs = source_runs.clone();
    let mut stop_reason = source_reason.unwrap_or_default();
    if stop_reason.is_empty() {
        let mut seen = std::collections::BTreeSet::new();
        seen.insert(canonical_input(&current));
        loop {
            let candidates =
                reducer_candidates(&current).expect("admitted reduction candidate fields");
            let mut improved = false;
            for candidate in candidates {
                if evaluated_count >= max_evaluations {
                    stop_reason = "evaluation_limit".into();
                    break;
                }
                if !seen.insert(canonical_input(&candidate)) {
                    continue;
                }
                evaluated_count += 1;
                let before = reduction_metric(&current).expect("current metric");
                let after = reduction_metric(&candidate).expect("candidate metric");
                let validation = if is_value_only_candidate(&current, &candidate) {
                    validate_input(&candidate)
                } else {
                    Err("candidate changed immutable input structure".into())
                };
                let result = validation
                    .map_err(|error| format!("candidate admission/digest rejected: {error}"))
                    .and_then(|()| evaluate(&candidate));
                let (runs, signature, reason) = match result {
                    Ok((runs, signature)) => {
                        let accepted = signature == source_signature_for_report
                            && stable_semantic_mismatch(&runs, &signature)
                            && after < before;
                        let reason = if accepted {
                            "same_stable_signature_and_lower_metric"
                        } else if signature != source_signature_for_report {
                            "signature_changed_or_disappeared"
                        } else if !stable_semantic_mismatch(&runs, &signature) {
                            "unstable_error_or_missing_result"
                        } else {
                            "metric_not_lower"
                        };
                        (runs, signature, reason.to_string())
                    }
                    Err(error) => (
                        empty_backend_runs(Some(error)),
                        Vec::new(),
                        "candidate_rejected".into(),
                    ),
                };
                let accepted = reason == "same_stable_signature_and_lower_metric";
                attempts.push(ReductionAttempt {
                    digest: candidate.input_digest.clone(),
                    metric_before: before,
                    metric_after: after,
                    accepted,
                    reason,
                    signature: signature.clone(),
                    backend_runs: runs.clone(),
                });
                if accepted {
                    accepted_count += 1;
                    current = candidate;
                    current_runs = runs;
                    improved = true;
                    break;
                }
                rejected_count += 1;
            }
            if !stop_reason.is_empty() {
                break;
            }
            if !improved {
                stop_reason = "no_accepted_candidate".into();
                break;
            }
        }
    }
    let reduced = if accepted_count > 0 {
        Some(SavedCase {
            input: current.clone(),
            backend_runs: current_runs.clone(),
            failure_signature: Some(source_signature_for_report.clone()),
            reproduction_command: case_replay_command(
                &output_dir.join("reduced-saved-case.json"),
                output_dir,
            ),
        })
    } else {
        None
    };
    ReductionReport {
        kind: "native-register-v1-value-reduction".into(),
        source_path: source_path.display().to_string(),
        source_digest: source.input.input_digest.clone(),
        source_signature: source.failure_signature.clone().unwrap_or_default(),
        source_replay_signature: source_signature_for_report,
        source_backend_runs: source_runs,
        source_replay_command: source_command,
        reduced_digest: reduced.as_ref().map(|case| case.input.input_digest.clone()),
        reduced_input: reduced.as_ref().map(|case| case.input.clone()),
        reduced_signature: reduced
            .as_ref()
            .and_then(|case| case.failure_signature.clone()),
        reduced_backend_runs: reduced.as_ref().map(|case| case.backend_runs.clone()),
        reduced_replay_command: reduced
            .as_ref()
            .map(|_| case_replay_command(&output_dir.join("reduced-saved-case.json"), output_dir)),
        metric_definition: "popcount(initial GPRs, initial RFLAGS & 0x0cd5, and eligible encoded immediate/displacement values)".into(),
        metric_before: source_metric,
        metric_after: if accepted_count > 0 {
            Some(reduction_metric(&current).expect("final metric"))
        } else {
            None
        },
        evaluated_count,
        accepted_count,
        rejected_count,
        stop_reason,
        attempts,
    }
}

fn persist_reduction(
    report: &ReductionReport,
    source: &SavedCase,
    output_dir: &Path,
) -> Result<(), String> {
    write_json_create_new(&output_dir.join("source-saved-case.json"), source)?;
    if let Some(input) = &report.reduced_input {
        let reduced = SavedCase {
            input: input.clone(),
            backend_runs: report
                .reduced_backend_runs
                .clone()
                .ok_or_else(|| "reduced backend observations missing".to_string())?,
            failure_signature: report.reduced_signature.clone(),
            reproduction_command: report
                .reduced_replay_command
                .clone()
                .ok_or_else(|| "reduced replay command missing".to_string())?,
        };
        write_json_create_new(&output_dir.join("reduced-saved-case.json"), &reduced)?;
        let mut digest_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output_dir.join("reduced-input-digest.txt"))
            .map_err(|error| format!("refusing to overwrite reduced digest: {error}"))?;
        use std::io::Write;
        writeln!(digest_file, "{}", input.input_digest).map_err(|error| error.to_string())?;
    }
    write_json_create_new(&output_dir.join("reduction-report.json"), report)
}

fn run_repeated(input: &Input) -> BackendRuns {
    let (runs, mismatch_signature) = observe_repeated(input);
    if !mismatch_signature.is_empty() {
        let path = artifact_dir().join(format!(
            "mismatch-{:016x}-{:04}.json",
            input.seed, input.index
        ));
        let saved = SavedCase {
            input: input.clone(),
            backend_runs: runs.clone(),
            failure_signature: Some(mismatch_signature.clone()),
            reproduction_command: format!(
                "OD_JIT_NATIVE_REGISTER_V1_CORPUS_DIR={} OD_JIT_NATIVE_REGISTER_V1_REPLAY_CASE={} cargo test --offline -p od-host --lib {REPLAY_GATE} -- --exact --test-threads=1",
                artifact_dir().display(),
                path.display()
            ),
        };
        write_json(&path, &saved);
        panic!(
            "NativeRegisterV1 mismatch signature={mismatch_signature:?}; saved original input, available repeat observations, and backend errors to {}; input={input:?}; runs={runs:?}",
            path.display()
        );
    }
    runs
}

fn validate_saved_observations(saved: &SavedCase) -> Result<(), String> {
    validate_input(&saved.input)?;
    if saved.failure_signature.is_some() {
        return Err(
            "saved case records an expected mismatch; use replay signature validation".into(),
        );
    }
    if !saved.backend_runs.errors.is_empty() {
        return Err("backend result contains error/timeout".into());
    }
    let expected = reference_observation(&saved.input);
    for (backend, observations) in [
        ("interpreter", &saved.backend_runs.interpreter),
        ("eager native", &saved.backend_runs.eager_native),
        ("demand candidate", &saved.backend_runs.demand_candidate),
    ] {
        for (repeat, observation) in observations.iter().enumerate() {
            let observation = observation
                .as_ref()
                .ok_or_else(|| format!("missing {backend} result repeat {repeat}"))?;
            compare(&expected, observation)?;
        }
    }
    Ok(())
}

fn replay_saved_case(saved: &SavedCase) -> Result<String, String> {
    validate_input(&saved.input)?;
    let repeated = observe_repeated(&saved.input);
    match &saved.failure_signature {
        Some(expected_signature) => {
            if expected_signature.is_empty() || expected_signature != &repeated.1 {
                return Err(format!(
                    "saved mismatch signature was not reproduced: expected={expected_signature:?}, actual={:?}",
                    repeated.1
                ));
            }
            Ok(format!("mismatch reproduced: {expected_signature:?}"))
        }
        None => {
            validate_saved_observations(saved)?;
            if !repeated.1.is_empty() || repeated.0 != saved.backend_runs {
                return Err("successful saved-case observations changed on replay".into());
            }
            Ok("saved successful input replayed twice".into())
        }
    }
}

fn observe_repeated(input: &Input) -> (BackendRuns, Vec<String>) {
    let mut references = [None, None];
    let mut eager = [None, None];
    let mut candidate = [None, None];
    let mut signature = Vec::new();
    let mut errors = Vec::new();
    for repeat in 0..2 {
        let expected = reference_observation(input);
        match run_backend(input, false) {
            Ok(actual) => {
                if let Err(class) = compare(&expected, &actual) {
                    signature.push(format!("eager[{repeat}]:{class}"));
                }
                eager[repeat] = Some(actual);
            }
            Err(error) => {
                signature.push(format!("eager[{repeat}]:error"));
                errors.push(error);
            }
        }
        match run_backend(input, true) {
            Ok(actual) => {
                if let Err(class) = compare(&expected, &actual) {
                    signature.push(format!("candidate[{repeat}]:{class}"));
                }
                candidate[repeat] = Some(actual);
            }
            Err(error) => {
                signature.push(format!("candidate[{repeat}]:error"));
                errors.push(error);
            }
        }
        references[repeat] = Some(expected);
    }
    if references[0] != references[1] {
        signature.push("interpreter[0..2]:repeat".into());
    }
    if eager[0].is_some() && eager[0] != eager[1] {
        signature.push("eager[0..2]:repeat".into());
    }
    if candidate[0].is_some() && candidate[0] != candidate[1] {
        signature.push("candidate[0..2]:repeat".into());
    }
    (
        BackendRuns {
            interpreter: references,
            eager_native: eager,
            demand_candidate: candidate,
            errors,
        },
        signature,
    )
}

/// Single source of the observation corpus: every generated input is admitted,
/// digested and run twice on all backends (about one second). Used by
/// `generated_corpus_runs_twice_across_all_backends` and by every test that
/// needs the corpus fixture.
fn generate_corpus_observations() -> CorpusObservationsEnvelope {
    let mut code_digest = 0xcbf2_9ce4_8422_2325u64;
    let mut cases = Vec::with_capacity(SEEDS.len() * CASES_PER_SEED);
    for seed in SEEDS {
        for index in 0..CASES_PER_SEED {
            let input = generate_case(seed, index);
            validate_input(&input)
                .unwrap_or_else(|error| panic!("seed={seed:#x} index={index}: {error}"));
            for byte in &canonical_input(&input) {
                code_digest = (code_digest ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3);
            }
            let backend_runs = run_repeated(&input);
            cases.push(CorpusObservation {
                input,
                backend_runs,
            });
        }
    }
    let mut envelope = CorpusObservationsEnvelope {
        version: ENVELOPE_VERSION,
        profile: PROFILE.into(),
        inputs_digest: format!("fnv1a64:{code_digest:016x}"),
        cases,
        envelope_digest: String::new(),
    };
    envelope.envelope_digest = corpus_observations_digest(
        envelope.version,
        &envelope.profile,
        &envelope.inputs_digest,
        &envelope.cases,
    );
    envelope
}

/// The saved case is the first generated input with its observations.
fn saved_case_from_corpus(corpus: &CorpusObservationsEnvelope, dir: &Path) -> SavedCase {
    let first = &corpus.cases[0];
    SavedCase {
        input: first.input.clone(),
        backend_runs: first.backend_runs.clone(),
        failure_signature: None,
        reproduction_command: format!(
            "OD_JIT_NATIVE_REGISTER_V1_CORPUS_DIR={} OD_JIT_NATIVE_REGISTER_V1_REPLAY_CASE={} cargo test --offline -p od-host --lib {REPLAY_GATE} -- --exact --test-threads=1",
            dir.display(),
            dir.join("native-register-v1-saved-case.json").display()
        ),
    }
}

/// Returns the observation corpus fixture. The ignored `work/` artifact is used
/// when it exists; otherwise the corpus is generated deterministically and
/// saved, so tests do not depend on test order or on files left by an earlier
/// run. Fixture consumers must call this before reading the artifact directory.
fn load_or_generate_corpus_observations() -> CorpusObservationsEnvelope {
    let corpus_path = artifact_dir().join("native-register-v1-observations.json");
    let corpus = match fs::read(&corpus_path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .unwrap_or_else(|error| panic!("source corpus observations malformed: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let corpus = generate_corpus_observations();
            write_json(&corpus_path, &corpus);
            corpus
        }
        Err(error) => panic!("source corpus observations unreadable: {error}"),
    };
    validate_corpus_observations(&corpus).unwrap();
    corpus
}

/// Returns the saved-case path: an explicit environment path is used as is
/// (it must exist); the default `work/` artifact is generated when missing.
fn saved_case_path(env_name: &str) -> PathBuf {
    if let Some(path) = std::env::var_os(env_name) {
        return PathBuf::from(path);
    }
    let dir = artifact_dir();
    let path = dir.join("native-register-v1-saved-case.json");
    if !path.exists() {
        let corpus = load_or_generate_corpus_observations();
        write_json(&path, &saved_case_from_corpus(&corpus, &dir));
    }
    path
}

#[test]
fn generated_corpus_runs_twice_across_all_backends() {
    let _guard = jit_test_lock();
    let mut unique = std::collections::BTreeSet::new();
    let mut form_counts = [0usize; 13];
    let mut anchor_taken = 0usize;
    let mut anchor_fallthrough = 0usize;
    let mut code_end_exits = 0usize;
    let mut budget_exits = 0usize;
    let mut stop_address_exits = 0usize;
    let mut stop_requested_exits = 0usize;
    let mut fallback_retired = 0u64;
    let mut bounded_loop_inputs = 0usize;
    let mut zero_budget_inputs = 0usize;
    let dir = artifact_dir();
    let corpus_envelope = generate_corpus_observations();
    for (position, case) in corpus_envelope.cases.iter().enumerate() {
        let (seed, index) = (SEEDS[position / CASES_PER_SEED], position % CASES_PER_SEED);
        let (input, runs) = (&case.input, &case.backend_runs);
        if index != 0 && index % 8 == 0 {
            bounded_loop_inputs += 1;
        }
        if input.budget == 0 {
            zero_budget_inputs += 1;
        }
        assert!(
            unique.insert(canonical_input(input)),
            "duplicate full generated input seed={seed:#x} index={index}"
        );
        for form in 0..13 {
            if contains_form(&input.code, form) {
                form_counts[form] += 1;
            }
        }
        let first = runs.interpreter[0].as_ref().unwrap();
        match first.exit.as_str() {
            "CodeEnd" => code_end_exits += 1,
            "BudgetExhausted" => budget_exits += 1,
            "StopAddress" => stop_address_exits += 1,
            "StopRequested" => stop_requested_exits += 1,
            other => panic!("unexpected expected exit {other}"),
        }
        fallback_retired += first.retired_interpreter;
        if index == 0 {
            if first.gpr[0] != 0x1122_3344_5566_7788 {
                anchor_taken += 1;
            } else {
                anchor_fallthrough += 1;
            }
        }
    }
    assert_eq!(unique.len(), 1200);
    assert!(
        form_counts.iter().all(|count| *count > 0),
        "form coverage {form_counts:?}"
    );
    let inputs_digest = corpus_envelope.inputs_digest.clone();
    write_json(
        &dir.join("native-register-v1-saved-case.json"),
        &saved_case_from_corpus(&corpus_envelope, &dir),
    );
    validate_corpus_observations(&corpus_envelope).unwrap();
    write_json(
        &dir.join("native-register-v1-observations.json"),
        &corpus_envelope,
    );
    #[derive(Serialize)]
    struct Summary {
        profile: &'static str,
        version: u32,
        unique_inputs: usize,
        seeds: [u64; 3],
        cases_per_seed: usize,
        repeats_per_backend: usize,
        full_partial_preserve_xchg_branch_anchors: usize,
        rip_relative_lea_after_variable_predecessor_cases: usize,
        anchor_branch_taken: usize,
        anchor_branch_fallthrough: usize,
        code_end_exits: usize,
        budget_exits: usize,
        stop_address_exits: usize,
        stop_requested_exits: usize,
        fallback_instructions_retired: u64,
        bounded_loop_inputs: usize,
        zero_budget_inputs: usize,
        rip_predecessor_lengths: [u8; 3],
        form_counts: [usize; 13],
        canonical_inputs_digest: String,
    }
    write_json(
        &dir.join("native-register-v1-summary.json"),
        &Summary {
            profile: PROFILE,
            version: ENVELOPE_VERSION,
            unique_inputs: unique.len(),
            seeds: SEEDS,
            cases_per_seed: CASES_PER_SEED,
            repeats_per_backend: 2,
            full_partial_preserve_xchg_branch_anchors: 3,
            rip_relative_lea_after_variable_predecessor_cases: 1197,
            anchor_branch_taken: anchor_taken,
            anchor_branch_fallthrough: anchor_fallthrough,
            code_end_exits,
            budget_exits,
            stop_address_exits,
            stop_requested_exits,
            fallback_instructions_retired: fallback_retired,
            bounded_loop_inputs,
            zero_budget_inputs,
            rip_predecessor_lengths: [3, 4, 7],
            form_counts,
            canonical_inputs_digest: inputs_digest.clone(),
        },
    );
    assert!(anchor_taken > 0 && anchor_fallthrough > 0);
    assert!(bounded_loop_inputs == 147 && zero_budget_inputs > 0);
    assert!(
        code_end_exits > 0
            && budget_exits > 0
            && stop_address_exits > 0
            && stop_requested_exits > 0
    );
    eprintln!(
        "S3-34n corpus: profile={PROFILE} unique=1200 seeds={SEEDS:?} repeats=2 forms={form_counts:?} canonical_digest={inputs_digest}"
    );
}

fn cached_corpus_segment(
    corpus: &CorpusObservationsEnvelope,
    strategy: &str,
) -> Result<CacheTraceSegment, String> {
    let candidate = match strategy {
        "eager" => false,
        "demand_candidate" => true,
        other => return Err(format!("unsupported cache trace strategy {other}")),
    };
    let mut cache = od_jit::dispatch::SingleEntryBlockCache::new(
        1,
        od_jit::dispatch::MAX_SINGLE_ENTRY_CACHE_BYTES,
    )
    .map_err(|error| format!("cannot create cache corpus: {error:?}"))?;
    let mut steps = Vec::with_capacity(corpus.cases.len());
    for (sequence, case) in corpus.cases.iter().enumerate() {
        let interpreter = case.backend_runs.interpreter[0]
            .clone()
            .expect("validated corpus has interpreter observation");
        let uncached = if candidate {
            case.backend_runs.demand_candidate[0].clone()
        } else {
            case.backend_runs.eager_native[0].clone()
        }
        .expect("validated corpus has matching uncached observation");
        let cache_before = cache_counter_snapshot(&cache);
        let (cached, cache_after) = match run_cached_backend(&case.input, candidate, &mut cache) {
            Ok(result) => result,
            Err(error) => {
                return Err(save_cache_trace_failure(
                    &corpus.inputs_digest,
                    "generated_corpus",
                    strategy,
                    sequence,
                    &steps,
                    &case.input,
                    &interpreter,
                    &uncached,
                    None,
                    cache_before,
                    cache_counter_snapshot(&cache),
                    "backend_error",
                    &error,
                ));
            }
        };
        if let Err(class) = compare(&interpreter, &cached) {
            return Err(save_cache_trace_failure(
                &corpus.inputs_digest,
                "generated_corpus",
                strategy,
                sequence,
                &steps,
                &case.input,
                &interpreter,
                &uncached,
                Some(cached),
                cache_before,
                cache_after,
                "interpreter_mismatch",
                &class,
            ));
        }
        if let Err(class) = compare(&uncached, &cached) {
            return Err(save_cache_trace_failure(
                &corpus.inputs_digest,
                "generated_corpus",
                strategy,
                sequence,
                &steps,
                &case.input,
                &interpreter,
                &uncached,
                Some(cached),
                cache_before,
                cache_after,
                "uncached_mismatch",
                &class,
            ));
        }
        let (_, _, bindings) = cache_bindings(&case.input);
        steps.push(CacheTraceStep {
            sequence_index: sequence,
            input: case.input.clone(),
            bindings,
            interpreter,
            uncached,
            cached,
            cache: cache_after,
        });
    }
    cache.clear();
    if cache.resident_entries() != 0 {
        return Err("cached corpus retained an executable handle after clear".into());
    }
    Ok(CacheTraceSegment {
        kind: "generated_corpus".into(),
        strategy: strategy.into(),
        steps,
    })
}

fn retained_hit_segment(
    strategy: &str,
    source_inputs_digest: &str,
) -> Result<CacheTraceSegment, String> {
    let candidate = match strategy {
        "eager" => false,
        "demand_candidate" => true,
        other => return Err(format!("unsupported cache trace strategy {other}")),
    };
    let inputs = retained_hit_inputs();
    let mut baselines = Vec::with_capacity(inputs.len());
    for (sequence, input) in inputs.iter().enumerate() {
        let interpreter = reference_observation(input);
        let uncached = run_backend(input, candidate).map_err(|error| {
            format!("retained {strategy} step {sequence} uncached executor: {error}")
        })?;
        compare(&interpreter, &uncached)
            .map_err(|class| format!("retained uncached {strategy} mismatch: {class}"))?;
        baselines.push((interpreter, uncached));
    }
    let mut cache = od_jit::dispatch::SingleEntryBlockCache::new(
        1,
        od_jit::dispatch::MAX_SINGLE_ENTRY_CACHE_BYTES,
    )
    .map_err(|error| format!("cannot create retained cache: {error:?}"))?;
    let mut steps = Vec::with_capacity(2);
    for (sequence, (input, (interpreter, uncached))) in
        inputs.into_iter().zip(baselines).enumerate()
    {
        let cache_before = cache_counter_snapshot(&cache);
        let (cached, cache_snapshot) = match run_cached_backend(&input, candidate, &mut cache) {
            Ok(result) => result,
            Err(error) => {
                return Err(save_cache_trace_failure(
                    source_inputs_digest,
                    "retained_hit",
                    strategy,
                    sequence,
                    &steps,
                    &input,
                    &interpreter,
                    &uncached,
                    None,
                    cache_before,
                    cache_counter_snapshot(&cache),
                    "backend_error",
                    &error,
                ));
            }
        };
        if let Err(class) = compare(&interpreter, &cached) {
            return Err(save_cache_trace_failure(
                source_inputs_digest,
                "retained_hit",
                strategy,
                sequence,
                &steps,
                &input,
                &interpreter,
                &uncached,
                Some(cached),
                cache_before,
                cache_snapshot,
                "interpreter_mismatch",
                &class,
            ));
        }
        if let Err(class) = compare(&uncached, &cached) {
            return Err(save_cache_trace_failure(
                source_inputs_digest,
                "retained_hit",
                strategy,
                sequence,
                &steps,
                &input,
                &interpreter,
                &uncached,
                Some(cached),
                cache_before,
                cache_snapshot,
                "uncached_mismatch",
                &class,
            ));
        }
        if let Some(detail) = retained_hit_counter_mismatch(sequence, &cache_snapshot) {
            return Err(save_cache_trace_failure(
                source_inputs_digest,
                "retained_hit",
                strategy,
                sequence,
                &steps,
                &input,
                &interpreter,
                &uncached,
                Some(cached),
                cache_before,
                cache_snapshot,
                "cache_counter_mismatch",
                &detail,
            ));
        }
        let (_, _, bindings) = cache_bindings(&input);
        steps.push(CacheTraceStep {
            sequence_index: sequence,
            input,
            bindings,
            interpreter,
            uncached,
            cached,
            cache: cache_snapshot,
        });
    }
    cache.clear();
    if cache.resident_entries() != 0 {
        return Err("retained-hit trace did not release its executable handle".into());
    }
    Ok(CacheTraceSegment {
        kind: "retained_hit".into(),
        strategy: strategy.into(),
        steps,
    })
}

fn build_cache_trace(corpus: &CorpusObservationsEnvelope) -> CacheTraceEnvelope {
    let segments = vec![
        cached_corpus_segment(corpus, "eager").unwrap(),
        cached_corpus_segment(corpus, "demand_candidate").unwrap(),
        retained_hit_segment("eager", &corpus.inputs_digest).unwrap(),
        retained_hit_segment("demand_candidate", &corpus.inputs_digest).unwrap(),
    ];
    finish_cache_trace(corpus.inputs_digest.clone(), segments)
}

#[test]
fn cached_corpus_matches_existing_observations_and_saves_versioned_trace() {
    let _guard = jit_test_lock();
    let corpus = load_or_generate_corpus_observations();
    let trace = build_cache_trace(&corpus);
    validate_cache_trace(&trace, &corpus).unwrap();
    let path = artifact_dir().join("native-register-v1-cache-trace.json");
    write_json(&path, &trace);
    eprintln!(
        "S3-34p cached trace: version={} profile={} generated_steps=1200 eager_hits={} candidate_hits={} retained_hits=1+1 source_digest={} trace_digest={} artifact={}",
        trace.version,
        trace.profile,
        trace.segments[0].steps.last().unwrap().cache.hits,
        trace.segments[1].steps.last().unwrap().cache.hits,
        trace.source_inputs_digest,
        trace.trace_digest,
        path.display(),
    );
}

#[test]
fn cached_trace_replays_from_empty_cache_and_rejects_corruption() {
    let _guard = jit_test_lock();
    let corpus = load_or_generate_corpus_observations();
    let path = artifact_dir().join("native-register-v1-cache-trace.json");
    let trace: CacheTraceEnvelope = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .unwrap_or_else(|error| panic!("cache trace malformed: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let trace = build_cache_trace(&corpus);
            write_json(&path, &trace);
            trace
        }
        Err(error) => panic!("cache trace unreadable at {}: {error}", path.display()),
    };
    validate_cache_trace(&trace, &corpus).unwrap();

    let mut changed_input = trace.clone();
    changed_input.segments[0].steps[0].input.initial_gpr[0] ^= 1;
    changed_input.trace_digest = cache_trace_digest(&changed_input);
    assert!(
        validate_cache_trace(&changed_input, &corpus)
            .unwrap_err()
            .contains("input digest mismatch")
    );

    let mut changed_counter = trace.clone();
    changed_counter.segments[0].steps[0].cache.hits += 1;
    changed_counter.trace_digest = cache_trace_digest(&changed_counter);
    let changed_counter_error = replay_cache_trace(&changed_counter).unwrap_err();
    assert!(
        changed_counter_error.contains("counter"),
        "counter tamper rejection had unexpected reason: {changed_counter_error}"
    );

    let mut missing_step = trace.clone();
    missing_step.segments[0].steps.pop();
    missing_step.trace_digest = cache_trace_digest(&missing_step);
    assert!(
        validate_cache_trace(&missing_step, &corpus)
            .unwrap_err()
            .contains("step count differs")
    );

    let mut unknown_field = serde_json::to_value(&trace).unwrap();
    unknown_field["unrecognized"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<CacheTraceEnvelope>(unknown_field).is_err());

    replay_cache_trace(&trace).unwrap_or_else(|error| panic!("cache trace replay failed: {error}"));
    eprintln!(
        "S3-34p cache trace replay passed: segments={} generated_per_strategy=1200 retained_hits=2 digest={}",
        trace.segments.len(),
        trace.trace_digest,
    );
}

#[test]
fn cached_failure_trace_replays_saved_prefix_and_failure() {
    let _guard = jit_test_lock();
    if let Some(path) = std::env::var_os("OD_JIT_NATIVE_REGISTER_V1_FAILURE_TRACE") {
        replay_cache_trace_failure(Path::new(&path), None)
            .unwrap_or_else(|error| panic!("saved cache failure did not replay: {error}"));
        return;
    }

    let corpus = load_or_generate_corpus_observations();
    let prefix_case = &corpus.cases[0];
    let case = &corpus.cases[1];
    let mut cache = od_jit::dispatch::SingleEntryBlockCache::new(
        1,
        od_jit::dispatch::MAX_SINGLE_ENTRY_CACHE_BYTES,
    )
    .unwrap();
    let prefix_interpreter = prefix_case.backend_runs.interpreter[0].clone().unwrap();
    let prefix_uncached = prefix_case.backend_runs.eager_native[0].clone().unwrap();
    let (prefix_cached, prefix_after) =
        run_cached_backend(&prefix_case.input, false, &mut cache).unwrap();
    let (_, _, prefix_bindings) = cache_bindings(&prefix_case.input);
    let prefix = CacheTraceStep {
        sequence_index: 0,
        input: prefix_case.input.clone(),
        bindings: prefix_bindings,
        interpreter: prefix_interpreter,
        uncached: prefix_uncached,
        cached: prefix_cached,
        cache: prefix_after,
    };
    let before = cache_counter_snapshot(&cache);
    let (cached, after) = run_cached_backend(&case.input, false, &mut cache).unwrap();
    cache.clear();
    assert_eq!(cache.resident_entries(), 0);

    // This local fault injection exercises the failure consumer without reporting
    // the deliberately altered observation as a JIT defect.
    let mut injected = cached;
    injected.gpr[0] ^= 1;
    let interpreter = case.backend_runs.interpreter[0].clone().unwrap();
    let uncached = case.backend_runs.eager_native[0].clone().unwrap();
    let failure_detail = compare(&interpreter, &injected).unwrap_err();
    let mut failure = CacheTraceFailureEnvelope {
        version: CACHE_TRACE_VERSION,
        profile: PROFILE.into(),
        source_inputs_digest: corpus.inputs_digest.clone(),
        segment_kind: "generated_corpus".into(),
        strategy: "eager".into(),
        sequence_index: 1,
        successful_prefix: vec![prefix],
        failing_input: case.input.clone(),
        interpreter,
        uncached,
        cached: Some(injected.clone()),
        cache_before: before,
        cache_after: after,
        failure_kind: "interpreter_mismatch".into(),
        failure_detail,
        reproduction_command: "local selfcheck injection".into(),
        failure_digest: String::new(),
    };
    failure.failure_digest = cache_trace_failure_digest(&failure);
    let path = std::env::temp_dir().join(format!(
        "native-register-v1-cache-failure-selfcheck-{}.json",
        std::process::id()
    ));
    let _ = fs::remove_file(&path);
    write_json_create_new(&path, &failure).unwrap();
    replay_cache_trace_failure(&path, Some(injected.clone())).unwrap();
    assert!(
        replay_cache_trace_failure(&path, None)
            .unwrap_err()
            .contains("did not reproduce")
    );

    let mut corrupted = failure;
    corrupted.failure_digest = "fnv1a64:0000000000000000".into();
    write_json(&path, &corrupted);
    assert!(
        replay_cache_trace_failure(&path, Some(injected))
            .unwrap_err()
            .contains("digest")
    );
    fs::remove_file(path).unwrap();

    let retained_inputs = retained_hit_inputs();
    let retained_baselines: Vec<_> = retained_inputs
        .iter()
        .map(|input| {
            (
                reference_observation(&input),
                run_backend(&input, false).unwrap(),
            )
        })
        .collect();
    let mut retained_cache = od_jit::dispatch::SingleEntryBlockCache::new(
        1,
        od_jit::dispatch::MAX_SINGLE_ENTRY_CACHE_BYTES,
    )
    .unwrap();
    let (first_cached, first_counters) =
        run_cached_backend(&retained_inputs[0], false, &mut retained_cache).unwrap();
    let (_, _, first_bindings) = cache_bindings(&retained_inputs[0]);
    let retained_prefix = CacheTraceStep {
        sequence_index: 0,
        input: retained_inputs[0].clone(),
        bindings: first_bindings,
        interpreter: retained_baselines[0].0.clone(),
        uncached: retained_baselines[0].1.clone(),
        cached: first_cached,
        cache: first_counters,
    };
    let retained_before = cache_counter_snapshot(&retained_cache);
    let (retained_cached, retained_after) =
        run_cached_backend(&retained_inputs[1], false, &mut retained_cache).unwrap();
    retained_cache.clear();
    assert_eq!(retained_cache.resident_entries(), 0);
    let mut retained_injected = retained_cached;
    retained_injected.gpr[0] ^= 1;
    let retained_failure_detail =
        compare(&retained_baselines[1].0, &retained_injected).unwrap_err();
    let mut retained_failure = CacheTraceFailureEnvelope {
        version: CACHE_TRACE_VERSION,
        profile: PROFILE.into(),
        source_inputs_digest: corpus.inputs_digest,
        segment_kind: "retained_hit".into(),
        strategy: "eager".into(),
        sequence_index: 1,
        successful_prefix: vec![retained_prefix],
        failing_input: retained_inputs[1].clone(),
        interpreter: retained_baselines[1].0.clone(),
        uncached: retained_baselines[1].1.clone(),
        cached: Some(retained_injected.clone()),
        cache_before: retained_before,
        cache_after: retained_after,
        failure_kind: "interpreter_mismatch".into(),
        failure_detail: retained_failure_detail,
        reproduction_command: "local retained-hit selfcheck injection".into(),
        failure_digest: String::new(),
    };
    retained_failure.failure_digest = cache_trace_failure_digest(&retained_failure);
    let retained_path = std::env::temp_dir().join(format!(
        "native-register-v1-retained-failure-selfcheck-{}.json",
        std::process::id()
    ));
    let _ = fs::remove_file(&retained_path);
    write_json_create_new(&retained_path, &retained_failure).unwrap();
    replay_cache_trace_failure(&retained_path, Some(retained_injected.clone())).unwrap();
    assert!(
        replay_cache_trace_failure(&retained_path, None)
            .unwrap_err()
            .contains("did not reproduce")
    );
    fs::remove_file(retained_path).unwrap();
}

#[test]
fn retained_hit_failure_reducer_and_selfcheck() {
    let _guard = jit_test_lock();
    if let Some(path) = std::env::var_os("OD_JIT_NATIVE_REGISTER_V1_REDUCE_CACHE_FAILURE") {
        let source_path = PathBuf::from(path);
        let output_dir = std::env::var_os("OD_JIT_NATIVE_REGISTER_V1_REDUCER_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| artifact_dir().join("native-register-v1-retained-hit-reducer"));
        let artifact = run_retained_hit_reducer(
            &source_path,
            &output_dir,
            REDUCER_MAX_EVALUATIONS,
            |_, actual| Ok(actual),
        )
        .unwrap_or_else(|error| panic!("retained-hit reducer rejected source: {error}"));
        let _ = artifact;
        return;
    }
    if let Some(path) = std::env::var_os("OD_JIT_NATIVE_REGISTER_V1_REPLAY_RETAINED_HIT_REDUCTION")
    {
        let mut identity = |_: &Input, actual: Observation| Ok(actual);
        replay_retained_hit_reduction(Path::new(&path), &mut identity)
            .unwrap_or_else(|error| panic!("saved retained-hit reduction did not replay: {error}"));
        return;
    }

    // This labeled injection is input-dependent and exercises the reduction
    // and replay path only; it is not evidence of a JIT defect.
    let source_dir = std::env::temp_dir().join(format!(
        "native-register-v1-retained-reducer-selfcheck-{}",
        std::process::id()
    ));
    let source_path = source_dir.join("injected-source-failure.json");
    let output_dir = source_dir.join("output");
    let _ = fs::remove_dir_all(&source_dir);
    fs::create_dir_all(&source_dir).unwrap();
    let mut injected = |input: &Input, mut actual: Observation| {
        if input.initial_gpr[3] != 0 {
            actual.gpr[0] ^= 2;
        }
        Ok(actual)
    };
    let baseline = retained_hit_trial(&retained_hit_inputs()[1], "eager", &mut injected).unwrap();
    assert_eq!(
        baseline.signature.as_deref(),
        Some("interpreter_mismatch:gpr")
    );
    load_or_generate_corpus_observations();
    let source = load_source_observations().unwrap();
    let inputs = retained_hit_inputs();
    let mut failure = CacheTraceFailureEnvelope {
        version: CACHE_TRACE_VERSION,
        profile: PROFILE.into(),
        source_inputs_digest: source.inputs_digest,
        segment_kind: "retained_hit".into(),
        strategy: "eager".into(),
        sequence_index: 1,
        successful_prefix: vec![baseline.warmup.clone()],
        failing_input: inputs[1].clone(),
        interpreter: reference_observation(&inputs[1]),
        uncached: run_backend(&inputs[1], false).unwrap(),
        cached: baseline.cached.clone(),
        cache_before: baseline.warmup.cache.clone(),
        cache_after: {
            let mut after = baseline.warmup.cache.clone();
            after.hits += 1;
            after
        },
        failure_kind: "interpreter_mismatch".into(),
        failure_detail: "gpr".into(),
        reproduction_command: "LOCAL INPUT-DEPENDENT SELF-CHECK ONLY".into(),
        failure_digest: String::new(),
    };
    failure.failure_digest = cache_trace_failure_digest(&failure);
    write_json_create_new(&source_path, &failure).unwrap();
    let counter_only_path = source_dir.join("counter-only-source-failure.json");
    let mut counter_only_failure = failure.clone();
    counter_only_failure.failure_detail = "retirement".into();
    let mut counter_only_observation = counter_only_failure.interpreter.clone();
    counter_only_observation.retired_native ^= 1;
    counter_only_failure.cached = Some(counter_only_observation);
    counter_only_failure.failure_digest = cache_trace_failure_digest(&counter_only_failure);
    write_json_create_new(&counter_only_path, &counter_only_failure).unwrap();
    assert!(
        run_retained_hit_reducer(
            &counter_only_path,
            &source_dir.join("counter-only"),
            REDUCER_MAX_EVALUATIONS,
            |_, actual| Ok(actual),
        )
        .unwrap_err()
        .contains("semantic state mismatches")
    );
    let artifact = run_retained_hit_reducer(
        &source_path,
        &output_dir,
        REDUCER_MAX_EVALUATIONS,
        |input, actual| {
            let mut actual = actual;
            if input.initial_gpr[3] != 0 {
                actual.gpr[0] ^= 2;
            }
            Ok(actual)
        },
    )
    .unwrap()
    .expect("input-dependent mismatch must reduce");
    assert!(artifact.reduced_state_metric < artifact.original_state_metric);
    assert_eq!(artifact.original_replay_runs, 2);
    assert_eq!(artifact.reduced_replay_runs, 2);

    let artifact_path = output_dir.join("retained-hit-reduced-input-v1.json");
    let mut identity = |_: &Input, actual: Observation| Ok(actual);
    assert!(
        replay_retained_hit_reduction(&artifact_path, &mut identity)
            .unwrap_err()
            .contains("mismatch signature")
    );
    let mut no_hit = artifact.clone();
    no_hit.warmup.cache.hits = 1;
    no_hit.artifact_digest = retained_hit_reduction_digest(&no_hit);
    let no_hit_path = output_dir.join("negative-no-hit.json");
    write_json_create_new(&no_hit_path, &no_hit).unwrap();
    assert!(replay_retained_hit_reduction(&no_hit_path, &mut identity).is_err());
    let mut changed_prefix = artifact.clone();
    changed_prefix.warmup.input.initial_gpr[0] ^= 1;
    changed_prefix.warmup.input = finish_input(changed_prefix.warmup.input);
    changed_prefix.artifact_digest = retained_hit_reduction_digest(&changed_prefix);
    let prefix_path = output_dir.join("negative-prefix.json");
    write_json_create_new(&prefix_path, &changed_prefix).unwrap();
    assert!(replay_retained_hit_reduction(&prefix_path, &mut identity).is_err());
    let mut changed_context = artifact.clone();
    changed_context.reduced_failing_input.guest_base_rip += 0x1000;
    changed_context.reduced_failing_input = finish_input(changed_context.reduced_failing_input);
    changed_context.artifact_digest = retained_hit_reduction_digest(&changed_context);
    let context_path = output_dir.join("negative-context.json");
    write_json_create_new(&context_path, &changed_context).unwrap();
    assert!(replay_retained_hit_reduction(&context_path, &mut identity).is_err());
    let mut changed_code = artifact.clone();
    changed_code.reduced_failing_input.code[3] ^= 1;
    changed_code.reduced_failing_input = finish_input(changed_code.reduced_failing_input);
    changed_code.artifact_digest = retained_hit_reduction_digest(&changed_code);
    let code_path = output_dir.join("negative-code.json");
    write_json_create_new(&code_path, &changed_code).unwrap();
    assert!(replay_retained_hit_reduction(&code_path, &mut identity).is_err());
    let mut changed_source = artifact.clone();
    changed_source.source_inputs_digest = "fnv1a64:0000000000000000".into();
    changed_source.artifact_digest = retained_hit_reduction_digest(&changed_source);
    let source_change_path = output_dir.join("negative-source.json");
    write_json_create_new(&source_change_path, &changed_source).unwrap();
    assert!(replay_retained_hit_reduction(&source_change_path, &mut identity).is_err());
    let mut changed_signature = artifact.clone();
    changed_signature.mismatch_signature = "interpreter_mismatch:rip".into();
    changed_signature.artifact_digest = retained_hit_reduction_digest(&changed_signature);
    let signature_path = output_dir.join("negative-signature.json");
    write_json_create_new(&signature_path, &changed_signature).unwrap();
    assert!(replay_retained_hit_reduction(&signature_path, &mut identity).is_err());
    let tampered_path = output_dir.join("negative-digest.json");
    let mut tampered = artifact.clone();
    tampered.artifact_digest = "fnv1a64:0000000000000000".into();
    write_json_create_new(&tampered_path, &tampered).unwrap();
    assert!(
        replay_retained_hit_reduction(&tampered_path, &mut identity)
            .unwrap_err()
            .contains("digest")
    );
    let unstable_toggle = AtomicBool::new(false);
    let unstable_result = run_retained_hit_reducer(
        &source_path,
        &source_dir.join("unstable"),
        REDUCER_MAX_EVALUATIONS,
        |input, mut actual| {
            if input.initial_gpr[3] != 0 && unstable_toggle.fetch_xor(true, Ordering::Relaxed) {
                actual.gpr[0] ^= 2;
            }
            Ok(actual)
        },
    );
    assert!(
        unstable_result
            .unwrap_err()
            .contains("stable across two exact retained-hit replays")
    );
    let same_class_variation = AtomicU8::new(0);
    let canonical_failing_input = retained_hit_inputs()[1].clone();
    let same_class_output = source_dir.join("same-class-variation");
    let same_class_result = run_retained_hit_reducer(
        &source_path,
        &same_class_output,
        REDUCER_MAX_EVALUATIONS,
        |input, mut actual| {
            if input == &canonical_failing_input {
                actual.gpr[0] ^= 2;
            } else if input.initial_gpr[3] != 0 {
                actual.gpr[0] ^= if same_class_variation.fetch_add(1, Ordering::Relaxed) % 2 == 0 {
                    2
                } else {
                    4
                };
            }
            Ok(actual)
        },
    );
    assert!(same_class_result.unwrap().is_none());
    let same_class_status: RetainedHitReductionStatus = serde_json::from_slice(
        &fs::read(same_class_output.join("retained-hit-reducer-status-v1.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(same_class_status.outcome, "not_reproduced");
    assert!(
        !same_class_output
            .join("retained-hit-reduced-input-v1.json")
            .exists()
    );
    let backend_error_result = run_retained_hit_reducer(
        &source_path,
        &source_dir.join("backend-error"),
        REDUCER_MAX_EVALUATIONS,
        |_, _| Err("injected backend error".into()),
    );
    assert!(
        backend_error_result
            .unwrap_err()
            .contains("injected backend error")
    );
    let limited_dir = source_dir.join("limited");
    let limited = run_retained_hit_reducer(&source_path, &limited_dir, 1, |input, mut actual| {
        if input.initial_gpr[3] != 0 {
            actual.gpr[0] ^= 2;
        }
        Ok(actual)
    })
    .unwrap()
    .expect("limited reducer still persists its reproduced candidate");
    assert_eq!(
        limited.reduced_state_metric < limited.original_state_metric,
        true
    );
    let limited_status: RetainedHitReductionStatus = serde_json::from_slice(
        &fs::read(limited_dir.join("retained-hit-reducer-status-v1.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(limited_status.outcome, "evaluation_limit");
    assert_eq!(limited_status.evaluations, 1);
    assert_eq!(
        limited_status.status_digest,
        retained_hit_reduction_status_digest(&limited_status)
    );
    let not_reproduced_dir = source_dir.join("not-reproduced");
    let canonical_failing_input = retained_hit_inputs()[1].clone();
    let not_reproduced = run_retained_hit_reducer(
        &source_path,
        &not_reproduced_dir,
        REDUCER_MAX_EVALUATIONS,
        |input, mut actual| {
            if input == &canonical_failing_input {
                actual.gpr[0] ^= 2;
            }
            Ok(actual)
        },
    )
    .unwrap();
    assert!(not_reproduced.is_none());
    let not_reproduced_status: RetainedHitReductionStatus = serde_json::from_slice(
        &fs::read(not_reproduced_dir.join("retained-hit-reducer-status-v1.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(not_reproduced_status.outcome, "not_reproduced");
    assert!(not_reproduced_status.reduced_artifact.is_none());
    let _ = fs::remove_dir_all(source_dir);
}

fn contains_form(code: &[u8], form: usize) -> bool {
    let patterns: [&[u8]; 13] = [
        &[0x48, 0xff, 0xc0],
        &[0x48, 0xff, 0xc8],
        &[0x48, 0xf7, 0xd8],
        &[0x48, 0xf7, 0xd0],
        &[0x48, 0x83, 0xc0],
        &[0x48, 0x83, 0xf8],
        &[0x48, 0x83, 0xe8],
        &[0x48, 0xc7, 0xc0],
        &[0x48, 0x87, 0xd8],
        &[0x48, 0x8d, 0x03],
        &[0x48, 0x8d, 0x43],
        &[0x48, 0x8d, 0x04, 0x0b],
        &[0x48, 0x8d, 0x05],
    ];
    code.windows(patterns[form].len())
        .any(|window| window.starts_with(patterns[form]))
}

#[test]
fn saved_case_round_trip_and_replay() {
    let _guard = jit_test_lock();
    let path = saved_case_path("OD_JIT_NATIVE_REGISTER_V1_REPLAY_CASE");
    let original: SavedCase = serde_json::from_slice(
        &fs::read(&path)
            .unwrap_or_else(|error| panic!("saved case missing at {}: {error}", path.display())),
    )
    .unwrap();
    let round_trip = serde_json::to_vec(&original).unwrap();
    let restored: SavedCase = serde_json::from_slice(&round_trip).unwrap();
    assert_eq!(restored, original);
    let replay = replay_saved_case(&restored)
        .unwrap_or_else(|error| panic!("saved case replay failed: {error}"));
    eprintln!(
        "S3-34n saved-case replay passed: {} digest={} result={replay}",
        path.display(),
        restored.input.input_digest
    );
}

#[test]
fn comparator_and_envelope_fail_closed() {
    let _guard = jit_test_lock();
    let input = generate_case(SEEDS[0], 4);
    let expected = reference_observation(&input);
    let cases: [(&str, fn(&mut Observation)); 5] = [
        ("gpr", |value: &mut Observation| value.gpr[0] ^= 1),
        ("flags", |value: &mut Observation| value.rflags ^= 1),
        ("rip", |value: &mut Observation| value.rip ^= 1),
        ("exit", |value: &mut Observation| {
            value.exit = "Forged".into()
        }),
        ("retirement", |value: &mut Observation| {
            value.retired_native ^= 1
        }),
    ];
    for (class, corrupt) in cases {
        let mut actual = expected.clone();
        corrupt(&mut actual);
        assert_eq!(compare(&expected, &actual).unwrap_err(), class);
    }
    let mut bad_digest = input.clone();
    bad_digest.input_digest.push('0');
    assert!(validate_input(&bad_digest).is_err());
    let mut oversized_budget = input.clone();
    oversized_budget.budget = MAX_BUDGET + 1;
    oversized_budget = finish_input(oversized_budget);
    assert!(validate_input(&oversized_budget).is_err());
    let mut bad_profile = input.clone();
    bad_profile.profile = "Other".into();
    bad_profile = finish_input(bad_profile);
    assert!(validate_input(&bad_profile).is_err());
    let mut bad_version = input.clone();
    bad_version.version = ENVELOPE_VERSION + 1;
    bad_version = finish_input(bad_version);
    assert!(validate_input(&bad_version).is_err());
    let mut unknown_field = serde_json::to_value(&input).unwrap();
    unknown_field["unrecognized"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<Input>(unknown_field).is_err());
    let mut malformed = input.clone();
    malformed.code.pop();
    malformed = finish_input(malformed);
    assert!(validate_input(&malformed).is_err());
    let mut unsupported = input;
    unsupported.code.splice(0..3, [0x48, 0x89, 0xc0]);
    unsupported = finish_input(unsupported);
    assert!(validate_input(&unsupported).is_err());
    let mut missing = BackendRuns {
        interpreter: [Some(expected.clone()), Some(expected.clone())],
        eager_native: [Some(expected.clone()), Some(expected.clone())],
        demand_candidate: [Some(expected.clone()), None],
        errors: Vec::new(),
    };
    let saved = SavedCase {
        input: generate_case(SEEDS[0], 4),
        backend_runs: missing.clone(),
        failure_signature: None,
        reproduction_command: "test".into(),
    };
    assert_eq!(
        validate_saved_observations(&saved).unwrap_err(),
        "missing demand candidate result repeat 1"
    );
    missing.demand_candidate[1] = Some(expected.clone());
    missing.errors.push("backend error: compile failed".into());
    missing.errors.push("timeout: demand candidate".into());
    let saved = SavedCase {
        input: generate_case(SEEDS[0], 4),
        backend_runs: missing,
        failure_signature: None,
        reproduction_command: "test".into(),
    };
    assert_eq!(
        validate_saved_observations(&saved).unwrap_err(),
        "backend result contains error/timeout"
    );
    let mut unconfirmed_mismatch = SavedCase {
        input: generate_case(SEEDS[0], 4),
        backend_runs: BackendRuns {
            interpreter: [Some(expected.clone()), Some(expected.clone())],
            eager_native: [Some(expected.clone()), Some(expected.clone())],
            demand_candidate: [Some(expected.clone()), Some(expected.clone())],
            errors: Vec::new(),
        },
        failure_signature: Some(vec!["candidate[0]:gpr".into()]),
        reproduction_command: "test".into(),
    };
    assert!(
        replay_saved_case(&unconfirmed_mismatch)
            .unwrap_err()
            .contains("not reproduced")
    );
    unconfirmed_mismatch.failure_signature = Some(Vec::new());
    assert!(
        replay_saved_case(&unconfirmed_mismatch)
            .unwrap_err()
            .contains("not reproduced")
    );
}

fn local_fault_injection_evaluation(input: &Input) -> (BackendRuns, Vec<String>) {
    let expected = reference_observation(input);
    let mut injected = expected.clone();
    injected.gpr[0] ^= 1;
    (
        BackendRuns {
            interpreter: [Some(expected.clone()), Some(expected.clone())],
            eager_native: [Some(expected.clone()), Some(expected)],
            demand_candidate: [Some(injected.clone()), Some(injected)],
            errors: Vec::new(),
        },
        vec!["candidate[0]:gpr".into(), "candidate[1]:gpr".into()],
    )
}

fn locally_injected_source(input: &Input) -> SavedCase {
    let (backend_runs, failure_signature) = local_fault_injection_evaluation(input);
    SavedCase {
        input: input.clone(),
        backend_runs,
        failure_signature: Some(failure_signature),
        reproduction_command: "LOCAL-FAULT-INJECTION SELF-CHECK ONLY".into(),
    }
}

fn local_fault_injection_fixture() -> Input {
    let base = 0x0000_0001_0000_0000;
    let code = vec![
        0x48, 0x83, 0xc0, 0xff, // ADD RAX, -1
        0x48, 0x8d, 0x43, 0x7f, // LEA RAX, [RBX + disp8]
        0x48, 0x8d, 0x05, 0x80, 0xff, 0xff, 0xff, // LEA RAX, [RIP - 128]
        0x90, // interpreter fallback
    ];
    let mut initial_gpr = [0; 16];
    initial_gpr[0] = u64::MAX;
    initial_gpr[1] = 0x0102_0304_0506_0708;
    initial_gpr[3] = 0x1234;
    finish_input(Input {
        version: ENVELOPE_VERSION,
        profile: PROFILE.into(),
        seed: 0,
        index: 0,
        code,
        guest_base_rip: base,
        initial_gpr,
        initial_rip: base,
        initial_rflags: 0x202 | REDUCER_FLAG_MASK,
        initial_auxiliary_state: default_auxiliary_state(),
        stop_rip: None,
        stop_requested: false,
        budget: MAX_BUDGET,
        input_digest: String::new(),
    })
}

#[test]
fn reduce_saved_native_register_v1_case() {
    let _guard = jit_test_lock();
    let path = saved_case_path("OD_JIT_NATIVE_REGISTER_V1_REDUCE_CASE");
    let source: SavedCase = serde_json::from_slice(&fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "saved reducer source missing at {}: {error}",
            path.display()
        )
    }))
    .unwrap();
    validate_input(&source.input).expect("saved reducer source must pass admission and digest");
    // persist_reduction refuses to overwrite, so a fixed directory makes every
    // rerun fail. Use a per-process directory unless the caller chose one.
    let output_dir = if std::env::var_os("OD_JIT_NATIVE_REGISTER_V1_CORPUS_DIR").is_some() {
        reducer_artifact_dir()
    } else {
        let dir = std::env::temp_dir().join(format!(
            "native-register-v1-saved-case-reducer-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    };
    let report = reduce_saved_case_with(&path, &source, &output_dir, |input| {
        Ok(observe_repeated(input))
    });
    persist_reduction(&report, &source, &output_dir)
        .unwrap_or_else(|error| panic!("could not persist reducer artifacts: {error}"));
    assert!(report.evaluated_count <= REDUCER_MAX_EVALUATIONS);
    assert_eq!(
        report.accepted_count + report.rejected_count,
        report.evaluated_count
    );
    assert_eq!(report.reduced_input.is_some(), report.accepted_count > 0);

    // This positive path deliberately substitutes only a local observation
    // predicate. It exercises reducer mechanics and is not JIT bug evidence.
    let injected = locally_injected_source(&local_fault_injection_fixture());
    let injection_dir = output_dir.join("local-fault-injection-selfcheck");
    let mut injected_report = reduce_saved_case_with(
        Path::new("LOCAL-FAULT-INJECTION SELF-CHECK ONLY"),
        &injected,
        &injection_dir,
        |input| Ok(local_fault_injection_evaluation(input)),
    );
    injected_report.kind = "LOCAL-FAULT-INJECTION SELF-CHECK ONLY; NOT REPLAY EVIDENCE".into();
    assert!(injected_report.accepted_count > 0);
    assert_eq!(
        injected_report.source_signature,
        injected.failure_signature.clone().unwrap()
    );
    assert_eq!(
        injected_report.reduced_signature,
        injected.failure_signature.clone()
    );
    persist_reduction(&injected_report, &injected, &injection_dir).unwrap();
    let reduced_path = injection_dir.join("reduced-saved-case.json");
    let replayed_case: SavedCase =
        serde_json::from_slice(&fs::read(&reduced_path).expect("reduced case artifact"))
            .expect("reduced case envelope");
    validate_input(&replayed_case.input).unwrap();
    assert_eq!(
        replayed_case.failure_signature,
        Some(injected_report.source_signature.clone())
    );
    let (replayed_runs, replayed_signature) =
        local_fault_injection_evaluation(&replayed_case.input);
    assert_eq!(replayed_signature, injected_report.source_signature);
    assert!(stable_semantic_mismatch(
        &replayed_runs,
        &replayed_signature
    ));
    let mut limited = reduce_saved_case_with_limit(
        Path::new("LOCAL-FAULT-INJECTION SELF-CHECK ONLY"),
        &injected,
        &injection_dir.join("limit"),
        1,
        |input| Ok(local_fault_injection_evaluation(input)),
    );
    limited.kind = "LOCAL-FAULT-INJECTION LIMIT SELF-CHECK ONLY".into();
    assert_eq!(limited.evaluated_count, 1);
    assert_eq!(limited.stop_reason, "evaluation_limit");
    assert_ne!(limited.stop_reason, "globally_minimal");
    persist_reduction(&limited, &injected, &injection_dir.join("limit")).unwrap();
    if std::env::var_os("OD_JIT_NATIVE_REGISTER_V1_CORPUS_DIR").is_none() {
        // Only the per-process scratch directory is removed; a caller-chosen
        // artifact directory is kept for inspection.
        let _ = fs::remove_dir_all(&output_dir);
    }
}

#[test]
fn native_register_v1_reducer_rejects_unstable_and_structural_changes() {
    let _guard = jit_test_lock();
    let source = locally_injected_source(&generate_case(SEEDS[0], 4));
    let directory = reducer_artifact_dir().join("selfcheck-negative");

    let base = 0x0000_0001_0000_0000;
    let zero_metric = finish_input(Input {
        version: ENVELOPE_VERSION,
        profile: PROFILE.into(),
        seed: 0,
        index: 0,
        code: vec![0x48, 0xff, 0xc0],
        guest_base_rip: base,
        initial_gpr: [0; 16],
        initial_rip: base,
        initial_rflags: 0x2,
        initial_auxiliary_state: default_auxiliary_state(),
        stop_rip: None,
        stop_requested: false,
        budget: 1,
        input_digest: String::new(),
    });
    validate_input(&zero_metric).unwrap();
    assert_eq!(reduction_metric(&zero_metric).unwrap(), 0);
    let zero_metric_report = reduce_saved_case_with_limit(
        Path::new("selfcheck-zero-metric"),
        &locally_injected_source(&zero_metric),
        &directory,
        REDUCER_MAX_EVALUATIONS,
        |input| Ok(local_fault_injection_evaluation(input)),
    );
    assert_eq!(zero_metric_report.stop_reason, "no_accepted_candidate");
    assert_eq!(zero_metric_report.evaluated_count, 0);
    assert!(zero_metric_report.reduced_input.is_none());

    let mut bad_digest = source.input.clone();
    bad_digest.input_digest.push('0');
    assert!(validate_input(&bad_digest).is_err());
    let mut bad_profile = source.input.clone();
    bad_profile.profile = "Other".into();
    bad_profile = finish_input(bad_profile);
    assert!(validate_input(&bad_profile).is_err());

    let mut changed_branch = source.input.clone();
    let branch_displacement = changed_branch.code.len() - 1;
    changed_branch.code[branch_displacement] = 0xfe;
    changed_branch = finish_input(changed_branch);
    assert!(validate_input(&changed_branch).is_ok());
    assert!(!is_value_only_candidate(&source.input, &changed_branch));

    let real_signature = source.failure_signature.clone().unwrap();
    for changed in ["candidate[0]:flags", "eager[0]:gpr"] {
        let mut calls = 0usize;
        let changed_signature =
            reduce_saved_case_with_limit(Path::new("selfcheck"), &source, &directory, 1, |input| {
                calls += 1;
                let (runs, mut signature) = local_fault_injection_evaluation(input);
                if calls > 1 {
                    signature = vec![changed.into()];
                }
                Ok((runs, signature))
            });
        assert_eq!(changed_signature.accepted_count, 0);
        assert_eq!(changed_signature.stop_reason, "evaluation_limit");
        assert!(
            changed_signature.attempts[0]
                .reason
                .contains("signature_changed")
        );
        assert_eq!(real_signature, changed_signature.source_signature);
    }

    for condition in ["missing", "unstable", "error"] {
        let mut calls = 0usize;
        let rejected =
            reduce_saved_case_with_limit(Path::new("selfcheck"), &source, &directory, 1, |input| {
                calls += 1;
                let (mut runs, signature) = local_fault_injection_evaluation(input);
                if calls == 1 {
                    match condition {
                        "missing" => runs.demand_candidate[1] = None,
                        "unstable" => runs.demand_candidate[1].as_mut().unwrap().gpr[1] ^= 1,
                        _ => runs.errors.push("injected backend error".into()),
                    }
                }
                Ok((runs, signature))
            });
        assert!(rejected.accepted_count == 0);
        assert_eq!(rejected.stop_reason, "source_unstable_or_nonsemantic");
        assert!(rejected.reduced_input.is_none());
    }

    let mut calls = 0usize;
    let candidate_rejection =
        reduce_saved_case_with_limit(Path::new("selfcheck"), &source, &directory, 1, |input| {
            calls += 1;
            let (runs, signature) = local_fault_injection_evaluation(input);
            if calls == 1 {
                Ok((runs, signature))
            } else {
                Err("injected timeout/crash".into())
            }
        });
    assert_eq!(candidate_rejection.accepted_count, 0);
    assert_eq!(candidate_rejection.evaluated_count, 1);
    assert_eq!(candidate_rejection.attempts[0].backend_runs.errors.len(), 1);
    assert!(candidate_rejection.reduced_input.is_none());
}
