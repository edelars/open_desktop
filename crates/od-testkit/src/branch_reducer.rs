//! Bounded reduction for exact forward JZ/JNZ rel8 programs with NOP padding.

use crate::HexU64;
use crate::result::{Backend, MemoryRegionSnapshot, Observation};
use crate::sha256_hex;
use crate::v2::{
    ExpectedExit, Fixture, IntegerState, LoadedFixture, Mode, Outcome, Profile, RunResult,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const BRANCH_REDUCER_DEFAULT_BUDGET: usize = 260;
pub const BRANCH_REDUCER_MAX_BUDGET: usize = 260;
pub const BRANCH_REDUCER_VERSION: &str = "s2-42k-branch-nop-v1";
pub const BRANCH_ORACLE_VERSION: &str = "s2-42k-jcc-full-state-oracle-v1";
const MAX_NOP_COUNT: usize = 128;
const ZF: u64 = 1 << 6;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchPath {
    Taken,
    Fallthrough,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchOpcode {
    Jz,
    Jnz,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BranchMismatchFingerprint {
    pub expected_path: BranchPath,
    pub observed_path: BranchPath,
    pub differing_locations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchReductionStop {
    Reduced,
    NoReduction,
    NoInitialMismatch,
    FlakyInitial,
    FlakyCandidate,
    CandidateBudgetExhausted,
    CallbackError,
    FinalConfirmationFailed,
    UnsupportedMismatch,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BranchExpectedResult {
    pub case_id: String,
    pub profile: Profile,
    pub input_sha256: String,
    pub path: BranchPath,
    pub outcome: Outcome,
    pub instructions_executed: HexU64,
    pub state: IntegerState,
    pub rflags_defined: HexU64,
    pub memory: Vec<MemoryRegionSnapshot>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BranchReduction {
    pub stop: BranchReductionStop,
    pub stop_detail: Option<String>,
    pub opcode: BranchOpcode,
    pub original_fixture: Fixture,
    pub reduced_fixture: Option<Fixture>,
    pub initial_results: Vec<RunResult>,
    pub final_results: Vec<RunResult>,
    pub expected_original: BranchExpectedResult,
    pub expected_reduced: Option<BranchExpectedResult>,
    pub initial_fingerprint: Option<BranchMismatchFingerprint>,
    pub final_fingerprint: Option<BranchMismatchFingerprint>,
    pub candidate_attempts: Vec<BranchCandidateAttempt>,
    pub actual_callbacks: usize,
    pub candidate_proposals: usize,
    pub candidate_runs: usize,
    pub accepted_candidates: usize,
    pub original_nops: usize,
    pub final_nops: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BranchCandidateAttempt {
    pub sequence: usize,
    pub source_sha256: String,
    pub candidate_sha256: String,
    pub deleted_nop_group: String,
    pub nops_before: usize,
    pub nops_after: usize,
    pub expected_result: BranchExpectedResult,
    pub expected_result_sha256: String,
    pub result_sha256: Vec<String>,
    pub observed_outcomes: Vec<Outcome>,
    pub observed_instruction_counts: Vec<Observation<HexU64>>,
    pub observed_fingerprint: Option<BranchMismatchFingerprint>,
    pub instruction_map: Vec<BranchInstructionRelocation>,
    pub decision: BranchCandidateDecision,
    pub reason: String,
    pub callbacks: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BranchInstructionRelocation {
    pub instruction: String,
    pub source_offset: usize,
    pub candidate_offset: usize,
    pub source_address: HexU64,
    pub candidate_address: HexU64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchCandidateDecision {
    Accepted,
    Rejected,
    Flaky,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Template {
    opcode: BranchOpcode,
    a_nops: usize,
    b_nops: usize,
    c_nops: usize,
    a: u64,
    b: u64,
    base: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NopGroup {
    BeforeRax,
    BetweenMoves,
    AfterRbx,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum InstructionIdentity {
    Branch,
    MovRax,
    MovRbx,
}

enum CandidateDecision {
    Accept,
    Reject(String),
    Flaky(String),
    Error(String),
}

struct CandidateEvaluation {
    decision: CandidateDecision,
    results: Vec<RunResult>,
    fingerprint: Option<BranchMismatchFingerprint>,
}

/// Run a deterministic NOP-deletion schedule while the exact opposite branch
/// path remains reproducible. The callback owns execution and backend choice.
pub fn reduce_branch_fixture_with_input_digest<F>(
    original: &Fixture,
    original_input_sha256: &str,
    call_budget: usize,
    execute: F,
) -> Result<BranchReduction, String>
where
    F: FnMut(&Fixture) -> Result<RunResult, String>,
{
    reduce_branch_fixture_with_backend_input_digest(
        original,
        original_input_sha256,
        call_budget,
        Backend::Interpreter,
        execute,
    )
}

/// Run the same reducer while requiring every callback to report `backend`.
/// The callback remains responsible for backend admission and execution.
pub fn reduce_branch_fixture_with_backend_input_digest<F>(
    original: &Fixture,
    original_input_sha256: &str,
    call_budget: usize,
    backend: Backend,
    mut execute: F,
) -> Result<BranchReduction, String>
where
    F: FnMut(&Fixture) -> Result<RunResult, String>,
{
    if !(4..=BRANCH_REDUCER_MAX_BUDGET).contains(&call_budget) {
        return Err(format!(
            "branch reducer call budget must be 4..={BRANCH_REDUCER_MAX_BUDGET}"
        ));
    }
    if !is_sha256(original_input_sha256) {
        return Err("original input digest must be lowercase SHA-256".into());
    }
    let template = validate_branch_fixture(original)?;
    let expected_original = branch_oracle(original, original_input_sha256)?;
    let original_nops = template.total_nops();
    let mut report = BranchReduction {
        stop: BranchReductionStop::NoInitialMismatch,
        stop_detail: None,
        opcode: template.opcode,
        original_fixture: original.clone(),
        reduced_fixture: None,
        initial_results: Vec::with_capacity(2),
        final_results: Vec::new(),
        expected_original,
        expected_reduced: None,
        initial_fingerprint: None,
        final_fingerprint: None,
        candidate_attempts: Vec::new(),
        actual_callbacks: 0,
        candidate_proposals: 0,
        candidate_runs: 0,
        accepted_candidates: 0,
        original_nops,
        final_nops: None,
    };

    let first = match invoke(&mut execute, original, &mut report.actual_callbacks) {
        Ok(result) => result,
        Err(error) => {
            report.stop = BranchReductionStop::CallbackError;
            report.stop_detail = Some(error);
            return Ok(report);
        }
    };
    let second = match invoke(&mut execute, original, &mut report.actual_callbacks) {
        Ok(result) => result,
        Err(error) => {
            report.initial_results.push(first);
            report.stop = BranchReductionStop::CallbackError;
            report.stop_detail = Some(error);
            return Ok(report);
        }
    };
    report
        .initial_results
        .extend([first.clone(), second.clone()]);
    if !same_semantic_result(&first, &second) {
        report.stop = BranchReductionStop::FlakyInitial;
        report.stop_detail = Some("initial interpreter results differ semantically".into());
        return Ok(report);
    }
    let initial_fingerprint =
        match fingerprint_pair(original, original_input_sha256, backend, &first, &second) {
            Ok((None, None)) => {
                report.stop = BranchReductionStop::NoInitialMismatch;
                return Ok(report);
            }
            Ok((Some(left), Some(right))) if left == right => left,
            Ok((Some(_), Some(_))) => {
                report.stop = BranchReductionStop::FlakyInitial;
                report.stop_detail = Some("initial branch fingerprints differ".into());
                return Ok(report);
            }
            Ok(_) => {
                report.stop = BranchReductionStop::FlakyInitial;
                report.stop_detail = Some("only one initial result preserves a mismatch".into());
                return Ok(report);
            }
            Err(error) => {
                report.stop = BranchReductionStop::UnsupportedMismatch;
                report.stop_detail = Some(error);
                return Ok(report);
            }
        };
    report.initial_fingerprint = Some(initial_fingerprint.clone());

    let mut current = original.clone();
    let mut current_template = template;
    let mut budget_exhausted = false;
    let schedule = [
        (NopGroup::BeforeRax, template.a_nops),
        (NopGroup::BetweenMoves, template.b_nops),
        (NopGroup::AfterRbx, template.c_nops),
    ];
    'groups: for (group, original_count) in schedule {
        for _ in 0..original_count {
            if current_template.group_count(group) == 0 {
                continue;
            }
            if report.candidate_proposals >= MAX_NOP_COUNT
                || !has_room_for_candidate(call_budget, report.actual_callbacks)
            {
                budget_exhausted = true;
                break 'groups;
            }
            let candidate = delete_group_nop(&current, current_template, group)?;
            candidate
                .validate()
                .map_err(|error| format!("branch reducer produced invalid fixture: {error}"))?;
            let candidate_template = validate_branch_fixture(&candidate)?;
            if candidate_template.total_nops() >= current_template.total_nops() {
                return Err("NOP candidate did not strictly decrease instruction count".into());
            }
            report.candidate_proposals += 1;
            let source_digest = if current == *original {
                original_input_sha256.to_string()
            } else {
                sha256_hex(&current.to_json().map_err(|error| error.to_string())?)
            };
            let candidate_digest =
                sha256_hex(&candidate.to_json().map_err(|error| error.to_string())?);
            let expected_candidate = branch_oracle(&candidate, &candidate_digest)?;
            let expected_candidate_sha256 = sha256_hex(
                &serde_json::to_vec(&expected_candidate).map_err(|error| error.to_string())?,
            );
            let callbacks_before = report.actual_callbacks;
            let evaluation = try_candidate(
                &candidate,
                &initial_fingerprint,
                backend,
                &mut execute,
                &mut report,
            )?;
            let candidate_callback_count = report.actual_callbacks - callbacks_before;
            report.candidate_runs += candidate_callback_count;
            let (decision_status, reason) = match &evaluation.decision {
                CandidateDecision::Accept => (
                    BranchCandidateDecision::Accepted,
                    "two stable executions preserve the original mismatch fingerprint".to_string(),
                ),
                CandidateDecision::Reject(reason) => {
                    (BranchCandidateDecision::Rejected, reason.clone())
                }
                CandidateDecision::Flaky(reason) => {
                    (BranchCandidateDecision::Flaky, reason.clone())
                }
                CandidateDecision::Error(reason) => {
                    (BranchCandidateDecision::Error, reason.clone())
                }
            };
            report.candidate_attempts.push(BranchCandidateAttempt {
                sequence: report.candidate_proposals,
                source_sha256: source_digest,
                candidate_sha256: candidate_digest,
                deleted_nop_group: nop_group_name(group).to_string(),
                nops_before: current_template.total_nops(),
                nops_after: candidate_template.total_nops(),
                expected_result: expected_candidate,
                expected_result_sha256: expected_candidate_sha256,
                result_sha256: evaluation
                    .results
                    .iter()
                    .map(run_result_digest)
                    .collect::<Result<Vec<_>, _>>()?,
                observed_outcomes: evaluation
                    .results
                    .iter()
                    .map(|result| result.outcome.clone())
                    .collect(),
                observed_instruction_counts: evaluation
                    .results
                    .iter()
                    .map(|result| result.instructions_executed.clone())
                    .collect(),
                observed_fingerprint: evaluation.fingerprint.clone(),
                instruction_map: instruction_relocations(current_template, candidate_template)?,
                decision: decision_status,
                reason,
                callbacks: candidate_callback_count,
            });
            match evaluation.decision {
                CandidateDecision::Accept => {
                    current = candidate;
                    current_template = candidate_template;
                    report.accepted_candidates += 1;
                }
                CandidateDecision::Reject(_) => break,
                CandidateDecision::Flaky(detail) => {
                    report.stop = BranchReductionStop::FlakyCandidate;
                    report.stop_detail = Some(detail);
                    break 'groups;
                }
                CandidateDecision::Error(detail) => {
                    report.stop = BranchReductionStop::CallbackError;
                    report.stop_detail = Some(detail);
                    break 'groups;
                }
            }
        }
    }
    if matches!(
        report.stop,
        BranchReductionStop::FlakyCandidate | BranchReductionStop::CallbackError
    ) {
        return Ok(report);
    }

    let current_digest = if current == *original {
        original_input_sha256.to_string()
    } else {
        sha256_hex(&current.to_json().map_err(|error| error.to_string())?)
    };
    let expected_current = branch_oracle(&current, &current_digest)?;
    if !has_room_for_final(call_budget, report.actual_callbacks) {
        report.stop = BranchReductionStop::CandidateBudgetExhausted;
        report.stop_detail = Some("budget left no room for repeated final confirmation".into());
        return Ok(report);
    }
    let final_first = match invoke(&mut execute, &current, &mut report.actual_callbacks) {
        Ok(result) => result,
        Err(error) => {
            report.stop = BranchReductionStop::FinalConfirmationFailed;
            report.stop_detail = Some(error);
            return Ok(report);
        }
    };
    let final_second = match invoke(&mut execute, &current, &mut report.actual_callbacks) {
        Ok(result) => result,
        Err(error) => {
            report.final_results.push(final_first);
            report.stop = BranchReductionStop::FinalConfirmationFailed;
            report.stop_detail = Some(error);
            return Ok(report);
        }
    };
    report
        .final_results
        .extend([final_first.clone(), final_second.clone()]);
    if !same_semantic_result(&final_first, &final_second) {
        report.stop = BranchReductionStop::FinalConfirmationFailed;
        report.stop_detail = Some("final interpreter results differ semantically".into());
        return Ok(report);
    }
    match fingerprint_pair(
        &current,
        &current_digest,
        backend,
        &final_first,
        &final_second,
    ) {
        Ok((Some(left), Some(right))) if left == right && left == initial_fingerprint => {
            report.final_fingerprint = Some(left);
            report.final_nops = Some(current_template.total_nops());
            report.expected_reduced = Some(expected_current);
            if report.accepted_candidates > 0 {
                report.reduced_fixture = Some(current);
                report.stop = if budget_exhausted {
                    BranchReductionStop::CandidateBudgetExhausted
                } else {
                    BranchReductionStop::Reduced
                };
            } else {
                report.stop = if budget_exhausted {
                    BranchReductionStop::CandidateBudgetExhausted
                } else {
                    BranchReductionStop::NoReduction
                };
            }
        }
        Ok(_) => {
            report.stop = BranchReductionStop::FinalConfirmationFailed;
            report.stop_detail =
                Some("final callback results do not preserve the initial mismatch".into());
        }
        Err(error) => {
            report.stop = BranchReductionStop::FinalConfirmationFailed;
            report.stop_detail = Some(error);
        }
    }
    Ok(report)
}

/// Calculate the expected architectural state from the ZF bit and literal
/// MOV immediates; this oracle does not decode or execute the program.
pub fn branch_oracle(
    fixture: &Fixture,
    input_sha256: &str,
) -> Result<BranchExpectedResult, String> {
    if !is_sha256(input_sha256) {
        return Err("fixture digest must be lowercase SHA-256".into());
    }
    let template = validate_branch_fixture(fixture)?;
    expected_for_path(
        fixture,
        template,
        input_sha256,
        expected_path(fixture, template),
    )
}

fn expected_for_path(
    fixture: &Fixture,
    template: Template,
    input_sha256: &str,
    path: BranchPath,
) -> Result<BranchExpectedResult, String> {
    let mut state = fixture.initial_state.clone();
    if path == BranchPath::Fallthrough {
        state.gpr[0] = HexU64(template.a);
    }
    state.gpr[3] = HexU64(template.b);
    state.rip = fixture.stop_rip.ok_or("missing stop RIP")?;
    let instructions = match path {
        BranchPath::Taken => 2 + template.c_nops,
        BranchPath::Fallthrough => 3 + template.a_nops + template.b_nops + template.c_nops,
    };
    let memory = fixture
        .memory
        .iter()
        .map(|region| MemoryRegionSnapshot {
            region_id: region.region_id.clone(),
            guest_address: region.guest_address,
            bytes_hex: region.initial_bytes_hex.clone(),
        })
        .collect();
    Ok(BranchExpectedResult {
        case_id: fixture.case_id.clone(),
        profile: fixture.profile,
        input_sha256: input_sha256.to_string(),
        path,
        outcome: Outcome::StopAddress {},
        instructions_executed: HexU64(instructions as u64),
        state,
        rflags_defined: HexU64(crate::v2::INTEGER_FLAGS),
        memory,
    })
}

fn expected_path(fixture: &Fixture, template: Template) -> BranchPath {
    let zero = fixture.initial_state.rflags.get() & ZF != 0;
    let taken = match template.opcode {
        BranchOpcode::Jz => zero,
        BranchOpcode::Jnz => !zero,
    };
    if taken {
        BranchPath::Taken
    } else {
        BranchPath::Fallthrough
    }
}

fn opposite(path: BranchPath) -> BranchPath {
    match path {
        BranchPath::Taken => BranchPath::Fallthrough,
        BranchPath::Fallthrough => BranchPath::Taken,
    }
}

fn validate_branch_fixture(fixture: &Fixture) -> Result<Template, String> {
    fixture.validate().map_err(|error| error.to_string())?;
    if fixture.profile != Profile::LongIntegerV1 || fixture.initial_state.mode != Mode::Long64 {
        return Err("branch reducer accepts only long64 s2-long-integer-v1 fixtures".into());
    }
    if fixture.expected_exit != ExpectedExit::StopAddress {
        return Err("branch reducer requires a stop_address fixture".into());
    }
    if fixture.rflags_mask.get() & ZF == 0 {
        return Err("branch reducer requires ZF to be compared".into());
    }
    if fixture.memory.len() != 1 {
        return Err("branch reducer requires exactly one code region".into());
    }
    let code = &fixture.memory[0];
    if code.region_id != "code"
        || code.guest_address != fixture.initial_state.rip
        || !code.permissions.read
        || code.permissions.write
        || !code.permissions.execute
    {
        return Err("code region must be RX and mapped at the fixed entry RIP".into());
    }
    let bytes = decode_hex(&code.initial_bytes_hex)?;
    if bytes.len() != code.length as usize || bytes.len() > 150 {
        return Err("code bytes must exactly match a template-sized region".into());
    }
    let mask = decode_hex(&code.comparison_mask_hex)?;
    if mask.len() != bytes.len() || mask.iter().any(|value| *value != 0xff) {
        return Err("branch reducer requires a full code comparison mask".into());
    }
    let base = code.guest_address.get();
    let final_byte = base
        .checked_add(bytes.len().saturating_sub(1) as u64)
        .ok_or("code address range overflows")?;
    if base >> 12 != final_byte >> 12 {
        return Err("the complete code region must remain in one 4 KiB page".into());
    }
    let stop = base
        .checked_add(bytes.len() as u64)
        .ok_or("stop RIP overflows")?;
    if fixture.stop_rip != Some(HexU64(stop)) {
        return Err("stop_rip must immediately follow the complete code region".into());
    }
    let template = parse_template(&bytes, base)?;
    // The alternate path must be distinguishable by RAX changing.
    if template.a == fixture.initial_state.gpr[0].get() {
        return Err("MOV RAX immediate must differ from the initial RAX".into());
    }
    let maximum_path = 3 + template.a_nops + template.b_nops + template.c_nops;
    if fixture.instruction_budget < maximum_path as u64 {
        return Err("instruction budget must allow the complete fallthrough path".into());
    }
    Ok(template)
}

fn parse_template(bytes: &[u8], base: u64) -> Result<Template, String> {
    if bytes.len() < 22 {
        return Err("branch code is shorter than the exact instruction template".into());
    }
    let opcode = match bytes[0] {
        0x74 => BranchOpcode::Jz,
        0x75 => BranchOpcode::Jnz,
        _ => return Err("template must start with exactly one JZ/JNZ rel8".into()),
    };
    let mut offset = 2usize;
    let a_nops = consume_nops(bytes, &mut offset)?;
    let a = consume_mov(bytes, &mut offset, 0xb8, "MOV RAX")?;
    let b_nops = consume_nops(bytes, &mut offset)?;
    let b = consume_mov(bytes, &mut offset, 0xbb, "MOV RBX")?;
    let c_nops = consume_nops(bytes, &mut offset)?;
    if offset != bytes.len() {
        return Err("template has trailing or unsupported instruction bytes".into());
    }
    let total = a_nops + b_nops + c_nops;
    if total > MAX_NOP_COUNT || a_nops + b_nops > 117 {
        return Err("NOP counts exceed the bounded rel8 template limits".into());
    }
    let target_offset = 2 + a_nops + 10 + b_nops;
    if usize::from(bytes[1]) != target_offset - 2 {
        return Err("JZ/JNZ rel8 must target the retained MOV RBX instruction".into());
    }
    let code_end = base
        .checked_add(bytes.len() as u64)
        .ok_or("code end overflows")?;
    if base >> 12 != (code_end - 1) >> 12 {
        return Err("complete code template must fit within one page".into());
    }
    Ok(Template {
        opcode,
        a_nops,
        b_nops,
        c_nops,
        a,
        b,
        base,
    })
}

fn consume_nops(bytes: &[u8], offset: &mut usize) -> Result<usize, String> {
    let start = *offset;
    while bytes.get(*offset) == Some(&0x90) {
        *offset += 1;
    }
    Ok(*offset - start)
}

fn consume_mov(bytes: &[u8], offset: &mut usize, opcode: u8, label: &str) -> Result<u64, String> {
    let end = offset.checked_add(10).ok_or("MOV end overflows")?;
    let instruction = bytes
        .get(*offset..end)
        .ok_or_else(|| format!("missing exact {label}, imm64 instruction"))?;
    if instruction[0] != 0x48 || instruction[1] != opcode {
        return Err(format!(
            "template requires exact 48 {opcode:02x} {label},imm64"
        ));
    }
    let immediate: [u8; 8] = instruction[2..10]
        .try_into()
        .map_err(|_| "MOV immediate is not eight bytes")?;
    *offset = end;
    Ok(u64::from_le_bytes(immediate))
}

fn delete_group_nop(
    fixture: &Fixture,
    template: Template,
    group: NopGroup,
) -> Result<Fixture, String> {
    let mut candidate_template = template;
    match group {
        NopGroup::BeforeRax if candidate_template.a_nops > 0 => candidate_template.a_nops -= 1,
        NopGroup::BetweenMoves if candidate_template.b_nops > 0 => candidate_template.b_nops -= 1,
        NopGroup::AfterRbx if candidate_template.c_nops > 0 => candidate_template.c_nops -= 1,
        _ => return Err("cannot delete a NOP from an empty group".into()),
    }
    let old_map = instruction_address_map(template);
    let new_map = instruction_address_map(candidate_template);
    for identity in [
        InstructionIdentity::Branch,
        InstructionIdentity::MovRax,
        InstructionIdentity::MovRbx,
    ] {
        if old_map.get(&identity).is_none() || new_map.get(&identity).is_none() {
            return Err("NOP deletion lost a retained instruction identity".into());
        }
    }
    let target_offset = *new_map
        .get(&InstructionIdentity::MovRbx)
        .ok_or("relocation map lost the MOV RBX branch target")?;
    let bytes = encode_template_with_target(candidate_template, target_offset)?;
    let mut candidate = fixture.clone();
    let code = candidate.memory.first_mut().ok_or("missing code region")?;
    code.length = u32::try_from(bytes.len()).map_err(|_| "candidate code too large")?;
    code.initial_bytes_hex = encode_hex(&bytes);
    code.comparison_mask_hex = encode_hex(&vec![0xff; bytes.len()]);
    candidate.stop_rip = Some(HexU64(
        candidate_template
            .base
            .checked_add(bytes.len() as u64)
            .ok_or("candidate stop RIP overflows")?,
    ));
    Ok(candidate)
}

#[cfg(test)]
fn encode_template(template: Template) -> Result<Vec<u8>, String> {
    let target_offset = *instruction_address_map(template)
        .get(&InstructionIdentity::MovRbx)
        .ok_or("instruction map has no MOV RBX target")?;
    encode_template_with_target(template, target_offset)
}

fn encode_template_with_target(
    template: Template,
    target_offset: usize,
) -> Result<Vec<u8>, String> {
    let expected_target = 2 + template.a_nops + 10 + template.b_nops;
    if target_offset != expected_target {
        return Err("relocation map does not point at the retained MOV RBX target".into());
    }
    let displacement = target_offset
        .checked_sub(2)
        .ok_or("target precedes Jcc end")?;
    if displacement > i8::MAX as usize || template.total_nops() > MAX_NOP_COUNT {
        return Err("candidate Jcc displacement or NOP count exceeds the bounded form".into());
    }
    let mut bytes = Vec::with_capacity(22 + template.total_nops());
    bytes.push(match template.opcode {
        BranchOpcode::Jz => 0x74,
        BranchOpcode::Jnz => 0x75,
    });
    bytes.push(displacement as u8);
    bytes.extend(std::iter::repeat(0x90).take(template.a_nops));
    bytes.extend_from_slice(&[0x48, 0xb8]);
    bytes.extend_from_slice(&template.a.to_le_bytes());
    bytes.extend(std::iter::repeat(0x90).take(template.b_nops));
    bytes.extend_from_slice(&[0x48, 0xbb]);
    bytes.extend_from_slice(&template.b.to_le_bytes());
    bytes.extend(std::iter::repeat(0x90).take(template.c_nops));
    Ok(bytes)
}

fn instruction_address_map(template: Template) -> BTreeMap<InstructionIdentity, usize> {
    BTreeMap::from([
        (InstructionIdentity::Branch, 0),
        (InstructionIdentity::MovRax, 2 + template.a_nops),
        (
            InstructionIdentity::MovRbx,
            2 + template.a_nops + 10 + template.b_nops,
        ),
    ])
}

fn instruction_relocations(
    source: Template,
    candidate: Template,
) -> Result<Vec<BranchInstructionRelocation>, String> {
    let source_map = instruction_address_map(source);
    let candidate_map = instruction_address_map(candidate);
    [
        (InstructionIdentity::Branch, "jcc"),
        (InstructionIdentity::MovRax, "mov_rax"),
        (InstructionIdentity::MovRbx, "mov_rbx_target"),
    ]
    .into_iter()
    .map(|(identity, label)| {
        let source_offset = *source_map
            .get(&identity)
            .ok_or("source instruction map is incomplete")?;
        let candidate_offset = *candidate_map
            .get(&identity)
            .ok_or("candidate instruction map is incomplete")?;
        Ok(BranchInstructionRelocation {
            instruction: label.to_string(),
            source_offset,
            candidate_offset,
            source_address: HexU64(
                source
                    .base
                    .checked_add(source_offset as u64)
                    .ok_or("source address overflows")?,
            ),
            candidate_address: HexU64(
                candidate
                    .base
                    .checked_add(candidate_offset as u64)
                    .ok_or("candidate address overflows")?,
            ),
        })
    })
    .collect()
}

fn nop_group_name(group: NopGroup) -> &'static str {
    match group {
        NopGroup::BeforeRax => "before_rax",
        NopGroup::BetweenMoves => "between_moves",
        NopGroup::AfterRbx => "after_rbx",
    }
}

impl Template {
    fn total_nops(self) -> usize {
        self.a_nops + self.b_nops + self.c_nops
    }

    fn group_count(self, group: NopGroup) -> usize {
        match group {
            NopGroup::BeforeRax => self.a_nops,
            NopGroup::BetweenMoves => self.b_nops,
            NopGroup::AfterRbx => self.c_nops,
        }
    }
}

fn fingerprint_pair(
    fixture: &Fixture,
    digest: &str,
    backend: Backend,
    first: &RunResult,
    second: &RunResult,
) -> Result<
    (
        Option<BranchMismatchFingerprint>,
        Option<BranchMismatchFingerprint>,
    ),
    String,
> {
    Ok((
        mismatch_fingerprint(fixture, digest, backend, first)?,
        mismatch_fingerprint(fixture, digest, backend, second)?,
    ))
}

fn mismatch_fingerprint(
    fixture: &Fixture,
    digest: &str,
    backend: Backend,
    result: &RunResult,
) -> Result<Option<BranchMismatchFingerprint>, String> {
    validate_callback_result(fixture, digest, backend, result)?;
    let template = validate_branch_fixture(fixture)?;
    let expected_path = expected_path(fixture, template);
    let expected = expected_for_path(fixture, template, digest, expected_path)?;
    if result_matches(result, &expected) {
        return Ok(None);
    }
    let observed_path = opposite(expected_path);
    let alternate = expected_for_path(fixture, template, digest, observed_path)?;
    if result_matches(result, &alternate) {
        return Ok(Some(BranchMismatchFingerprint {
            expected_path,
            observed_path,
            differing_locations: vec!["gpr.rax".into(), "instructions_executed".into()],
        }));
    }
    Err("completed callback is neither the expected path nor the exact opposite branch path".into())
}

fn result_matches(result: &RunResult, expected: &BranchExpectedResult) -> bool {
    if result.outcome != expected.outcome
        || result.instructions_executed
            != (Observation::Available {
                value: expected.instructions_executed,
            })
    {
        return false;
    }
    let Observation::Available { value } = &result.snapshot else {
        return false;
    };
    value.state == expected.state
        && value.rflags_defined == expected.rflags_defined
        && value.memory == expected.memory
}

fn validate_callback_result(
    fixture: &Fixture,
    digest: &str,
    backend: Backend,
    result: &RunResult,
) -> Result<(), String> {
    if result.profile != Profile::LongIntegerV1
        || result.backend != backend
        || result.case_id != fixture.case_id
        || result.input_sha256 != digest
    {
        return Err("callback result identity does not match its exact fixture".into());
    }
    let loaded = LoadedFixture {
        fixture: fixture.clone(),
        input_sha256: digest.to_string(),
    };
    result
        .validate_for(&loaded)
        .map_err(|error| format!("invalid {backend:?} result: {error}"))
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

fn run_result_digest(result: &RunResult) -> Result<String, String> {
    let bytes = match result.to_json() {
        Ok(bytes) => bytes,
        Err(_) => serde_json::to_vec(result).map_err(|error| error.to_string())?,
    };
    Ok(sha256_hex(&bytes))
}

fn try_candidate<F>(
    candidate: &Fixture,
    expected: &BranchMismatchFingerprint,
    backend: Backend,
    execute: &mut F,
    report: &mut BranchReduction,
) -> Result<CandidateEvaluation, String>
where
    F: FnMut(&Fixture) -> Result<RunResult, String>,
{
    let digest = sha256_hex(&candidate.to_json().map_err(|error| error.to_string())?);
    let first = match invoke(execute, candidate, &mut report.actual_callbacks) {
        Ok(result) => result,
        Err(error) => {
            return Ok(CandidateEvaluation {
                decision: CandidateDecision::Error(error),
                results: Vec::new(),
                fingerprint: None,
            });
        }
    };
    let second = match invoke(execute, candidate, &mut report.actual_callbacks) {
        Ok(result) => result,
        Err(error) => {
            return Ok(CandidateEvaluation {
                decision: CandidateDecision::Error(error),
                results: vec![first],
                fingerprint: None,
            });
        }
    };
    if !same_semantic_result(&first, &second) {
        return Ok(CandidateEvaluation {
            decision: CandidateDecision::Flaky(
                "candidate callback results differ semantically".into(),
            ),
            results: vec![first, second],
            fingerprint: None,
        });
    }
    let (decision, fingerprint) =
        match fingerprint_pair(candidate, &digest, backend, &first, &second) {
            Ok((Some(left), Some(right))) if left == right && left == *expected => {
                (CandidateDecision::Accept, Some(left))
            }
            Ok((None, None)) => (
                CandidateDecision::Reject(
                    "candidate no longer reproduces an opposite-path mismatch".into(),
                ),
                None,
            ),
            Ok((Some(left), Some(right))) if left != right => (
                CandidateDecision::Flaky(
                    "candidate branch fingerprints differ between repeats".into(),
                ),
                None,
            ),
            Ok((Some(_), Some(_))) | Ok(_) => (
                CandidateDecision::Reject(
                    "candidate mismatch fingerprint differs from the original".into(),
                ),
                None,
            ),
            Err(error) => (CandidateDecision::Error(error), None),
        };
    Ok(CandidateEvaluation {
        decision,
        results: vec![first, second],
        fingerprint,
    })
}

fn invoke<F>(
    execute: &mut F,
    fixture: &Fixture,
    actual_callbacks: &mut usize,
) -> Result<RunResult, String>
where
    F: FnMut(&Fixture) -> Result<RunResult, String>,
{
    *actual_callbacks = actual_callbacks
        .checked_add(1)
        .ok_or("branch reducer callback counter overflows")?;
    execute(fixture)
}

fn has_room_for_candidate(call_budget: usize, actual_callbacks: usize) -> bool {
    call_budget.saturating_sub(actual_callbacks) >= 4
}

fn has_room_for_final(call_budget: usize, actual_callbacks: usize) -> bool {
    call_budget.saturating_sub(actual_callbacks) >= 2
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn decode_hex(input: &str) -> Result<Vec<u8>, String> {
    if input.len() % 2 != 0 {
        return Err("hex data must contain whole bytes".into());
    }
    input
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text =
                std::str::from_utf8(pair).map_err(|_| "hex data must be ASCII".to_string())?;
            u8::from_str_radix(text, 16)
                .map_err(|_| "hex data contains a non-hex digit".to_string())
        })
        .collect()
}

fn encode_hex(input: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut output = String::with_capacity(input.len() * 2);
    for byte in input {
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{
        BRANCH_REDUCER_MAX_BUDGET, BranchOpcode, BranchPath, BranchReductionStop, branch_oracle,
        encode_template, reduce_branch_fixture_with_input_digest,
    };
    use crate::result::{Backend, Observation};
    use crate::sha256_hex;
    use crate::v2::{Fixture, Outcome, RunResult, SCHEMA_VERSION};
    use serde_json::json;

    const JE_TAKEN: &[u8] =
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_16a_je_short_taken.case.json");

    fn template_fixture(
        opcode: BranchOpcode,
        zf: bool,
        a_nops: usize,
        b_nops: usize,
        c_nops: usize,
    ) -> Fixture {
        let mut fixture = Fixture::from_json(JE_TAKEN).unwrap().fixture;
        let template = super::Template {
            opcode,
            a_nops,
            b_nops,
            c_nops,
            a: 0xfeed_face_cafe_beef,
            b: 0xaabb_ccdd_0123_4567,
            base: fixture.initial_state.rip.get(),
        };
        let code = encode_template(template).unwrap();
        fixture.case_id = format!(
            "s2_42k_{}_{}_{}_{}",
            if opcode == BranchOpcode::Jz {
                "jz"
            } else {
                "jnz"
            },
            a_nops,
            b_nops,
            c_nops
        );
        fixture.initial_state.rflags = crate::HexU64(if zf { 0x2 | 0x40 } else { 0x2 });
        fixture.initial_state.gpr[0] = crate::HexU64(0x1234_5678_9abc_def0);
        fixture.initial_state.gpr[3] = crate::HexU64(0x8765_4321_0123_4567);
        fixture.instruction_budget = 150;
        fixture.memory.truncate(1);
        fixture.memory[0].length = code.len() as u32;
        fixture.memory[0].initial_bytes_hex = super::encode_hex(&code);
        fixture.memory[0].comparison_mask_hex = "ff".repeat(code.len());
        fixture.stop_rip = Some(crate::HexU64(template.base + code.len() as u64));
        fixture.validate().unwrap();
        fixture
    }

    fn synthetic_result(fixture: &Fixture, digest: &str, path: BranchPath) -> RunResult {
        synthetic_result_backend(fixture, digest, path, Backend::Interpreter)
    }

    fn synthetic_result_backend(
        fixture: &Fixture,
        digest: &str,
        path: BranchPath,
        backend: Backend,
    ) -> RunResult {
        let template = super::validate_branch_fixture(fixture).unwrap();
        let expected = super::expected_for_path(fixture, template, digest, path).unwrap();
        serde_json::from_value(json!({
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
        })).unwrap()
    }

    #[test]
    fn jz_jnz_oracle_handles_taken_and_fallthrough_with_full_retirement() {
        for (opcode, zf, expected_path) in [
            (BranchOpcode::Jz, true, BranchPath::Taken),
            (BranchOpcode::Jz, false, BranchPath::Fallthrough),
            (BranchOpcode::Jnz, false, BranchPath::Taken),
            (BranchOpcode::Jnz, true, BranchPath::Fallthrough),
        ] {
            let fixture = template_fixture(opcode, zf, 2, 1, 2);
            let expected = branch_oracle(&fixture, &sha256_hex(b"fixture")).unwrap();
            assert_eq!(expected.path, expected_path);
            assert_eq!(expected.state.gpr[3].get(), 0xaabb_ccdd_0123_4567);
            assert_eq!(expected.state.rip, fixture.stop_rip.unwrap());
            assert_eq!(
                expected.instructions_executed.get(),
                match expected_path {
                    BranchPath::Taken => 4,
                    BranchPath::Fallthrough => 8,
                }
            );
        }
    }

    #[test]
    fn deleting_nops_relocates_the_same_mov_rbx_target_and_stop() {
        let fixture = template_fixture(BranchOpcode::Jz, false, 2, 3, 2);
        let template = super::validate_branch_fixture(&fixture).unwrap();
        let candidate =
            super::delete_group_nop(&fixture, template, super::NopGroup::BeforeRax).unwrap();
        let bytes = super::decode_hex(&candidate.memory[0].initial_bytes_hex).unwrap();
        assert_eq!(bytes[0], 0x74);
        assert_eq!(bytes[1], 10 + 1 + 3);
        let target = 2 + usize::from(bytes[1]);
        assert_eq!(&bytes[target..target + 2], &[0x48, 0xbb]);
        assert_eq!(
            candidate.stop_rip,
            Some(crate::HexU64(
                fixture.initial_state.rip.get() + bytes.len() as u64
            ))
        );
        assert_eq!(candidate.initial_state.rip, fixture.initial_state.rip);
        assert_eq!(candidate.instruction_budget, fixture.instruction_budget);
    }

    #[test]
    fn synthetic_opposite_branch_reduces_padding_without_changing_target_identity() {
        let fixture = template_fixture(BranchOpcode::Jz, true, 1, 1, 1);
        let digest = sha256_hex(JE_TAKEN);
        for backend in [Backend::Interpreter, Backend::NativeX86] {
            let reduction = super::reduce_branch_fixture_with_backend_input_digest(
                &fixture,
                &digest,
                BRANCH_REDUCER_MAX_BUDGET,
                backend,
                |candidate| {
                    let candidate_digest = if candidate == &fixture {
                        digest.clone()
                    } else {
                        sha256_hex(&candidate.to_json().unwrap())
                    };
                    let expected_path = super::expected_path(
                        candidate,
                        super::validate_branch_fixture(candidate).unwrap(),
                    );
                    Ok(synthetic_result_backend(
                        candidate,
                        &candidate_digest,
                        super::opposite(expected_path),
                        backend,
                    ))
                },
            )
            .unwrap();

            assert_eq!(reduction.stop, BranchReductionStop::Reduced);
            assert_eq!(reduction.actual_callbacks, 10);
            assert_eq!(reduction.candidate_proposals, 3);
            assert_eq!(reduction.accepted_candidates, 3);
            assert_eq!(reduction.candidate_runs, 6);
            assert_eq!(reduction.candidate_attempts.len(), 3);
            assert!(
                reduction
                    .initial_results
                    .iter()
                    .chain(&reduction.final_results)
                    .all(|result| result.backend == backend)
            );
            assert!(reduction.candidate_attempts.iter().all(|attempt| {
                attempt.callbacks == 2
                    && attempt.result_sha256.len() == 2
                    && attempt.observed_outcomes.len() == 2
                    && attempt.observed_fingerprint.is_some()
                    && attempt.decision == super::BranchCandidateDecision::Accepted
            }));
            let mut source = fixture.clone();
            let mut source_digest = digest.clone();
            for (index, attempt) in reduction.candidate_attempts.iter().enumerate() {
                assert_eq!(attempt.sequence, index + 1);
                assert_eq!(attempt.source_sha256, source_digest);
                let source_template = super::validate_branch_fixture(&source).unwrap();
                let group = match attempt.deleted_nop_group.as_str() {
                    "before_rax" => super::NopGroup::BeforeRax,
                    "between_moves" => super::NopGroup::BetweenMoves,
                    "after_rbx" => super::NopGroup::AfterRbx,
                    other => panic!("unexpected NOP group {other}"),
                };
                let candidate = super::delete_group_nop(&source, source_template, group).unwrap();
                let candidate_bytes =
                    super::decode_hex(&candidate.memory[0].initial_bytes_hex).unwrap();
                let source_bytes = super::decode_hex(&source.memory[0].initial_bytes_hex).unwrap();
                let candidate_digest = sha256_hex(&candidate.to_json().unwrap());
                assert_eq!(attempt.candidate_sha256, candidate_digest);

                let target = attempt
                    .instruction_map
                    .iter()
                    .find(|relocation| relocation.instruction == "mov_rbx_target")
                    .unwrap();
                let branch = attempt
                    .instruction_map
                    .iter()
                    .find(|relocation| relocation.instruction == "jcc")
                    .unwrap();
                assert_eq!(
                    &source_bytes[target.source_offset..target.source_offset + 2],
                    &[0x48, 0xbb]
                );
                assert_eq!(
                    &candidate_bytes[target.candidate_offset..target.candidate_offset + 2],
                    &[0x48, 0xbb]
                );
                assert_eq!(
                    &source_bytes[target.source_offset..target.source_offset + 10],
                    &candidate_bytes[target.candidate_offset..target.candidate_offset + 10]
                );
                assert_eq!(branch.source_offset, 0);
                assert_eq!(branch.candidate_offset, 0);
                assert_eq!(
                    target.candidate_offset,
                    branch.candidate_offset + 2 + candidate_bytes[1] as usize
                );
                assert_eq!(
                    target.source_address.get(),
                    source.initial_state.rip.get() + target.source_offset as u64
                );
                assert_eq!(
                    target.candidate_address.get(),
                    candidate.initial_state.rip.get() + target.candidate_offset as u64
                );
                assert_eq!(candidate_bytes.len() + 1, source_bytes.len());
                assert_eq!(candidate.memory[0].length as usize, candidate_bytes.len());
                assert_eq!(
                    candidate.memory[0].comparison_mask_hex,
                    "ff".repeat(candidate_bytes.len())
                );
                assert_eq!(
                    candidate.stop_rip,
                    Some(crate::HexU64(
                        candidate.initial_state.rip.get() + candidate_bytes.len() as u64
                    ))
                );
                assert_eq!(candidate.initial_state, source.initial_state);
                assert_eq!(candidate.instruction_budget, source.instruction_budget);
                assert_eq!(candidate.profile, source.profile);
                assert_eq!(candidate.expected_exit, source.expected_exit);
                assert_eq!(candidate.memory[0].region_id, source.memory[0].region_id);
                assert_eq!(
                    candidate.memory[0].permissions,
                    source.memory[0].permissions
                );
                assert_eq!(
                    candidate.memory[0].guest_address,
                    source.memory[0].guest_address
                );

                source = candidate;
                source_digest = candidate_digest;
            }
            assert_eq!(
                reduction.initial_fingerprint,
                reduction.candidate_attempts[0].observed_fingerprint
            );
            assert_eq!(reduction.final_fingerprint, reduction.initial_fingerprint);
            assert!(
                reduction.candidate_attempts.iter().all(|attempt| {
                    attempt.observed_fingerprint == reduction.initial_fingerprint
                })
            );

            let reduced = reduction.reduced_fixture.unwrap();
            let template = super::validate_branch_fixture(&reduced).unwrap();
            assert_eq!(template.total_nops(), 0);
            assert_eq!(template.a, 0xfeed_face_cafe_beef);
            assert_eq!(template.b, 0xaabb_ccdd_0123_4567);
            assert_eq!(reduced.initial_state, fixture.initial_state);
            assert_eq!(reduced.instruction_budget, fixture.instruction_budget);
            assert_eq!(reduced.profile, fixture.profile);
            let reduced_code = super::decode_hex(&reduced.memory[0].initial_bytes_hex).unwrap();
            assert_eq!(reduced_code.len(), 22);
            assert_eq!(reduced.memory[0].length, 22);
            assert_eq!(reduced.memory[0].comparison_mask_hex, "ff".repeat(22));
            assert_eq!(
                reduced.stop_rip,
                Some(crate::HexU64(fixture.initial_state.rip.get() + 22))
            );
            let target_offset = 2 + usize::from(reduced_code[1]);
            assert_eq!(
                &reduced_code[target_offset..target_offset + 2],
                &[0x48, 0xbb]
            );
            assert_eq!(reduced_code[1], 10);
            assert_eq!(reduction.initial_results[0].input_sha256, digest);
            let reduced_digest = sha256_hex(&reduced.to_json().unwrap());
            for result in &reduction.initial_results {
                assert_eq!(result.backend, backend);
                assert_eq!(result.input_sha256, digest);
            }
            for result in &reduction.final_results {
                assert_eq!(result.backend, backend);
                assert_eq!(result.input_sha256, reduced_digest);
            }
        }
    }

    #[test]
    fn exact_interpreter_result_has_no_initial_mismatch() {
        let fixture = template_fixture(BranchOpcode::Jnz, false, 1, 0, 1);
        let digest = sha256_hex(JE_TAKEN);
        let expected_path =
            super::expected_path(&fixture, super::validate_branch_fixture(&fixture).unwrap());
        let reduction =
            reduce_branch_fixture_with_input_digest(&fixture, &digest, 4, |candidate| {
                let candidate_digest = if candidate == &fixture {
                    digest.clone()
                } else {
                    sha256_hex(&candidate.to_json().unwrap())
                };
                let path = super::expected_path(
                    candidate,
                    super::validate_branch_fixture(candidate).unwrap(),
                );
                Ok(synthetic_result(candidate, &candidate_digest, path))
            })
            .unwrap();
        assert_eq!(expected_path, BranchPath::Taken);
        assert_eq!(reduction.stop, BranchReductionStop::NoInitialMismatch);
        assert_eq!(reduction.actual_callbacks, 2);
        assert_eq!(reduction.initial_results[0].input_sha256, digest);
        assert_eq!(
            reduction.expected_original.input_sha256,
            reduction.initial_results[0].input_sha256
        );
        assert!(reduction.reduced_fixture.is_none());
    }

    #[test]
    fn backend_parameter_preserves_native_identity_and_rejects_interpreter_result() {
        let fixture = template_fixture(BranchOpcode::Jz, true, 1, 0, 0);
        let digest = sha256_hex(JE_TAKEN);
        let native = super::reduce_branch_fixture_with_backend_input_digest(
            &fixture,
            &digest,
            4,
            Backend::NativeX86,
            |candidate| {
                let candidate_digest = if candidate == &fixture {
                    digest.clone()
                } else {
                    sha256_hex(&candidate.to_json().unwrap())
                };
                let template = super::validate_branch_fixture(candidate).unwrap();
                let path = super::expected_path(candidate, template);
                Ok(synthetic_result_backend(
                    candidate,
                    &candidate_digest,
                    path,
                    Backend::NativeX86,
                ))
            },
        )
        .unwrap();
        assert_eq!(native.stop, BranchReductionStop::NoInitialMismatch);
        assert!(
            native
                .initial_results
                .iter()
                .all(|result| result.backend == Backend::NativeX86)
        );

        let wrong_backend = super::reduce_branch_fixture_with_backend_input_digest(
            &fixture,
            &digest,
            4,
            Backend::NativeX86,
            |candidate| {
                let candidate_digest = if candidate == &fixture {
                    digest.clone()
                } else {
                    sha256_hex(&candidate.to_json().unwrap())
                };
                let template = super::validate_branch_fixture(candidate).unwrap();
                let path = super::expected_path(candidate, template);
                Ok(synthetic_result_backend(
                    candidate,
                    &candidate_digest,
                    path,
                    Backend::Interpreter,
                ))
            },
        )
        .unwrap();
        assert_eq!(wrong_backend.stop, BranchReductionStop::UnsupportedMismatch);
    }

    #[test]
    fn native_backend_swaps_fail_closed_on_candidate_and_final_confirmation() {
        let fixture = template_fixture(BranchOpcode::Jz, true, 1, 0, 0);
        let digest = sha256_hex(JE_TAKEN);

        let mut candidate_calls = 0;
        let candidate_swap = super::reduce_branch_fixture_with_backend_input_digest(
            &fixture,
            &digest,
            BRANCH_REDUCER_MAX_BUDGET,
            Backend::NativeX86,
            |candidate| {
                candidate_calls += 1;
                let result_backend = if candidate_calls <= 2 {
                    Backend::NativeX86
                } else {
                    Backend::Interpreter
                };
                let candidate_digest = if candidate == &fixture {
                    digest.clone()
                } else {
                    sha256_hex(&candidate.to_json().unwrap())
                };
                let template = super::validate_branch_fixture(candidate).unwrap();
                Ok(synthetic_result_backend(
                    candidate,
                    &candidate_digest,
                    super::opposite(super::expected_path(candidate, template)),
                    result_backend,
                ))
            },
        )
        .unwrap();
        assert_eq!(candidate_swap.stop, BranchReductionStop::CallbackError);
        assert_eq!(candidate_swap.actual_callbacks, 4);
        assert_eq!(candidate_swap.candidate_runs, 2);
        assert_eq!(candidate_swap.candidate_attempts.len(), 1);
        assert_eq!(
            candidate_swap.candidate_attempts[0].decision,
            super::BranchCandidateDecision::Error
        );
        assert!(candidate_swap.reduced_fixture.is_none());

        let mut final_calls = 0;
        let final_swap = super::reduce_branch_fixture_with_backend_input_digest(
            &fixture,
            &digest,
            BRANCH_REDUCER_MAX_BUDGET,
            Backend::NativeX86,
            |candidate| {
                final_calls += 1;
                let result_backend = if final_calls <= 4 {
                    Backend::NativeX86
                } else {
                    Backend::Interpreter
                };
                let candidate_digest = if candidate == &fixture {
                    digest.clone()
                } else {
                    sha256_hex(&candidate.to_json().unwrap())
                };
                let template = super::validate_branch_fixture(candidate).unwrap();
                Ok(synthetic_result_backend(
                    candidate,
                    &candidate_digest,
                    super::opposite(super::expected_path(candidate, template)),
                    result_backend,
                ))
            },
        )
        .unwrap();
        assert_eq!(
            final_swap.stop,
            BranchReductionStop::FinalConfirmationFailed
        );
        assert_eq!(final_swap.actual_callbacks, 6);
        assert_eq!(final_swap.candidate_runs, 2);
        assert_eq!(final_swap.candidate_attempts.len(), 1);
        assert_eq!(
            final_swap.candidate_attempts[0].decision,
            super::BranchCandidateDecision::Accepted
        );
        assert!(
            final_swap
                .final_results
                .iter()
                .all(|result| result.backend == Backend::Interpreter)
        );
        assert!(final_swap.reduced_fixture.is_none());
    }

    #[test]
    fn candidate_budget_reserves_two_final_confirmation_calls() {
        let fixture = template_fixture(BranchOpcode::Jz, true, 2, 0, 0);
        let digest = sha256_hex(JE_TAKEN);
        let reduction =
            reduce_branch_fixture_with_input_digest(&fixture, &digest, 6, |candidate| {
                let candidate_digest = if candidate == &fixture {
                    digest.clone()
                } else {
                    sha256_hex(&candidate.to_json().unwrap())
                };
                let template = super::validate_branch_fixture(candidate).unwrap();
                let path = super::opposite(super::expected_path(candidate, template));
                Ok(synthetic_result(candidate, &candidate_digest, path))
            })
            .unwrap();
        assert_eq!(
            reduction.stop,
            BranchReductionStop::CandidateBudgetExhausted
        );
        assert_eq!(reduction.actual_callbacks, 6);
        assert_eq!(reduction.candidate_proposals, 1);
        assert_eq!(reduction.final_results.len(), 2);
    }

    #[test]
    fn final_confirmation_must_repeat_the_initial_fingerprint() {
        let fixture = template_fixture(BranchOpcode::Jnz, false, 1, 0, 0);
        let digest = sha256_hex(JE_TAKEN);
        let mut calls = 0;
        let reduction =
            reduce_branch_fixture_with_input_digest(&fixture, &digest, 6, |candidate| {
                calls += 1;
                let candidate_digest = if candidate == &fixture {
                    digest.clone()
                } else {
                    sha256_hex(&candidate.to_json().unwrap())
                };
                let template = super::validate_branch_fixture(candidate).unwrap();
                let expected = super::expected_path(candidate, template);
                let path = if calls <= 4 {
                    super::opposite(expected)
                } else {
                    expected
                };
                Ok(synthetic_result(candidate, &candidate_digest, path))
            })
            .unwrap();
        assert_eq!(reduction.stop, BranchReductionStop::FinalConfirmationFailed);
        assert_eq!(reduction.final_results.len(), 2);
        assert!(reduction.reduced_fixture.is_none());
    }

    #[test]
    fn arbitrary_state_drift_is_unsupported_even_when_path_looks_wrong() {
        let fixture = template_fixture(BranchOpcode::Jz, true, 0, 0, 0);
        let digest = sha256_hex(JE_TAKEN);
        let reduction =
            reduce_branch_fixture_with_input_digest(&fixture, &digest, 4, |candidate| {
                let candidate_digest = if candidate == &fixture {
                    digest.clone()
                } else {
                    sha256_hex(&candidate.to_json().unwrap())
                };
                let template = super::validate_branch_fixture(candidate).unwrap();
                let path = super::expected_path(candidate, template);
                let mut result = synthetic_result(candidate, &candidate_digest, path);
                if let Observation::Available { value } = &mut result.snapshot {
                    value.state.gpr[1] = crate::HexU64(0xfeed);
                }
                Ok(result)
            })
            .unwrap();
        assert_eq!(reduction.stop, BranchReductionStop::UnsupportedMismatch);
        assert_eq!(reduction.actual_callbacks, 2);
    }

    #[test]
    fn malformed_candidate_result_keeps_attempt_and_partial_result_hashes() {
        let fixture = template_fixture(BranchOpcode::Jz, true, 1, 0, 0);
        let digest = sha256_hex(JE_TAKEN);
        let reduction =
            reduce_branch_fixture_with_input_digest(&fixture, &digest, 6, |candidate| {
                let candidate_digest = if candidate == &fixture {
                    digest.clone()
                } else {
                    sha256_hex(&candidate.to_json().unwrap())
                };
                let template = super::validate_branch_fixture(candidate).unwrap();
                let path = super::opposite(super::expected_path(candidate, template));
                let mut result = synthetic_result(candidate, &candidate_digest, path);
                if candidate != &fixture {
                    result.schema_version = 0;
                }
                Ok(result)
            })
            .unwrap();
        assert_eq!(reduction.stop, BranchReductionStop::CallbackError);
        assert_eq!(reduction.candidate_attempts.len(), 1);
        assert_eq!(
            reduction.candidate_attempts[0].decision,
            super::BranchCandidateDecision::Error
        );
        assert_eq!(reduction.candidate_attempts[0].result_sha256.len(), 2);
        assert_eq!(reduction.candidate_attempts[0].callbacks, 2);
    }

    #[test]
    fn rejects_rel8_targets_into_instruction_and_page_crossing_code() {
        let fixture = template_fixture(BranchOpcode::Jz, true, 0, 0, 0);
        let mut bad = fixture.clone();
        bad.memory[0].initial_bytes_hex.replace_range(2..4, "");
        bad.memory[0].initial_bytes_hex =
            "740148b8".to_string() + &"00".repeat(8) + "48bb" + &"00".repeat(8);
        bad.memory[0].length = 22;
        bad.memory[0].comparison_mask_hex = "ff".repeat(22);
        bad.stop_rip = Some(crate::HexU64(bad.initial_state.rip.get() + 22));
        assert!(super::validate_branch_fixture(&bad).is_err());

        let mut crossing = fixture;
        crossing.initial_state.rip = crate::HexU64(0x10_0ff0);
        crossing.memory[0].guest_address = crossing.initial_state.rip;
        assert!(super::validate_branch_fixture(&crossing).is_err());
    }

    #[test]
    fn unsupported_result_drift_and_callback_failure_do_not_preserve_mismatch() {
        let fixture = template_fixture(BranchOpcode::Jz, true, 1, 1, 0);
        let digest = sha256_hex(JE_TAKEN);
        let mut calls = 0;
        let flaky = reduce_branch_fixture_with_input_digest(&fixture, &digest, 8, |candidate| {
            calls += 1;
            let candidate_digest = if candidate == &fixture {
                digest.clone()
            } else {
                sha256_hex(&candidate.to_json().unwrap())
            };
            let template = super::validate_branch_fixture(candidate).unwrap();
            let expected = super::expected_path(candidate, template);
            let path = if calls == 1 {
                super::opposite(expected)
            } else {
                expected
            };
            Ok(synthetic_result(candidate, &candidate_digest, path))
        })
        .unwrap();
        assert_eq!(flaky.stop, BranchReductionStop::FlakyInitial);

        let failed = reduce_branch_fixture_with_input_digest(&fixture, &digest, 4, |_| {
            Err("callback unavailable".into())
        })
        .unwrap();
        assert_eq!(failed.stop, BranchReductionStop::CallbackError);
        assert_eq!(failed.actual_callbacks, 1);
    }
}
