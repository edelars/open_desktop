use super::*;
use od_isa::memory::{AccessContext, AccessKind, GuestMemory};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU8;

const PROFILE: &str = "NativeRegisterV1";
const ENVELOPE_VERSION: u32 = 1;
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

#[test]
fn generated_corpus_runs_twice_across_all_backends() {
    let _guard = jit_test_lock();
    let mut unique = std::collections::BTreeSet::new();
    let mut form_counts = [0usize; 13];
    let mut code_digest = 0xcbf2_9ce4_8422_2325u64;
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
    let mut saved_sample = None;
    for seed in SEEDS {
        for index in 0..CASES_PER_SEED {
            let input = generate_case(seed, index);
            if index != 0 && index % 8 == 0 {
                bounded_loop_inputs += 1;
            }
            if input.budget == 0 {
                zero_budget_inputs += 1;
            }
            validate_input(&input)
                .unwrap_or_else(|error| panic!("seed={seed:#x} index={index}: {error}"));
            let bytes = canonical_input(&input);
            assert!(
                unique.insert(bytes.clone()),
                "duplicate full generated input seed={seed:#x} index={index}"
            );
            for byte in &bytes {
                code_digest = (code_digest ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3);
            }
            for form in 0..13 {
                if contains_form(&input.code, form) {
                    form_counts[form] += 1;
                }
            }
            let runs = run_repeated(&input);
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
                if runs.interpreter[0].as_ref().unwrap().gpr[0] != 0x1122_3344_5566_7788 {
                    anchor_taken += 1;
                } else {
                    anchor_fallthrough += 1;
                }
            }
            if saved_sample.is_none() {
                saved_sample = Some(SavedCase {
                    input: input.clone(),
                    backend_runs: runs.clone(),
                    failure_signature: None,
                    reproduction_command: format!(
                        "OD_JIT_NATIVE_REGISTER_V1_CORPUS_DIR={} OD_JIT_NATIVE_REGISTER_V1_REPLAY_CASE={} cargo test --offline -p od-host --lib {REPLAY_GATE} -- --exact --test-threads=1",
                        dir.display(),
                        dir.join("native-register-v1-saved-case.json").display()
                    ),
                });
            }
        }
    }
    assert_eq!(unique.len(), 1200);
    assert!(
        form_counts.iter().all(|count| *count > 0),
        "form coverage {form_counts:?}"
    );
    let sample = saved_sample.unwrap();
    write_json(&dir.join("native-register-v1-saved-case.json"), &sample);
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
            canonical_inputs_digest: format!("fnv1a64:{code_digest:016x}"),
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
        "S3-34n corpus: profile={PROFILE} unique=1200 seeds={SEEDS:?} repeats=2 forms={form_counts:?} canonical_digest=fnv1a64:{code_digest:016x}"
    );
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
    let path = std::env::var_os("OD_JIT_NATIVE_REGISTER_V1_REPLAY_CASE")
        .map(PathBuf::from)
        .unwrap_or_else(|| artifact_dir().join("native-register-v1-saved-case.json"));
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
    let path = std::env::var_os("OD_JIT_NATIVE_REGISTER_V1_REDUCE_CASE")
        .map(PathBuf::from)
        .unwrap_or_else(|| artifact_dir().join("native-register-v1-saved-case.json"));
    let source: SavedCase = serde_json::from_slice(&fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "saved reducer source missing at {}: {error}",
            path.display()
        )
    }))
    .unwrap();
    validate_input(&source.input).expect("saved reducer source must pass admission and digest");
    let output_dir = reducer_artifact_dir();
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
