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
