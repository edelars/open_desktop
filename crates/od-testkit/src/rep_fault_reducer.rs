//! Bounded interpreter reducer for REP STOSB reaching the canonical48 upper boundary.

use crate::HexU64;
use crate::result::{Backend, ExceptionSnapshot, MemoryRegionSnapshot, Observation};
use crate::sha256_hex;
use crate::v2::{
    ExpectedExit, Fixture, IntegerState, LoadedFixture, Mode, Outcome, Profile,
    RunResult as V2RunResult,
};
use serde::{Deserialize, Serialize};

pub const REP_FAULT_REDUCER_VERSION: &str = "od-testkit-s2-42l-rep-stosb-canonical48-v1";
pub const REP_FAULT_REDUCER_DEFAULT_BUDGET: usize = 20;
pub const REP_FAULT_REDUCER_MAX_BUDGET: usize = 20;
const CANONICAL48_UPPER: u64 = 0x0000_7fff_ffff_ffff;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepFaultFingerprint {
    /// Offsets in the mapped destination whose observed values differ from AL.
    pub wrong_destination_byte_positions: Vec<usize>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepFaultReductionStop {
    Reduced,
    NoReduction,
    NoInitialMismatch,
    FlakyInitial,
    FlakyCandidate,
    CallbackBudgetExhausted,
    CallbackError,
    FinalConfirmationFailed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepFaultExpectedResult {
    pub case_id: String,
    pub profile: Profile,
    pub input_sha256: String,
    pub outcome: Outcome,
    pub instructions_executed: HexU64,
    pub state: IntegerState,
    pub rflags_defined: HexU64,
    pub memory: Vec<MemoryRegionSnapshot>,
    pub exception: ExceptionSnapshot,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepFaultReduction {
    pub stop: RepFaultReductionStop,
    pub stop_detail: Option<String>,
    pub original_fixture: Fixture,
    pub reduced_fixture: Option<Fixture>,
    pub initial_results: Vec<V2RunResult>,
    pub final_results: Vec<V2RunResult>,
    pub expected_original: RepFaultExpectedResult,
    pub expected_reduced: Option<RepFaultExpectedResult>,
    pub initial_fingerprint: Option<RepFaultFingerprint>,
    pub final_fingerprint: Option<RepFaultFingerprint>,
    pub actual_callbacks: usize,
    pub candidate_proposals: usize,
    pub candidate_runs: usize,
    pub accepted_candidates: usize,
    pub initial_hamming_weight: u32,
    pub final_hamming_weight: Option<u32>,
}

/// Reduce AL only. The callback is deliberately owned by the caller so the reducer cannot
/// silently switch backends or turn infrastructure failures into guest mismatches.
pub fn reduce_rep_fault_fixture<F>(
    original: &Fixture,
    original_input_sha256: &str,
    call_budget: usize,
    mut execute: F,
) -> Result<RepFaultReduction, String>
where
    F: FnMut(&Fixture) -> Result<V2RunResult, String>,
{
    if !(4..=REP_FAULT_REDUCER_MAX_BUDGET).contains(&call_budget) {
        return Err(format!(
            "REP fault reducer call budget must be 4..={REP_FAULT_REDUCER_MAX_BUDGET}"
        ));
    }
    if !is_sha256(original_input_sha256) {
        return Err("original input digest must be lowercase SHA-256".into());
    }
    validate_fixture(original)?;
    let expected_original = rep_fault_oracle(original, original_input_sha256)?;
    let initial_weight = (original.initial_state.gpr[0].get() as u8).count_ones();
    let mut report = RepFaultReduction {
        stop: RepFaultReductionStop::NoInitialMismatch,
        stop_detail: None,
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
        Ok(value) => value,
        Err(error) => return callback_error(&mut report, error),
    };
    let second = match invoke(&mut execute, original, &mut report.actual_callbacks) {
        Ok(value) => value,
        Err(error) => {
            report.initial_results.push(first);
            return callback_error(&mut report, error);
        }
    };
    report
        .initial_results
        .extend([first.clone(), second.clone()]);
    if !same_semantic_result(&first, &second) {
        report.stop = RepFaultReductionStop::FlakyInitial;
        report.stop_detail = Some("initial interpreter results differ semantically".into());
        return Ok(report);
    }
    let digest = original_input_sha256;
    let first_fp = match mismatch_fingerprint(original, digest, &first) {
        Ok(value) => value,
        Err(error) => return callback_error(&mut report, error),
    };
    let second_fp = match mismatch_fingerprint(original, digest, &second) {
        Ok(value) => value,
        Err(error) => return callback_error(&mut report, error),
    };
    if first_fp != second_fp {
        report.stop = RepFaultReductionStop::FlakyInitial;
        report.stop_detail = Some("initial mismatch fingerprints differ".into());
        return Ok(report);
    }
    let Some(initial_fp) = first_fp else {
        report.stop = RepFaultReductionStop::NoInitialMismatch;
        return Ok(report);
    };
    report.initial_fingerprint = Some(initial_fp.clone());

    let mut current = original.clone();
    let mut budget_exhausted = false;
    for bit in (0..8).rev() {
        if !has_al_bit(&current, bit) {
            continue;
        }
        if report.actual_callbacks.saturating_add(4) > call_budget {
            budget_exhausted = true;
            break;
        }
        let candidate = clear_al_bit(&current, bit);
        candidate
            .validate()
            .map_err(|error| format!("generated REP fault candidate is invalid: {error}"))?;
        report.candidate_proposals += 1;
        report.candidate_runs += 1;
        let candidate_digest = sha256_hex(&candidate.to_json().map_err(|error| error.to_string())?);
        let first = match invoke(&mut execute, &candidate, &mut report.actual_callbacks) {
            Ok(value) => value,
            Err(error) => return callback_error(&mut report, error),
        };
        let second = match invoke(&mut execute, &candidate, &mut report.actual_callbacks) {
            Ok(value) => value,
            Err(error) => return callback_error(&mut report, error),
        };
        if !same_semantic_result(&first, &second) {
            report.stop = RepFaultReductionStop::FlakyCandidate;
            report.stop_detail = Some("candidate interpreter results differ semantically".into());
            return Ok(report);
        }
        let first_candidate_fp = match mismatch_fingerprint(&candidate, &candidate_digest, &first) {
            Ok(value) => value,
            Err(error) => return callback_error(&mut report, error),
        };
        let second_candidate_fp = match mismatch_fingerprint(&candidate, &candidate_digest, &second)
        {
            Ok(value) => value,
            Err(error) => return callback_error(&mut report, error),
        };
        if first_candidate_fp != second_candidate_fp {
            report.stop = RepFaultReductionStop::FlakyCandidate;
            report.stop_detail = Some("candidate mismatch fingerprints differ".into());
            return Ok(report);
        }
        if first_candidate_fp.as_ref() == Some(&initial_fp) {
            current = candidate;
            report.accepted_candidates += 1;
        }
    }

    if report.actual_callbacks.saturating_add(2) > call_budget {
        report.stop = RepFaultReductionStop::CallbackBudgetExhausted;
        report.stop_detail = Some("insufficient callback budget for final confirmation".into());
        return Ok(report);
    }
    let final_digest = if current == *original {
        original_input_sha256.to_string()
    } else {
        sha256_hex(&current.to_json().map_err(|error| error.to_string())?)
    };
    let final_first = match invoke(&mut execute, &current, &mut report.actual_callbacks) {
        Ok(value) => value,
        Err(error) => return callback_error(&mut report, error),
    };
    let final_second = match invoke(&mut execute, &current, &mut report.actual_callbacks) {
        Ok(value) => value,
        Err(error) => {
            report.final_results.push(final_first);
            return callback_error(&mut report, error);
        }
    };
    report
        .final_results
        .extend([final_first.clone(), final_second.clone()]);
    if !same_semantic_result(&final_first, &final_second) {
        report.stop = RepFaultReductionStop::FinalConfirmationFailed;
        report.stop_detail = Some("final interpreter results differ semantically".into());
        return Ok(report);
    }
    let final_first_fp = match mismatch_fingerprint(&current, &final_digest, &final_first) {
        Ok(value) => value,
        Err(error) => return callback_error(&mut report, error),
    };
    let final_second_fp = match mismatch_fingerprint(&current, &final_digest, &final_second) {
        Ok(value) => value,
        Err(error) => return callback_error(&mut report, error),
    };
    if final_first_fp != Some(initial_fp.clone()) || final_second_fp != Some(initial_fp.clone()) {
        report.stop = RepFaultReductionStop::FinalConfirmationFailed;
        report.stop_detail =
            Some("final results do not preserve the initial destination-byte mismatch".into());
        return Ok(report);
    }
    report.stop = if budget_exhausted {
        report.stop_detail =
            Some("candidate budget exhausted; current mismatch was finally confirmed".into());
        RepFaultReductionStop::CallbackBudgetExhausted
    } else if current == *original {
        RepFaultReductionStop::NoReduction
    } else {
        RepFaultReductionStop::Reduced
    };
    report.final_fingerprint = Some(initial_fp);
    report.final_hamming_weight = Some((current.initial_state.gpr[0].get() as u8).count_ones());
    report.expected_reduced = Some(rep_fault_oracle(&current, &final_digest)?);
    report.reduced_fixture = Some(current);
    Ok(report)
}

/// Independent expected partial progress for the narrowly admitted canonical48 boundary case.
pub fn rep_fault_oracle(
    fixture: &Fixture,
    input_sha256: &str,
) -> Result<RepFaultExpectedResult, String> {
    if !is_sha256(input_sha256) {
        return Err("fixture digest must be lowercase SHA-256".into());
    }
    validate_fixture(fixture)?;
    let mut state = fixture.initial_state.clone();
    let k = fixture
        .memory
        .iter()
        .find(|region| region.region_id == "data")
        .unwrap()
        .length as u64;
    state.gpr[1] = HexU64(state.gpr[1].get() - k);
    state.gpr[7] = HexU64(CANONICAL48_UPPER + 1);
    let al = fixture.initial_state.gpr[0].get() as u8;
    let memory = fixture
        .memory
        .iter()
        .map(|region| {
            let bytes = if region.region_id == "data" {
                vec![al; k as usize]
            } else {
                decode_hex(&region.initial_bytes_hex)?
            };
            Ok(MemoryRegionSnapshot {
                region_id: region.region_id.clone(),
                guest_address: region.guest_address,
                bytes_hex: encode_hex(&bytes),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(RepFaultExpectedResult {
        case_id: fixture.case_id.clone(),
        profile: fixture.profile,
        input_sha256: input_sha256.into(),
        outcome: Outcome::Exception {},
        instructions_executed: HexU64(0),
        state,
        rflags_defined: fixture.rflags_mask,
        memory,
        exception: ExceptionSnapshot {
            vector: 13,
            error_code: Observation::Available { value: HexU64(0) },
            faulting_rip: fixture.initial_state.rip,
            fault_address: Observation::Unavailable {
                reason: "exception has no reported linear address".into(),
            },
        },
    })
}

fn validate_fixture(fixture: &Fixture) -> Result<(), String> {
    fixture.validate().map_err(|error| error.to_string())?;
    if fixture.profile != Profile::LongIntegerV1 || fixture.initial_state.mode != Mode::Long64 {
        return Err("REP fault reducer accepts only Long64 s2-long-integer-v1 fixtures".into());
    }
    if fixture.expected_exit != ExpectedExit::Exception || fixture.instruction_budget != 1 {
        return Err(
            "REP fault reducer requires expected exception and instruction budget 1".into(),
        );
    }
    let entry = fixture.initial_state.rip.get();
    if fixture.stop_rip.map(HexU64::get) != entry.checked_add(2) {
        return Err("REP fault reducer requires stop_rip=entry+2".into());
    }
    if fixture.memory.len() != 2 {
        return Err("REP fault reducer requires exactly code and data regions".into());
    }
    let code = fixture
        .memory
        .iter()
        .find(|region| region.region_id == "code")
        .ok_or("missing code region")?;
    let data = fixture
        .memory
        .iter()
        .find(|region| region.region_id == "data")
        .ok_or("missing data region")?;
    if code.guest_address.get() != entry
        || code.length != 2
        || code.initial_bytes_hex != "f3aa"
        || code.permissions
            != (crate::case::MemoryPermissions {
                read: false,
                write: false,
                execute: true,
            })
        || decode_hex(&code.comparison_mask_hex)? != [0xff, 0xff]
    {
        return Err("code must be exact RX F3 AA at entry with a complete comparison mask".into());
    }
    let k = data.length as usize;
    if !(1..=7).contains(&k)
        || data.guest_address.get() != CANONICAL48_UPPER - k as u64 + 1
        || data.permissions
            != (crate::case::MemoryPermissions {
                read: true,
                write: true,
                execute: false,
            })
        || decode_hex(&data.initial_bytes_hex)? != vec![0xff; k]
        || decode_hex(&data.comparison_mask_hex)? != vec![0xff; k]
    {
        return Err("data must be k=1..7 fully observed RW bytes ending at canonical48 upper boundary, initialized to FF".into());
    }
    let n = fixture.initial_state.gpr[1].get();
    if !(k as u64 + 1..=8).contains(&n) {
        return Err("RCX must satisfy k < n <= 8".into());
    }
    if fixture.initial_state.gpr[7].get() != data.guest_address.get() {
        return Err("initial RDI must equal data start".into());
    }
    let al = fixture.initial_state.gpr[0].get() as u8;
    if al == 0xff {
        return Err("initial AL must be in 00..FE".into());
    }
    if fixture.initial_state.rflags.get() & (1 << 10) != 0 {
        return Err("REP fault reducer requires DF=0".into());
    }
    Ok(())
}

fn mismatch_fingerprint(
    fixture: &Fixture,
    digest: &str,
    result: &V2RunResult,
) -> Result<Option<RepFaultFingerprint>, String> {
    if result.profile != Profile::LongIntegerV1
        || result.backend != Backend::Interpreter
        || result.case_id != fixture.case_id
        || result.input_sha256 != digest
    {
        return Err(format!(
            "callback result identity does not match exact interpreter fixture (profile={}, backend={}, case_id={}, digest={})",
            result.profile == Profile::LongIntegerV1,
            result.backend == Backend::Interpreter,
            result.case_id == fixture.case_id,
            result.input_sha256 == digest
        ));
    }
    result
        .validate_for(&LoadedFixture {
            fixture: fixture.clone(),
            input_sha256: digest.to_string(),
        })
        .map_err(|error| format!("invalid interpreter result: {error}"))?;
    let expected = rep_fault_oracle(fixture, digest)?;
    if result.outcome != expected.outcome
        || result.instructions_executed
            != (Observation::Available {
                value: expected.instructions_executed,
            })
    {
        return Err(
            "callback did not produce the expected #GP exception with zero retirement".into(),
        );
    }
    let Observation::Available { value: actual } = &result.snapshot else {
        return Err("callback has no complete architectural snapshot".into());
    };
    let Some(exception) = (match &actual.exception {
        Observation::Available { value } => value.as_ref(),
        Observation::Unavailable { .. } => None,
    }) else {
        return Err("callback omitted the #GP exception snapshot".into());
    };
    if exception.vector != 13
        || exception.error_code != (Observation::Available { value: HexU64(0) })
        || exception.faulting_rip != fixture.initial_state.rip
        || exception.fault_address.is_available()
    {
        return Err("callback fault class/progress changed; expected #GP(0) at entry without a linear address".into());
    }
    if actual.state != expected.state {
        return Err(
            "callback architectural state/progress differs from the independent REP oracle".into(),
        );
    }
    if actual.rflags_defined != expected.rflags_defined {
        return Err("callback flags-defined mask changed".into());
    }
    if actual.memory.len() != expected.memory.len() {
        return Err("callback memory-region count changed".into());
    }
    let mut wrong = Vec::new();
    for (actual_region, expected_region) in actual.memory.iter().zip(&expected.memory) {
        if actual_region.region_id != expected_region.region_id
            || actual_region.guest_address != expected_region.guest_address
        {
            return Err("callback memory layout changed".into());
        }
        let actual_bytes = decode_hex(&actual_region.bytes_hex)?;
        let expected_bytes = decode_hex(&expected_region.bytes_hex)?;
        if actual_bytes.len() != expected_bytes.len() {
            return Err("callback memory length changed".into());
        }
        if actual_region.region_id == "data" {
            for (offset, (a, e)) in actual_bytes.iter().zip(expected_bytes).enumerate() {
                if *a != e {
                    wrong.push(offset);
                }
            }
        } else if actual_bytes != expected_bytes {
            return Err("callback changed code bytes".into());
        }
    }
    Ok((!wrong.is_empty()).then_some(RepFaultFingerprint {
        wrong_destination_byte_positions: wrong,
    }))
}

fn clear_al_bit(fixture: &Fixture, bit: u8) -> Fixture {
    let mut candidate = fixture.clone();
    let value = candidate.initial_state.gpr[0].get() & !(1u64 << bit);
    candidate.initial_state.gpr[0] = HexU64(value);
    candidate
}
fn has_al_bit(fixture: &Fixture, bit: u8) -> bool {
    fixture.initial_state.gpr[0].get() & (1u64 << bit) != 0
}
fn same_semantic_result(a: &V2RunResult, b: &V2RunResult) -> bool {
    a.schema_version == b.schema_version
        && a.case_id == b.case_id
        && a.input_sha256 == b.input_sha256
        && a.profile == b.profile
        && a.backend == b.backend
        && a.outcome == b.outcome
        && a.instructions_executed == b.instructions_executed
        && a.snapshot == b.snapshot
}
fn invoke<F>(execute: &mut F, fixture: &Fixture, count: &mut usize) -> Result<V2RunResult, String>
where
    F: FnMut(&Fixture) -> Result<V2RunResult, String>,
{
    *count += 1;
    execute(fixture)
}
fn callback_error(
    report: &mut RepFaultReduction,
    error: String,
) -> Result<RepFaultReduction, String> {
    report.stop = RepFaultReductionStop::CallbackError;
    report.stop_detail = Some(error);
    Ok(report.clone())
}
fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if value.len() % 2 != 0 {
        return Err("hex byte string has odd length".into());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).map_err(|_| "invalid hex encoding")?;
            u8::from_str_radix(text, 16).map_err(|_| "invalid hex encoding".into())
        })
        .collect()
}
fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
