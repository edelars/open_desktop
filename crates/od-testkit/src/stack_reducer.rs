//! Bounded schema-v2 reducer for exact one-instruction qword GPR PUSH/POP cases.

use crate::HexU64;
use crate::result::{Backend, MemoryRegionSnapshot, Observation};
use crate::sha256_hex;
use crate::v2::{
    ExpectedExit, Fixture, IntegerState, LoadedFixture, Mode, Outcome, Profile, RunResult,
};
use serde::{Deserialize, Serialize};

pub const STACK_REDUCER_DEFAULT_BUDGET: usize = 132;
pub const STACK_REDUCER_MAX_BUDGET: usize = 132;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StackOperation {
    Push { register: u8 },
    Pop { register: u8 },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StackMismatchFingerprint {
    /// Architectural fields/bytes that differ from the independent stack oracle.
    pub differing_locations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StackReductionStop {
    Reduced,
    NoReduction,
    NoInitialMismatch,
    FlakyInitial,
    FlakyCandidate,
    CandidateBudgetExhausted,
    CallbackError,
    FinalConfirmationFailed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StackExpectedResult {
    pub case_id: String,
    pub profile: Profile,
    pub input_sha256: String,
    pub outcome: Outcome,
    pub instructions_executed: HexU64,
    pub state: IntegerState,
    pub rflags_defined: HexU64,
    pub memory: Vec<MemoryRegionSnapshot>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StackReduction {
    pub stop: StackReductionStop,
    pub stop_detail: Option<String>,
    pub operation: StackOperation,
    pub original_fixture: Fixture,
    pub reduced_fixture: Option<Fixture>,
    pub initial_results: Vec<RunResult>,
    pub final_results: Vec<RunResult>,
    pub expected_original: StackExpectedResult,
    pub expected_reduced: Option<StackExpectedResult>,
    pub initial_fingerprint: Option<StackMismatchFingerprint>,
    pub final_fingerprint: Option<StackMismatchFingerprint>,
    pub actual_callbacks: usize,
    pub candidate_proposals: usize,
    pub candidate_runs: usize,
    pub accepted_candidates: usize,
    pub initial_hamming_weight: u32,
    pub final_hamming_weight: Option<u32>,
}

/// Reduce only the PUSH source register value or the eight bytes read by POP.
/// The caller owns execution; this crate neither launches nor selects a backend.
pub fn reduce_stack_fixture_with_input_digest<F>(
    original: &Fixture,
    original_input_sha256: &str,
    call_budget: usize,
    execute: F,
) -> Result<StackReduction, String>
where
    F: FnMut(&Fixture) -> Result<RunResult, String>,
{
    reduce_stack_fixture_with_backend(
        original,
        original_input_sha256,
        call_budget,
        Backend::Interpreter,
        execute,
    )
}

/// Native-x86 callback variant used by the isolated physical x86 runner.
pub fn reduce_stack_fixture_with_native_input_digest<F>(
    original: &Fixture,
    original_input_sha256: &str,
    call_budget: usize,
    execute: F,
) -> Result<StackReduction, String>
where
    F: FnMut(&Fixture) -> Result<RunResult, String>,
{
    reduce_stack_fixture_with_backend(
        original,
        original_input_sha256,
        call_budget,
        Backend::NativeX86,
        execute,
    )
}

fn reduce_stack_fixture_with_backend<F>(
    original: &Fixture,
    original_input_sha256: &str,
    call_budget: usize,
    expected_backend: Backend,
    mut execute: F,
) -> Result<StackReduction, String>
where
    F: FnMut(&Fixture) -> Result<RunResult, String>,
{
    if !(4..=STACK_REDUCER_MAX_BUDGET).contains(&call_budget) {
        return Err(format!(
            "stack reducer call budget must be 4..={STACK_REDUCER_MAX_BUDGET}"
        ));
    }
    if !is_sha256(original_input_sha256) {
        return Err("original input digest must be lowercase SHA-256".into());
    }
    let operation = validate_stack_fixture(original)?;
    let expected_original = stack_oracle(original, original_input_sha256)?;
    let initial_weight = mutable_hamming_weight(original, operation)?;
    let mut report = StackReduction {
        stop: StackReductionStop::NoInitialMismatch,
        stop_detail: None,
        operation,
        original_fixture: original.clone(),
        reduced_fixture: None,
        initial_results: Vec::with_capacity(2),
        final_results: Vec::new(),
        expected_original,
        expected_reduced: None,
        initial_fingerprint: None,
        final_fingerprint: None,
        actual_callbacks: 0,
        candidate_proposals: 0,
        candidate_runs: 0,
        accepted_candidates: 0,
        initial_hamming_weight: initial_weight,
        final_hamming_weight: None,
    };

    let first = match invoke(&mut execute, original, &mut report.actual_callbacks) {
        Ok(result) => result,
        Err(error) => {
            report.stop = StackReductionStop::CallbackError;
            report.stop_detail = Some(error);
            return Ok(report);
        }
    };
    let second = match invoke(&mut execute, original, &mut report.actual_callbacks) {
        Ok(result) => result,
        Err(error) => {
            report.initial_results.push(first);
            report.stop = StackReductionStop::CallbackError;
            report.stop_detail = Some(error);
            return Ok(report);
        }
    };
    report
        .initial_results
        .extend([first.clone(), second.clone()]);
    if !same_semantic_result(&first, &second) {
        report.stop = StackReductionStop::FlakyInitial;
        report.stop_detail = Some("initial backend results differ semantically".into());
        return Ok(report);
    }
    let original_digest = original_input_sha256;
    let initial_fp = match mismatch_fingerprint(original, original_digest, &first, expected_backend)
        .and_then(|first_fp| {
            mismatch_fingerprint(original, original_digest, &second, expected_backend)
                .map(|second_fp| (first_fp, second_fp))
        }) {
        Ok((Some(first_fp), Some(second_fp))) if first_fp == second_fp => first_fp,
        Ok((Some(_), Some(_))) => {
            report.stop = StackReductionStop::FlakyInitial;
            report.stop_detail = Some("initial mismatch fingerprints differ".into());
            return Ok(report);
        }
        Ok((None, None)) => {
            report.stop = StackReductionStop::NoInitialMismatch;
            return Ok(report);
        }
        Ok(_) => {
            report.stop = StackReductionStop::FlakyInitial;
            report.stop_detail = Some("only one initial result preserves a mismatch".into());
            return Ok(report);
        }
        Err(error) => {
            report.stop = StackReductionStop::CallbackError;
            report.stop_detail = Some(error);
            return Ok(report);
        }
    };
    report.initial_fingerprint = Some(initial_fp.clone());

    let mut current = original.clone();
    let mut budget_exhausted = false;
    for bit in (0..64).rev() {
        let current_value = mutable_value(&current, operation)?;
        if current_value & (1u64 << bit) == 0 {
            continue;
        }
        if !has_room_for_candidate(call_budget, report.actual_callbacks) {
            budget_exhausted = true;
            break;
        }
        let candidate = clear_mutable_bit(&current, operation, bit)?;
        let candidate_weight = mutable_hamming_weight(&candidate, operation)?;
        if candidate_weight >= mutable_hamming_weight(&current, operation)? {
            return Err("candidate schedule did not strictly decrease input cost".into());
        }
        candidate.validate().map_err(|error| {
            format!("stack reducer generated an invalid candidate fixture: {error}")
        })?;
        report.candidate_proposals += 1;
        report.candidate_runs += 1;
        match try_candidate(
            &candidate,
            &initial_fp,
            expected_backend,
            &mut execute,
            &mut report,
        )? {
            CandidateDecision::Accept => {
                current = candidate;
                report.accepted_candidates += 1;
            }
            CandidateDecision::Reject => {}
            CandidateDecision::Flaky(detail) => {
                report.stop = StackReductionStop::FlakyCandidate;
                report.stop_detail = Some(detail);
                break;
            }
            CandidateDecision::Error(detail) => {
                report.stop = StackReductionStop::CallbackError;
                report.stop_detail = Some(detail);
                break;
            }
        }
    }

    if report.stop == StackReductionStop::CallbackError
        || report.stop == StackReductionStop::FlakyCandidate
    {
        return Ok(report);
    }
    let current_digest = if current == *original {
        original_input_sha256.to_string()
    } else {
        sha256_hex(&current.to_json().map_err(|error| error.to_string())?)
    };
    let expected_current = stack_oracle(&current, &current_digest)?;
    if !has_room_for_final(call_budget, report.actual_callbacks) {
        report.stop = StackReductionStop::CandidateBudgetExhausted;
        report.stop_detail = Some("budget left no room for repeated final confirmation".into());
        return Ok(report);
    }
    let final_first = match invoke(&mut execute, &current, &mut report.actual_callbacks) {
        Ok(result) => result,
        Err(error) => {
            report.stop = StackReductionStop::FinalConfirmationFailed;
            report.stop_detail = Some(error);
            return Ok(report);
        }
    };
    let final_second = match invoke(&mut execute, &current, &mut report.actual_callbacks) {
        Ok(result) => result,
        Err(error) => {
            report.final_results.push(final_first);
            report.stop = StackReductionStop::FinalConfirmationFailed;
            report.stop_detail = Some(error);
            return Ok(report);
        }
    };
    report
        .final_results
        .extend([final_first.clone(), final_second.clone()]);
    if !same_semantic_result(&final_first, &final_second) {
        report.stop = StackReductionStop::FinalConfirmationFailed;
        report.stop_detail = Some("final interpreter results differ semantically".into());
        return Ok(report);
    }
    let final_fingerprints =
        mismatch_fingerprint(&current, &current_digest, &final_first, expected_backend).and_then(
            |first_fp| {
                mismatch_fingerprint(&current, &current_digest, &final_second, expected_backend)
                    .map(|second_fp| (first_fp, second_fp))
            },
        );
    match final_fingerprints {
        Ok((Some(first_fp), Some(second_fp)))
            if first_fp == second_fp && first_fp == initial_fp =>
        {
            report.final_fingerprint = Some(first_fp);
            report.final_hamming_weight = Some(mutable_hamming_weight(&current, operation)?);
            report.expected_reduced = Some(expected_current);
            if report.accepted_candidates > 0 {
                report.reduced_fixture = Some(current);
                report.stop = if budget_exhausted {
                    StackReductionStop::CandidateBudgetExhausted
                } else {
                    StackReductionStop::Reduced
                };
            } else {
                report.stop = if budget_exhausted {
                    StackReductionStop::CandidateBudgetExhausted
                } else {
                    StackReductionStop::NoReduction
                };
            }
        }
        Ok(_) => {
            report.stop = StackReductionStop::FinalConfirmationFailed;
            report.stop_detail = Some("final result does not preserve the initial mismatch".into());
        }
        Err(error) => {
            report.stop = StackReductionStop::FinalConfirmationFailed;
            report.stop_detail = Some(error);
        }
    }
    Ok(report)
}

/// Compute the expected state with byte-level stack loads/stores independent
/// from the interpreter's PUSH/POP implementation.
pub fn stack_oracle(fixture: &Fixture, input_sha256: &str) -> Result<StackExpectedResult, String> {
    if !is_sha256(input_sha256) {
        return Err("fixture digest must be lowercase SHA-256".into());
    }
    let operation = validate_stack_fixture(fixture)?;
    let mut state = fixture.initial_state.clone();
    let old_rsp = state.gpr[4].get();
    let mut memory = fixture.memory.clone();
    let stack_index = memory
        .iter()
        .position(|region| region.region_id == "stack")
        .ok_or("missing stack memory region")?;
    let mut stack_bytes = decode_hex(&memory[stack_index].initial_bytes_hex)?;
    let stack_address = memory[stack_index].guest_address.get();

    match operation {
        StackOperation::Push { register } => {
            let new_rsp = old_rsp
                .checked_sub(8)
                .ok_or("PUSH stack pointer underflows")?;
            let write_offset = stack_offset(new_rsp, stack_address, stack_bytes.len())?;
            let value = if register == 4 {
                old_rsp
            } else {
                state.gpr[usize::from(register)].get()
            };
            stack_bytes[write_offset..write_offset + 8].copy_from_slice(&value.to_le_bytes());
            state.gpr[4] = HexU64(new_rsp);
        }
        StackOperation::Pop { register } => {
            let offset = stack_offset(old_rsp, stack_address, stack_bytes.len())?;
            let value = u64::from_le_bytes(
                stack_bytes[offset..offset + 8]
                    .try_into()
                    .map_err(|_| "POP input is not eight bytes")?,
            );
            if register == 4 {
                state.gpr[4] = HexU64(value);
            } else {
                state.gpr[usize::from(register)] = HexU64(value);
                state.gpr[4] = HexU64(
                    old_rsp
                        .checked_add(8)
                        .ok_or("POP stack pointer overflows")?,
                );
            }
        }
    }
    state.rip = fixture.stop_rip.ok_or("missing stop RIP")?;
    memory[stack_index].initial_bytes_hex = encode_hex(&stack_bytes);
    let snapshots = memory
        .into_iter()
        .map(|region| MemoryRegionSnapshot {
            region_id: region.region_id,
            guest_address: region.guest_address,
            bytes_hex: region.initial_bytes_hex,
        })
        .collect();
    Ok(StackExpectedResult {
        case_id: fixture.case_id.clone(),
        profile: fixture.profile,
        input_sha256: input_sha256.to_string(),
        outcome: Outcome::StopAddress {},
        instructions_executed: HexU64(1),
        state,
        rflags_defined: fixture.rflags_mask,
        memory: snapshots,
    })
}

fn validate_stack_fixture(fixture: &Fixture) -> Result<StackOperation, String> {
    fixture.validate().map_err(|error| error.to_string())?;
    if fixture.profile != Profile::LongIntegerV1 {
        return Err("stack reducer accepts only s2-long-integer-v1".into());
    }
    if fixture.initial_state.mode != Mode::Long64 {
        return Err("stack reducer accepts only long64 mode".into());
    }
    if fixture.expected_exit != ExpectedExit::StopAddress || fixture.instruction_budget != 1 {
        return Err("stack reducer requires one instruction ending at stop_rip".into());
    }
    let stop_rip = fixture.stop_rip.ok_or("missing stop RIP")?.get();
    if fixture.memory.len() != 2 {
        return Err("stack reducer requires exactly code and stack regions".into());
    }
    let code = fixture
        .memory
        .iter()
        .find(|region| region.region_id == "code")
        .ok_or("missing code region")?;
    let stack = fixture
        .memory
        .iter()
        .find(|region| region.region_id == "stack")
        .ok_or("missing stack region")?;
    if code.guest_address != fixture.initial_state.rip
        || !code.permissions.read
        || code.permissions.write
        || !code.permissions.execute
    {
        return Err("code region must be read/execute at the initial RIP".into());
    }
    if !stack.permissions.read || !stack.permissions.write || stack.permissions.execute {
        return Err("stack region must be read/write and non-executable".into());
    }
    if decode_hex(&code.comparison_mask_hex)?
        .iter()
        .any(|byte| *byte != 0xff)
        || decode_hex(&stack.comparison_mask_hex)?
            .iter()
            .any(|byte| *byte != 0xff)
    {
        return Err("stack reducer requires complete code and stack observation masks".into());
    }
    let instruction = decode_hex(&code.initial_bytes_hex)?;
    if instruction.len() != code.length as usize {
        return Err("code region length does not match its bytes".into());
    }
    let (opcode, extension, length) = match instruction.as_slice() {
        [opcode @ 0x50..=0x5f] => (*opcode, 0u8, 1u64),
        [0x41, opcode @ 0x50..=0x5f] => (*opcode, 8u8, 2u64),
        _ => {
            return Err("stack reducer accepts only exact qword GPR PUSH/POP opcodes 50–5f".into());
        }
    };
    let expected_stop = fixture
        .initial_state
        .rip
        .get()
        .checked_add(length)
        .ok_or("stop RIP overflows")?;
    if stop_rip != expected_stop {
        return Err("stop_rip must immediately follow the single opcode".into());
    }
    let register = (opcode & 7) + extension;
    if opcode <= 0x57 {
        Ok(StackOperation::Push { register })
    } else {
        Ok(StackOperation::Pop { register })
    }
}

fn mismatch_fingerprint(
    fixture: &Fixture,
    digest: &str,
    result: &RunResult,
    expected_backend: Backend,
) -> Result<Option<StackMismatchFingerprint>, String> {
    validate_callback_result(fixture, digest, result, expected_backend)?;
    if !matches!(result.outcome, Outcome::StopAddress {})
        || result.instructions_executed != (Observation::Available { value: HexU64(1) })
    {
        return Err("callback did not complete the exact one-instruction stop profile".into());
    }
    let Observation::Available { value: actual } = &result.snapshot else {
        return Err("successful callback result has no complete snapshot".into());
    };
    let expected = stack_oracle(fixture, digest)?;
    if actual.rflags_defined != expected.rflags_defined {
        return Err("callback changed the flags-defined mask".into());
    }
    if actual.memory.len() != expected.memory.len() {
        return Err("callback snapshot changed the compared memory-region count".into());
    }
    let mut locations = Vec::new();
    for index in 0..16 {
        if actual.state.gpr[index] != expected.state.gpr[index] {
            locations.push(format!("gpr.{index}"));
        }
    }
    if actual.state.rip != expected.state.rip {
        locations.push("rip".into());
    }
    let flag_diff = actual.state.rflags.get() ^ expected.state.rflags.get();
    for bit in 0..64 {
        if flag_diff & expected.rflags_defined.get() & (1u64 << bit) != 0 {
            locations.push(format!("rflags.bit.{bit}"));
        }
    }
    let mut actual_state = actual.state.clone();
    let expected_state = expected.state.clone();
    actual_state.rflags = expected_state.rflags;
    if actual_state != expected_state {
        locations.push("state.other".into());
    }
    for (actual_region, expected_region) in actual.memory.iter().zip(&expected.memory) {
        if actual_region.region_id != expected_region.region_id
            || actual_region.guest_address != expected_region.guest_address
        {
            return Err("callback snapshot changed memory region identity".into());
        }
        let actual_bytes = decode_hex(&actual_region.bytes_hex)?;
        let expected_bytes = decode_hex(&expected_region.bytes_hex)?;
        if actual_bytes.len() != expected_bytes.len() {
            return Err("callback snapshot changed memory region length".into());
        }
        for (offset, (actual_byte, expected_byte)) in
            actual_bytes.iter().zip(expected_bytes).enumerate()
        {
            if *actual_byte != expected_byte {
                locations.push(format!("memory.{}.byte.{offset}", actual_region.region_id));
            }
        }
    }
    if locations.is_empty() {
        Ok(None)
    } else {
        Ok(Some(StackMismatchFingerprint {
            differing_locations: locations,
        }))
    }
}

fn validate_callback_result(
    fixture: &Fixture,
    digest: &str,
    result: &RunResult,
    expected_backend: Backend,
) -> Result<(), String> {
    if result.profile != Profile::LongIntegerV1
        || result.backend != expected_backend
        || result.case_id != fixture.case_id
        || result.input_sha256 != digest
    {
        return Err(format!(
            "callback result identity does not match its exact fixture and {expected_backend:?} backend"
        ));
    }
    let loaded = LoadedFixture {
        fixture: fixture.clone(),
        input_sha256: digest.to_string(),
    };
    result
        .validate_for(&loaded)
        .map_err(|error| format!("invalid {expected_backend:?} result: {error}"))
}

enum CandidateDecision {
    Accept,
    Reject,
    Flaky(String),
    Error(String),
}

fn try_candidate<F>(
    candidate: &Fixture,
    expected: &StackMismatchFingerprint,
    expected_backend: Backend,
    execute: &mut F,
    report: &mut StackReduction,
) -> Result<CandidateDecision, String>
where
    F: FnMut(&Fixture) -> Result<RunResult, String>,
{
    let digest = sha256_hex(&candidate.to_json().map_err(|error| error.to_string())?);
    let first = match invoke(execute, candidate, &mut report.actual_callbacks) {
        Ok(result) => result,
        Err(error) => return Ok(CandidateDecision::Error(error)),
    };
    let second = match invoke(execute, candidate, &mut report.actual_callbacks) {
        Ok(result) => result,
        Err(error) => return Ok(CandidateDecision::Error(error)),
    };
    if !same_semantic_result(&first, &second) {
        return Ok(CandidateDecision::Flaky(
            "candidate backend results differ semantically".into(),
        ));
    }
    let pair =
        mismatch_fingerprint(candidate, &digest, &first, expected_backend).and_then(|first_fp| {
            mismatch_fingerprint(candidate, &digest, &second, expected_backend)
                .map(|second_fp| (first_fp, second_fp))
        });
    Ok(match pair {
        Ok((Some(first_fp), Some(second_fp))) if first_fp == second_fp && first_fp == *expected => {
            CandidateDecision::Accept
        }
        Ok((None, None)) => CandidateDecision::Reject,
        Ok((Some(_), Some(_))) | Ok(_) => CandidateDecision::Reject,
        Err(error) => CandidateDecision::Error(error),
    })
}

fn same_semantic_result(left: &RunResult, right: &RunResult) -> bool {
    left.schema_version == right.schema_version
        && left.case_id == right.case_id
        && left.input_sha256 == right.input_sha256
        && left.profile == right.profile
        && left.backend == right.backend
        && left.outcome == right.outcome
        && left.instructions_executed == right.instructions_executed
        && left.snapshot == right.snapshot
}

fn invoke<F>(execute: &mut F, fixture: &Fixture, actual: &mut usize) -> Result<RunResult, String>
where
    F: FnMut(&Fixture) -> Result<RunResult, String>,
{
    *actual += 1;
    execute(fixture)
}

fn has_room_for_candidate(budget: usize, actual: usize) -> bool {
    actual.saturating_add(4) <= budget
}

fn has_room_for_final(budget: usize, actual: usize) -> bool {
    actual.saturating_add(2) <= budget
}

fn mutable_hamming_weight(fixture: &Fixture, operation: StackOperation) -> Result<u32, String> {
    Ok(mutable_value(fixture, operation)?.count_ones())
}

fn mutable_value(fixture: &Fixture, operation: StackOperation) -> Result<u64, String> {
    match operation {
        StackOperation::Push { register } if register != 4 => {
            Ok(fixture.initial_state.gpr[usize::from(register)].get())
        }
        StackOperation::Push { .. } => Ok(0),
        StackOperation::Pop { .. } => {
            let stack = fixture
                .memory
                .iter()
                .find(|region| region.region_id == "stack")
                .ok_or("missing stack region")?;
            let bytes = decode_hex(&stack.initial_bytes_hex)?;
            let offset = stack_offset(
                fixture.initial_state.gpr[4].get(),
                stack.guest_address.get(),
                bytes.len(),
            )?;
            Ok(u64::from_le_bytes(
                bytes[offset..offset + 8]
                    .try_into()
                    .map_err(|_| "POP input is not eight bytes")?,
            ))
        }
    }
}

fn clear_mutable_bit(
    fixture: &Fixture,
    operation: StackOperation,
    bit: u32,
) -> Result<Fixture, String> {
    let mut candidate = fixture.clone();
    match operation {
        StackOperation::Push { register } if register != 4 => {
            let source = &mut candidate.initial_state.gpr[usize::from(register)];
            source.0 &= !(1u64 << bit);
        }
        StackOperation::Push { .. } => return Err("PUSH RSP has no mutable input".into()),
        StackOperation::Pop { .. } => {
            let stack = candidate
                .memory
                .iter_mut()
                .find(|region| region.region_id == "stack")
                .ok_or("missing stack region")?;
            let mut bytes = decode_hex(&stack.initial_bytes_hex)?;
            let offset = stack_offset(
                candidate.initial_state.gpr[4].get(),
                stack.guest_address.get(),
                bytes.len(),
            )?;
            let byte_index = offset + usize::try_from(bit / 8).map_err(|_| "bit overflow")?;
            bytes[byte_index] &= !(1u8 << (bit % 8));
            stack.initial_bytes_hex = encode_hex(&bytes);
        }
    }
    Ok(candidate)
}

fn stack_offset(rsp: u64, base: u64, length: usize) -> Result<usize, String> {
    let offset = rsp
        .checked_sub(base)
        .ok_or("stack access precedes mapped stack region")?;
    let offset = usize::try_from(offset).map_err(|_| "stack offset does not fit host size")?;
    let end = offset.checked_add(8).ok_or("stack span overflows")?;
    if end > length {
        return Err("eight-byte stack access is outside its mapped region".into());
    }
    Ok(offset)
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if value.len() % 2 != 0 {
        return Err("invalid odd-length byte string".into());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).map_err(|error| error.to_string())?;
            u8::from_str_radix(pair, 16).map_err(|error| error.to_string())
        })
        .collect()
}

fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::{
        StackReductionStop, decode_hex, encode_hex, reduce_stack_fixture_with_input_digest,
        stack_oracle,
    };
    use crate::result::{Backend, Observation};
    use crate::sha256_hex;
    use crate::v2::{Fixture, Outcome, RunResult, SCHEMA_VERSION};
    use serde_json::json;

    const PUSH_RAX: &[u8] =
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_17a_push_rax.case.json");
    const PUSH_RSP: &[u8] =
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_17a_push_rsp.case.json");
    const PUSH_R15: &[u8] =
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_17a_push_r15.case.json");
    const POP_RSP: &[u8] =
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_17a_pop_rsp.case.json");

    fn loaded(bytes: &[u8]) -> Fixture {
        Fixture::from_json(bytes).unwrap().fixture
    }

    fn synthetic_result(
        fixture: &Fixture,
        digest: &str,
        backend: Backend,
        corrupt: bool,
    ) -> RunResult {
        let expected = stack_oracle(fixture, digest).unwrap();
        let value = json!({
            "schema_version": SCHEMA_VERSION,
            "case_id": fixture.case_id,
            "input_sha256": digest,
            "profile": fixture.profile,
            "backend": backend,
            "outcome": Outcome::StopAddress {},
            "instructions_executed": {"status": "available", "value": expected.instructions_executed},
            "snapshot": {"status": "available", "value": {
                "state": expected.state,
                "rflags_defined": expected.rflags_defined,
                "memory": expected.memory,
                "exception": {"status": "available", "value": null}
            }},
            "build": {"revision": "test", "target": "test", "profile": "test", "compiler": "test"},
            "machine": {"host_os": "test", "host_arch": "test"}
        });
        let mut result: RunResult = serde_json::from_value(value).unwrap();
        if corrupt {
            let Observation::Available { value } = &mut result.snapshot else {
                unreachable!();
            };
            let stack = value
                .memory
                .iter_mut()
                .find(|region| region.region_id == "stack")
                .unwrap();
            let mut bytes = decode_hex(&stack.bytes_hex).unwrap();
            let offset = fixture.initial_state.gpr[4].get() as usize
                - fixture
                    .memory
                    .iter()
                    .find(|region| region.region_id == "stack")
                    .unwrap()
                    .guest_address
                    .get() as usize;
            let mismatch_offset = if fixture.memory[0].initial_bytes_hex == "50" {
                offset - 8
            } else {
                offset
            };
            bytes[mismatch_offset] ^= 1;
            stack.bytes_hex = encode_hex(&bytes);
        }
        result
    }

    #[test]
    fn oracle_push_rsp_uses_predecrement_value_and_commits_rsp() {
        let fixture = loaded(PUSH_RSP);
        let expected = stack_oracle(&fixture, &sha256_hex(PUSH_RSP)).unwrap();
        assert_eq!(expected.state.gpr[4].get(), 0x0000_0000_0030_00f8);
        let stack = expected
            .memory
            .iter()
            .find(|region| region.region_id == "stack")
            .unwrap();
        let bytes = decode_hex(&stack.bytes_hex).unwrap();
        assert_eq!(&bytes[0xf8..0x100], &0x0000_0000_0030_0100u64.to_le_bytes());
    }

    #[test]
    fn oracle_pop_rsp_overwrites_incremented_stack_pointer() {
        let fixture = loaded(POP_RSP);
        let expected = stack_oracle(&fixture, &sha256_hex(POP_RSP)).unwrap();
        assert_eq!(expected.state.gpr[4].get(), 0x8877_6655_4433_2211);
        assert_eq!(expected.state.rip, fixture.stop_rip.unwrap());
    }

    #[test]
    fn oracle_accepts_only_the_documented_rex_b_register_form() {
        let fixture = loaded(PUSH_R15);
        let expected = stack_oracle(&fixture, &sha256_hex(PUSH_R15)).unwrap();
        let stack = expected
            .memory
            .iter()
            .find(|region| region.region_id == "stack")
            .unwrap();
        let bytes = decode_hex(&stack.bytes_hex).unwrap();
        assert_eq!(&bytes[0xf8..0x100], &[0xff; 8]);
        assert_eq!(expected.state.gpr[4].get(), 0x0000_0000_0030_00f8);
    }

    #[test]
    fn repeated_stack_mismatch_reduces_only_push_source_value() {
        let fixture = loaded(PUSH_RAX);
        let digest = sha256_hex(PUSH_RAX);
        let reduction = reduce_stack_fixture_with_input_digest(
            &fixture,
            &digest,
            super::STACK_REDUCER_MAX_BUDGET,
            |candidate| {
                let candidate_digest = if candidate == &fixture {
                    digest.clone()
                } else {
                    sha256_hex(&candidate.to_json().unwrap())
                };
                Ok(synthetic_result(
                    candidate,
                    &candidate_digest,
                    Backend::Interpreter,
                    true,
                ))
            },
        )
        .unwrap();
        assert_eq!(reduction.stop, StackReductionStop::Reduced);
        assert_eq!(
            reduction.accepted_candidates,
            fixture.initial_state.gpr[0].get().count_ones() as usize
        );
        let reduced = reduction.reduced_fixture.unwrap();
        assert_eq!(reduced.initial_state.gpr[0].get(), 0);
        assert_eq!(reduced.initial_state.gpr[4], fixture.initial_state.gpr[4]);
        assert_eq!(reduced.memory, fixture.memory);
    }

    #[test]
    fn exact_interpreter_results_stop_with_no_initial_mismatch() {
        let fixture = loaded(PUSH_RAX);
        let digest = sha256_hex(PUSH_RAX);
        let reduction = reduce_stack_fixture_with_input_digest(&fixture, &digest, 4, |candidate| {
            let candidate_digest = if candidate == &fixture {
                digest.clone()
            } else {
                sha256_hex(&candidate.to_json().unwrap())
            };
            Ok(synthetic_result(
                candidate,
                &candidate_digest,
                Backend::Interpreter,
                false,
            ))
        })
        .unwrap();
        assert_eq!(reduction.stop, StackReductionStop::NoInitialMismatch);
        assert_eq!(reduction.actual_callbacks, 2);
        assert!(reduction.reduced_fixture.is_none());
    }

    #[test]
    fn exact_native_results_keep_the_original_raw_digest() {
        let mut raw = b" \n".to_vec();
        raw.extend_from_slice(PUSH_RAX);
        raw.extend_from_slice(b"\n ");
        let loaded_fixture = Fixture::from_json(&raw).unwrap();
        let digest = sha256_hex(&raw);
        let reduction = super::reduce_stack_fixture_with_native_input_digest(
            &loaded_fixture.fixture,
            &digest,
            4,
            |candidate| {
                let candidate_digest = if candidate == &loaded_fixture.fixture {
                    digest.clone()
                } else {
                    sha256_hex(&candidate.to_json().unwrap())
                };
                Ok(synthetic_result(
                    candidate,
                    &candidate_digest,
                    Backend::NativeX86,
                    false,
                ))
            },
        )
        .unwrap();

        assert_eq!(reduction.stop, StackReductionStop::NoInitialMismatch);
        assert_eq!(reduction.expected_original.input_sha256, digest);
        assert_eq!(reduction.initial_results.len(), 2);
        assert!(
            reduction
                .initial_results
                .iter()
                .all(|result| result.backend == Backend::NativeX86)
        );
        assert!(reduction.reduced_fixture.is_none());
    }

    #[test]
    fn native_reducer_rejects_interpreter_results_before_fingerprinting() {
        let fixture = loaded(PUSH_RAX);
        let digest = sha256_hex(PUSH_RAX);
        let reduction = super::reduce_stack_fixture_with_native_input_digest(
            &fixture,
            &digest,
            4,
            |candidate| {
                let candidate_digest = if candidate == &fixture {
                    digest.clone()
                } else {
                    sha256_hex(&candidate.to_json().unwrap())
                };
                Ok(synthetic_result(
                    candidate,
                    &candidate_digest,
                    Backend::Interpreter,
                    false,
                ))
            },
        )
        .unwrap();

        assert_eq!(reduction.stop, StackReductionStop::CallbackError);
        assert_eq!(reduction.actual_callbacks, 2);
        assert!(reduction.initial_fingerprint.is_none());
        assert!(reduction.reduced_fixture.is_none());
        assert!(
            reduction
                .stop_detail
                .as_deref()
                .unwrap_or_default()
                .contains("NativeX86")
        );
    }

    #[test]
    fn native_reducer_repeats_and_preserves_stable_native_mismatch() {
        let fixture = loaded(PUSH_RAX);
        let digest = sha256_hex(PUSH_RAX);
        let mut callback_backends = Vec::new();
        let reduction = super::reduce_stack_fixture_with_native_input_digest(
            &fixture,
            &digest,
            super::STACK_REDUCER_MAX_BUDGET,
            |candidate| {
                callback_backends.push(Backend::NativeX86);
                let candidate_digest = if candidate == &fixture {
                    digest.clone()
                } else {
                    sha256_hex(&candidate.to_json().unwrap())
                };
                Ok(synthetic_result(
                    candidate,
                    &candidate_digest,
                    Backend::NativeX86,
                    true,
                ))
            },
        )
        .unwrap();

        assert_eq!(reduction.stop, StackReductionStop::Reduced);
        assert_eq!(reduction.actual_callbacks, callback_backends.len());
        assert!(
            callback_backends
                .iter()
                .all(|backend| *backend == Backend::NativeX86)
        );
        assert_eq!(reduction.initial_results.len(), 2);
        assert_eq!(reduction.final_results.len(), 2);
        assert!(
            reduction
                .initial_results
                .iter()
                .chain(&reduction.final_results)
                .all(|result| result.backend == Backend::NativeX86)
        );
        assert!(reduction.initial_fingerprint.is_some());
        assert_eq!(reduction.initial_fingerprint, reduction.final_fingerprint);
    }

    #[test]
    fn native_unsupported_mapping_and_timeout_results_do_not_preserve_a_mismatch() {
        let fixture = loaded(PUSH_RAX);
        let digest = sha256_hex(PUSH_RAX);
        for outcome in [
            serde_json::json!({"kind":"unsupported","reason":"native execution unavailable"}),
            serde_json::json!({"kind":"unsupported","reason":"mapping denied for stack region"}),
            serde_json::json!({"kind":"timed_out","reason":"fixture deadline exceeded"}),
        ] {
            let reduction = super::reduce_stack_fixture_with_native_input_digest(
                &fixture,
                &digest,
                4,
                |candidate| {
                    let candidate_digest = if candidate == &fixture {
                        digest.clone()
                    } else {
                        sha256_hex(&candidate.to_json().unwrap())
                    };
                    let mut value = serde_json::to_value(synthetic_result(
                        candidate,
                        &candidate_digest,
                        Backend::NativeX86,
                        true,
                    ))
                    .unwrap();
                    value["outcome"] = outcome.clone();
                    value["instructions_executed"] = serde_json::json!({"status":"unavailable","reason":"no completed instruction"});
                    value["snapshot"] = serde_json::json!({"status":"unavailable","reason":"native execution did not complete"});
                    Ok(serde_json::from_value(value).unwrap())
                },
            )
            .unwrap();

            assert_eq!(reduction.stop, StackReductionStop::CallbackError);
            assert!(reduction.initial_fingerprint.is_none());
            assert!(reduction.reduced_fixture.is_none());
        }
    }

    #[test]
    fn native_malformed_result_identity_does_not_preserve_a_mismatch() {
        let fixture = loaded(PUSH_RAX);
        let digest = sha256_hex(PUSH_RAX);
        let reduction = super::reduce_stack_fixture_with_native_input_digest(
            &fixture,
            &digest,
            4,
            |candidate| {
                let wrong_digest = sha256_hex(b"different raw fixture");
                Ok(synthetic_result(
                    candidate,
                    &wrong_digest,
                    Backend::NativeX86,
                    true,
                ))
            },
        )
        .unwrap();

        assert_eq!(reduction.stop, StackReductionStop::CallbackError);
        assert!(reduction.initial_fingerprint.is_none());
        assert!(reduction.reduced_fixture.is_none());
        assert!(
            reduction
                .stop_detail
                .as_deref()
                .unwrap_or_default()
                .contains("exact fixture")
        );
    }

    #[test]
    fn invalid_instruction_is_rejected_before_execution() {
        let mut fixture = loaded(PUSH_RAX);
        fixture.memory[0].initial_bytes_hex = "90".into();
        let mut calls = 0;
        let result =
            reduce_stack_fixture_with_input_digest(&fixture, &sha256_hex(b"fixture"), 4, |_| {
                calls += 1;
                unreachable!()
            });
        assert!(result.is_err());
        assert_eq!(calls, 0);
    }

    #[test]
    fn callback_failure_is_not_reported_as_a_reproduced_mismatch() {
        let fixture = loaded(PUSH_RAX);
        let reduction =
            reduce_stack_fixture_with_input_digest(&fixture, &sha256_hex(PUSH_RAX), 4, |_| {
                Err("interpreter callback unavailable".into())
            })
            .unwrap();
        assert_eq!(reduction.stop, StackReductionStop::CallbackError);
        assert_eq!(reduction.actual_callbacks, 1);
        assert!(reduction.initial_fingerprint.is_none());
        assert!(reduction.reduced_fixture.is_none());
    }

    #[test]
    fn budget_exhaustion_keeps_the_original_and_reconfirms_it() {
        let fixture = loaded(PUSH_RAX);
        let digest = sha256_hex(PUSH_RAX);
        let reduction = reduce_stack_fixture_with_input_digest(&fixture, &digest, 4, |candidate| {
            let candidate_digest = if candidate == &fixture {
                digest.clone()
            } else {
                sha256_hex(&candidate.to_json().unwrap())
            };
            Ok(synthetic_result(
                candidate,
                &candidate_digest,
                Backend::Interpreter,
                true,
            ))
        })
        .unwrap();
        assert_eq!(reduction.stop, StackReductionStop::CandidateBudgetExhausted);
        assert_eq!(reduction.actual_callbacks, 4);
        assert!(reduction.reduced_fixture.is_none());
        assert_eq!(reduction.final_results.len(), 2);
    }
}
