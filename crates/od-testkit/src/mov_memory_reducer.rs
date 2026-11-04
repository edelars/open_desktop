//! Bounded schema-v2 reducer for exact `MOV qword ptr [RBX], RAX` fixtures.

use crate::HexU64;
use crate::result::{Backend, MemoryRegionSnapshot, Observation};
use crate::sha256_hex;
use crate::v2::{
    ExpectedExit, Fixture, IntegerState, LoadedFixture, Mode, Outcome, Profile, RunResult,
};
use serde::{Deserialize, Serialize};

pub const MOV_MEMORY_REDUCER_VERSION: &str = "od-testkit-s2-42n-mov-rbx-rax-v1";
pub const MOV_MEMORY_REDUCER_DEFAULT_BUDGET: usize = 132;
pub const MOV_MEMORY_REDUCER_MAX_BUDGET: usize = 132;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MovMemoryFingerprint {
    pub wrong_destination_byte_offsets: Vec<usize>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MovMemoryReductionStop {
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
pub struct MovMemoryExpectedResult {
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
pub struct MovMemoryReduction {
    pub stop: MovMemoryReductionStop,
    pub stop_detail: Option<String>,
    pub original_fixture: Fixture,
    pub reduced_fixture: Option<Fixture>,
    pub initial_results: Vec<RunResult>,
    pub final_results: Vec<RunResult>,
    pub expected_original: MovMemoryExpectedResult,
    pub expected_reduced: Option<MovMemoryExpectedResult>,
    pub initial_fingerprint: Option<MovMemoryFingerprint>,
    pub final_fingerprint: Option<MovMemoryFingerprint>,
    pub actual_callbacks: usize,
    pub candidate_proposals: usize,
    pub candidate_runs: usize,
    pub accepted_candidates: usize,
    pub initial_hamming_weight: u32,
    pub final_hamming_weight: Option<u32>,
}

/// Clears set bits in RAX in a deterministic high-to-low schedule. Execution is caller-owned.
pub fn reduce_mov_memory_fixture<F>(
    original: &Fixture,
    original_input_sha256: &str,
    call_budget: usize,
    execute: F,
) -> Result<MovMemoryReduction, String>
where
    F: FnMut(&Fixture) -> Result<RunResult, String>,
{
    reduce_mov_memory_fixture_with_backend(
        original,
        original_input_sha256,
        call_budget,
        Backend::Interpreter,
        execute,
    )
}

/// Native-X86 adapter for the same bounded reducer and independent MOV oracle.
pub fn reduce_mov_memory_fixture_with_native_backend<F>(
    original: &Fixture,
    original_input_sha256: &str,
    call_budget: usize,
    execute: F,
) -> Result<MovMemoryReduction, String>
where
    F: FnMut(&Fixture) -> Result<RunResult, String>,
{
    reduce_mov_memory_fixture_with_backend(
        original,
        original_input_sha256,
        call_budget,
        Backend::NativeX86,
        execute,
    )
}

fn reduce_mov_memory_fixture_with_backend<F>(
    original: &Fixture,
    original_input_sha256: &str,
    call_budget: usize,
    expected_backend: Backend,
    mut execute: F,
) -> Result<MovMemoryReduction, String>
where
    F: FnMut(&Fixture) -> Result<RunResult, String>,
{
    if !(4..=MOV_MEMORY_REDUCER_MAX_BUDGET).contains(&call_budget) {
        return Err(format!(
            "MOV memory reducer call budget must be 4..={MOV_MEMORY_REDUCER_MAX_BUDGET}"
        ));
    }
    if !is_sha256(original_input_sha256) {
        return Err("original input digest must be lowercase SHA-256".into());
    }
    validate_fixture(original)?;
    let expected_original = mov_memory_oracle(original, original_input_sha256)?;
    let initial_hamming_weight = original.initial_state.gpr[0].get().count_ones();
    let mut report = MovMemoryReduction {
        stop: MovMemoryReductionStop::NoInitialMismatch,
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
        initial_hamming_weight,
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
        report.stop = MovMemoryReductionStop::FlakyInitial;
        report.stop_detail = Some("initial interpreter results differ semantically".into());
        return Ok(report);
    }
    let first_fp =
        match mismatch_fingerprint(original, original_input_sha256, &first, expected_backend) {
            Ok(value) => value,
            Err(error) => return callback_error(&mut report, error),
        };
    let second_fp =
        match mismatch_fingerprint(original, original_input_sha256, &second, expected_backend) {
            Ok(value) => value,
            Err(error) => return callback_error(&mut report, error),
        };
    if first_fp != second_fp {
        report.stop = MovMemoryReductionStop::FlakyInitial;
        report.stop_detail = Some("initial mismatch fingerprints differ".into());
        return Ok(report);
    }
    let Some(initial_fp) = first_fp else {
        report.stop = MovMemoryReductionStop::NoInitialMismatch;
        return Ok(report);
    };
    report.initial_fingerprint = Some(initial_fp.clone());

    let mut current = original.clone();
    let mut budget_exhausted = false;
    for bit in (0..64).rev() {
        if current.initial_state.gpr[0].get() & (1u64 << bit) == 0 {
            continue;
        }
        if report.actual_callbacks.saturating_add(4) > call_budget {
            budget_exhausted = true;
            break;
        }
        let mut candidate = current.clone();
        candidate.initial_state.gpr[0].0 &= !(1u64 << bit);
        candidate
            .validate()
            .map_err(|error| format!("generated MOV memory candidate is invalid: {error}"))?;
        report.candidate_proposals += 1;
        report.candidate_runs += 1;
        let digest = sha256_hex(&candidate.to_json().map_err(|error| error.to_string())?);
        let first = match invoke(&mut execute, &candidate, &mut report.actual_callbacks) {
            Ok(value) => value,
            Err(error) => return callback_error(&mut report, error),
        };
        let second = match invoke(&mut execute, &candidate, &mut report.actual_callbacks) {
            Ok(value) => value,
            Err(error) => return callback_error(&mut report, error),
        };
        if !same_semantic_result(&first, &second) {
            report.stop = MovMemoryReductionStop::FlakyCandidate;
            report.stop_detail = Some("candidate interpreter results differ semantically".into());
            return Ok(report);
        }
        let first_fp = match mismatch_fingerprint(&candidate, &digest, &first, expected_backend) {
            Ok(value) => value,
            Err(error) => return callback_error(&mut report, error),
        };
        let second_fp = match mismatch_fingerprint(&candidate, &digest, &second, expected_backend) {
            Ok(value) => value,
            Err(error) => return callback_error(&mut report, error),
        };
        if first_fp != second_fp {
            report.stop = MovMemoryReductionStop::FlakyCandidate;
            report.stop_detail = Some("candidate mismatch fingerprints differ".into());
            return Ok(report);
        }
        if first_fp.as_ref() == Some(&initial_fp) {
            current = candidate;
            report.accepted_candidates += 1;
        }
    }

    if report.actual_callbacks.saturating_add(2) > call_budget {
        report.stop = MovMemoryReductionStop::CallbackBudgetExhausted;
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
        Err(error) => return final_error(&mut report, error),
    };
    let final_second = match invoke(&mut execute, &current, &mut report.actual_callbacks) {
        Ok(value) => value,
        Err(error) => {
            report.final_results.push(final_first);
            return final_error(&mut report, error);
        }
    };
    report
        .final_results
        .extend([final_first.clone(), final_second.clone()]);
    if !same_semantic_result(&final_first, &final_second) {
        report.stop = MovMemoryReductionStop::FinalConfirmationFailed;
        report.stop_detail = Some("final interpreter results differ semantically".into());
        return Ok(report);
    }
    let final_first_fp =
        match mismatch_fingerprint(&current, &final_digest, &final_first, expected_backend) {
            Ok(value) => value,
            Err(error) => return final_error(&mut report, error),
        };
    let final_second_fp =
        match mismatch_fingerprint(&current, &final_digest, &final_second, expected_backend) {
            Ok(value) => value,
            Err(error) => return final_error(&mut report, error),
        };
    if final_first_fp.as_ref() != Some(&initial_fp) || final_second_fp.as_ref() != Some(&initial_fp)
    {
        report.stop = MovMemoryReductionStop::FinalConfirmationFailed;
        report.stop_detail =
            Some("final result did not preserve the original destination-byte mismatch".into());
        return Ok(report);
    }
    report.final_fingerprint = Some(initial_fp);
    report.expected_reduced = Some(mov_memory_oracle(&current, &final_digest)?);
    report.final_hamming_weight = Some(current.initial_state.gpr[0].get().count_ones());
    if budget_exhausted {
        report.stop = MovMemoryReductionStop::CallbackBudgetExhausted;
        report.stop_detail = Some("proposal schedule stopped at the callback budget".into());
    } else if report.accepted_candidates > 0 {
        report.stop = MovMemoryReductionStop::Reduced;
        report.reduced_fixture = Some(current);
    } else {
        report.stop = MovMemoryReductionStop::NoReduction;
    }
    Ok(report)
}

/// Independent literal byte-store oracle: the effective address is fixed RBX and the source is RAX.
pub fn mov_memory_oracle(
    fixture: &Fixture,
    input_sha256: &str,
) -> Result<MovMemoryExpectedResult, String> {
    if !is_sha256(input_sha256) {
        return Err("fixture digest must be lowercase SHA-256".into());
    }
    let offset = validate_fixture(fixture)?;
    let mut state = fixture.initial_state.clone();
    let data_index = fixture
        .memory
        .iter()
        .position(|region| region.region_id == "data")
        .ok_or("missing data region")?;
    let mut bytes = decode_hex(&fixture.memory[data_index].initial_bytes_hex)?;
    bytes[offset..offset + 8].copy_from_slice(&state.gpr[0].get().to_le_bytes());
    state.rip = fixture.stop_rip.ok_or("missing stop RIP")?;
    let memory = fixture
        .memory
        .iter()
        .enumerate()
        .map(|(index, region)| MemoryRegionSnapshot {
            region_id: region.region_id.clone(),
            guest_address: region.guest_address,
            bytes_hex: if index == data_index {
                encode_hex(&bytes)
            } else {
                region.initial_bytes_hex.clone()
            },
        })
        .collect();
    Ok(MovMemoryExpectedResult {
        case_id: fixture.case_id.clone(),
        profile: fixture.profile,
        input_sha256: input_sha256.to_string(),
        outcome: Outcome::StopAddress {},
        instructions_executed: HexU64(1),
        state,
        rflags_defined: fixture.rflags_mask,
        memory,
    })
}

fn validate_fixture(fixture: &Fixture) -> Result<usize, String> {
    fixture.validate().map_err(|error| error.to_string())?;
    if fixture.profile != Profile::LongIntegerV1 || fixture.initial_state.mode != Mode::Long64 {
        return Err("MOV memory reducer accepts only s2-long-integer-v1 in Long64 mode".into());
    }
    if fixture.expected_exit != ExpectedExit::StopAddress || fixture.instruction_budget != 1 {
        return Err("MOV memory reducer requires one instruction ending at stop_rip".into());
    }
    if fixture.memory.len() != 2 {
        return Err("MOV memory reducer requires exactly code and data regions".into());
    }
    let code = fixture
        .memory
        .iter()
        .find(|r| r.region_id == "code")
        .ok_or("missing code region")?;
    let data = fixture
        .memory
        .iter()
        .find(|r| r.region_id == "data")
        .ok_or("missing data region")?;
    if code.guest_address != fixture.initial_state.rip
        || code.length != 3
        || code.initial_bytes_hex.to_ascii_lowercase() != "488903"
        || !code.permissions.read
        || code.permissions.write
        || !code.permissions.execute
    {
        return Err("code must be one RX region containing exact bytes 48 89 03 at entry".into());
    }
    if !data.permissions.read || !data.permissions.write || data.permissions.execute {
        return Err("data must be a non-executable RW region".into());
    }
    let code_mask = decode_hex(&code.comparison_mask_hex)?;
    let data_mask = decode_hex(&data.comparison_mask_hex)?;
    if code_mask.iter().any(|byte| *byte != 0xff) || data_mask.iter().any(|byte| *byte != 0xff) {
        return Err("MOV memory reducer requires complete code/data observation masks".into());
    }
    let data_bytes = decode_hex(&data.initial_bytes_hex)?;
    if data_bytes.len() < 10 {
        return Err(
            "data region must include at least one canary byte around the qword store".into(),
        );
    }
    let start = data.guest_address.get();
    let address = fixture.initial_state.gpr[3].get();
    let offset = address
        .checked_sub(start)
        .ok_or("RBX precedes data region")? as usize;
    if offset == 0
        || offset
            .checked_add(8)
            .filter(|end| *end < data_bytes.len())
            .is_none()
    {
        return Err("RBX must address eight bytes with a canary byte before and after".into());
    }
    if fixture.stop_rip.ok_or("missing stop RIP")?.get()
        != fixture
            .initial_state
            .rip
            .get()
            .checked_add(3)
            .ok_or("stop RIP overflows")?
    {
        return Err("stop_rip must immediately follow the exact three-byte MOV".into());
    }
    Ok(offset)
}

fn mismatch_fingerprint(
    fixture: &Fixture,
    digest: &str,
    result: &RunResult,
    expected_backend: Backend,
) -> Result<Option<MovMemoryFingerprint>, String> {
    validate_callback_result(fixture, digest, result, expected_backend)?;
    if result.outcome != (Outcome::StopAddress {})
        || result.instructions_executed != (Observation::Available { value: HexU64(1) })
    {
        return Err("callback did not complete the exact one-instruction stop profile".into());
    }
    let Observation::Available { value: actual } = &result.snapshot else {
        return Err("callback has no complete snapshot".into());
    };
    let expected = mov_memory_oracle(fixture, digest)?;
    if actual.rflags_defined != expected.rflags_defined
        || actual.memory.len() != expected.memory.len()
    {
        return Err("callback changed flags-definedness or memory observation scope".into());
    }
    if actual.exception != (Observation::Available { value: None }) {
        return Err(
            "successful MOV callback did not provide an explicit no-exception observation".into(),
        );
    }
    if (actual.state.rflags.get() ^ expected.state.rflags.get()) & expected.rflags_defined.get()
        != 0
    {
        return Err("callback changed a defined RFLAGS bit".into());
    }
    let mut state = actual.state.clone();
    let expected_state = expected.state.clone();
    state.rflags = expected_state.rflags;
    let mut locations = Vec::new();
    if state != expected_state {
        for index in 0..16 {
            if actual.state.gpr[index] != expected.state.gpr[index] {
                locations.push(format!("gpr.{index}"));
            }
        }
        if actual.state.rip != expected.state.rip {
            locations.push("rip".into());
        }
        if (actual.state.rflags.get() ^ expected.state.rflags.get()) & expected.rflags_defined.get()
            != 0
        {
            locations.push("rflags".into());
        }
        return Err(format!(
            "callback architectural state drift outside the destination bytes: {locations:?}"
        ));
    }
    let data = fixture
        .memory
        .iter()
        .find(|r| r.region_id == "data")
        .ok_or("missing data region")?;
    let offset = (fixture.initial_state.gpr[3].get() - data.guest_address.get()) as usize;
    let mut destination_mismatches = Vec::new();
    for (actual_region, expected_region) in actual.memory.iter().zip(&expected.memory) {
        if actual_region.region_id != expected_region.region_id
            || actual_region.guest_address != expected_region.guest_address
        {
            return Err("callback changed memory region identity".into());
        }
        let actual_bytes = decode_hex(&actual_region.bytes_hex)?;
        let expected_bytes = decode_hex(&expected_region.bytes_hex)?;
        if actual_bytes.len() != expected_bytes.len() {
            return Err("callback changed observed memory length".into());
        }
        for (index, (observed, wanted)) in actual_bytes.iter().zip(expected_bytes).enumerate() {
            if *observed == wanted {
                continue;
            }
            if actual_region.region_id == "data" && (offset..offset + 8).contains(&index) {
                destination_mismatches.push(index - offset);
            } else {
                return Err("callback memory drift outside the exact destination qword".into());
            }
        }
    }
    if destination_mismatches.is_empty() {
        Ok(None)
    } else {
        Ok(Some(MovMemoryFingerprint {
            wrong_destination_byte_offsets: destination_mismatches,
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
            "callback result identity does not match its fixture/{expected_backend:?} backend"
        ));
    }
    result
        .validate_for(&LoadedFixture {
            fixture: fixture.clone(),
            input_sha256: digest.to_string(),
        })
        .map_err(|error| format!("invalid {expected_backend:?} result: {error}"))
}

fn same_semantic_result(a: &RunResult, b: &RunResult) -> bool {
    a == b
}
fn invoke<F: FnMut(&Fixture) -> Result<RunResult, String>>(
    execute: &mut F,
    fixture: &Fixture,
    count: &mut usize,
) -> Result<RunResult, String> {
    *count += 1;
    execute(fixture)
}
fn callback_error(
    report: &mut MovMemoryReduction,
    error: String,
) -> Result<MovMemoryReduction, String> {
    report.stop = MovMemoryReductionStop::CallbackError;
    report.stop_detail = Some(error);
    Ok(report.clone())
}
fn final_error(
    report: &mut MovMemoryReduction,
    error: String,
) -> Result<MovMemoryReduction, String> {
    report.stop = MovMemoryReductionStop::FinalConfirmationFailed;
    report.stop_detail = Some(error);
    Ok(report.clone())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if value.len() % 2 != 0 {
        return Err("hex string has odd length".into());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).map_err(|e| e.to_string())?, 16)
                .map_err(|e| e.to_string())
        })
        .collect()
}
fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 15) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        MovMemoryReductionStop, decode_hex, mov_memory_oracle, reduce_mov_memory_fixture,
        reduce_mov_memory_fixture_with_native_backend,
    };
    use crate::result::Backend;
    use crate::sha256_hex;
    use crate::v2::{Fixture, Outcome, RunResult, SCHEMA_VERSION};
    use serde_json::json;

    const FIXTURE: &[u8] =
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_42n_mov_qword_rbx_rax.case.json");
    fn fixture() -> Fixture {
        Fixture::from_json(FIXTURE).unwrap().fixture
    }

    fn result(fixture: &Fixture, digest: &str, damage: bool, extra_drift: bool) -> RunResult {
        result_for_backend(fixture, digest, damage, extra_drift, Backend::Interpreter)
    }

    fn result_for_backend(
        fixture: &Fixture,
        digest: &str,
        damage: bool,
        extra_drift: bool,
        backend: Backend,
    ) -> RunResult {
        let expected = mov_memory_oracle(fixture, digest).unwrap();
        let mut memory = expected.memory;
        if damage || extra_drift {
            let data = memory
                .iter_mut()
                .find(|region| region.region_id == "data")
                .unwrap();
            let mut bytes = decode_hex(&data.bytes_hex).unwrap();
            bytes[8] ^= 1;
            if extra_drift {
                bytes[0] ^= 1;
            }
            data.bytes_hex = super::encode_hex(&bytes);
        }
        serde_json::from_value(json!({
            "schema_version": SCHEMA_VERSION, "case_id": fixture.case_id, "input_sha256": digest,
            "profile": fixture.profile, "backend": backend, "outcome": Outcome::StopAddress {},
            "instructions_executed": {"status":"available","value":expected.instructions_executed},
            "snapshot": {"status":"available","value":{"state":expected.state,"rflags_defined":expected.rflags_defined,"memory":memory,"exception":{"status":"available","value":null}}},
            "build":{"revision":"test","target":"test","profile":"test","compiler":"test"},
            "machine":{"host_os":"test","host_arch":"test"}
        })).unwrap()
    }

    #[test]
    fn literal_oracle_writes_little_endian_rax_and_keeps_canaries() {
        let fixture = fixture();
        let expected = mov_memory_oracle(&fixture, &sha256_hex(FIXTURE)).unwrap();
        let data = expected
            .memory
            .iter()
            .find(|region| region.region_id == "data")
            .unwrap();
        let bytes = decode_hex(&data.bytes_hex).unwrap();
        assert_eq!(
            &bytes[7..17],
            &[0xa7, 0xef, 0xcd, 0xab, 0x89, 0x67, 0x45, 0x23, 0x01, 0xc0]
        );
        assert_eq!(expected.state.rip, fixture.stop_rip.unwrap());
        assert_eq!(expected.instructions_executed.0, 1);
    }

    #[test]
    fn synthetic_stable_destination_mismatch_reduces_only_rax() {
        let fixture = fixture();
        let digest = sha256_hex(FIXTURE);
        let reduction = reduce_mov_memory_fixture(&fixture, &digest, 132, |candidate| {
            let candidate_digest = if candidate == &fixture {
                digest.clone()
            } else {
                sha256_hex(&candidate.to_json().unwrap())
            };
            Ok(result(candidate, &candidate_digest, true, false))
        })
        .unwrap();
        assert_eq!(reduction.stop, MovMemoryReductionStop::Reduced);
        let reduced = reduction.reduced_fixture.unwrap();
        assert_eq!(reduced.initial_state.gpr[0].get(), 0);
        for index in 1..16 {
            assert_eq!(
                reduced.initial_state.gpr[index],
                fixture.initial_state.gpr[index]
            );
        }
        assert_eq!(reduced.memory, fixture.memory);
        assert_eq!(
            reduction
                .initial_fingerprint
                .unwrap()
                .wrong_destination_byte_offsets,
            [0]
        );
    }

    #[test]
    fn extra_memory_drift_and_callback_failure_do_not_preserve_mismatch() {
        let fixture = fixture();
        let digest = sha256_hex(FIXTURE);
        let drift = reduce_mov_memory_fixture(&fixture, &digest, 132, |candidate| {
            let candidate_digest = if candidate == &fixture {
                digest.clone()
            } else {
                sha256_hex(&candidate.to_json().unwrap())
            };
            Ok(result(candidate, &candidate_digest, false, true))
        })
        .unwrap();
        assert_eq!(drift.stop, MovMemoryReductionStop::CallbackError);
        let failure =
            reduce_mov_memory_fixture(&fixture, &digest, 132, |_| Err("synthetic timeout".into()))
                .unwrap();
        assert_eq!(failure.stop, MovMemoryReductionStop::CallbackError);
        assert_eq!(failure.actual_callbacks, 1);
    }

    #[test]
    fn budget_exhaustion_and_flaky_initial_are_rejected() {
        let fixture = fixture();
        let digest = sha256_hex(FIXTURE);
        let budget = reduce_mov_memory_fixture(&fixture, &digest, 4, |candidate| {
            let candidate_digest = if candidate == &fixture {
                digest.clone()
            } else {
                sha256_hex(&candidate.to_json().unwrap())
            };
            Ok(result(candidate, &candidate_digest, true, false))
        })
        .unwrap();
        assert_eq!(budget.stop, MovMemoryReductionStop::CallbackBudgetExhausted);
        assert!(budget.reduced_fixture.is_none());
        let mut calls = 0;
        let flaky = reduce_mov_memory_fixture(&fixture, &digest, 132, |candidate| {
            calls += 1;
            let candidate_digest = if candidate == &fixture {
                digest.clone()
            } else {
                sha256_hex(&candidate.to_json().unwrap())
            };
            Ok(result(candidate, &candidate_digest, calls == 1, false))
        })
        .unwrap();
        assert_eq!(flaky.stop, MovMemoryReductionStop::FlakyInitial);
    }

    #[test]
    fn native_adapter_requires_native_x86_callback_identity() {
        let fixture = fixture();
        let digest = sha256_hex(FIXTURE);
        let native =
            reduce_mov_memory_fixture_with_native_backend(&fixture, &digest, 132, |candidate| {
                let candidate_digest = if candidate == &fixture {
                    digest.clone()
                } else {
                    sha256_hex(&candidate.to_json().unwrap())
                };
                Ok(result_for_backend(
                    candidate,
                    &candidate_digest,
                    false,
                    false,
                    Backend::NativeX86,
                ))
            })
            .unwrap();
        assert_eq!(native.stop, MovMemoryReductionStop::NoInitialMismatch);
        assert_eq!(native.actual_callbacks, 2);

        let wrong_backend =
            reduce_mov_memory_fixture_with_native_backend(&fixture, &digest, 132, |candidate| {
                let candidate_digest = if candidate == &fixture {
                    digest.clone()
                } else {
                    sha256_hex(&candidate.to_json().unwrap())
                };
                Ok(result(candidate, &candidate_digest, false, false))
            })
            .unwrap();
        assert_eq!(wrong_backend.stop, MovMemoryReductionStop::CallbackError);
        assert!(wrong_backend.reduced_fixture.is_none());
    }

    #[test]
    fn synthetic_native_x86_mismatch_reduces_and_preserves_the_fixture_shape() {
        let fixture = fixture();
        let digest = sha256_hex(FIXTURE);
        let reduction =
            reduce_mov_memory_fixture_with_native_backend(&fixture, &digest, 132, |candidate| {
                let candidate_digest = if candidate == &fixture {
                    digest.clone()
                } else {
                    sha256_hex(&candidate.to_json().unwrap())
                };
                Ok(result_for_backend(
                    candidate,
                    &candidate_digest,
                    true,
                    false,
                    Backend::NativeX86,
                ))
            })
            .unwrap();

        assert_eq!(reduction.stop, MovMemoryReductionStop::Reduced);
        assert_eq!(reduction.original_fixture, fixture);
        assert!(reduction.candidate_proposals > 0);
        assert_eq!(reduction.candidate_runs, reduction.candidate_proposals);
        assert_eq!(
            reduction.actual_callbacks,
            2 + reduction.candidate_runs * 2 + 2
        );
        let reduced = reduction.reduced_fixture.as_ref().unwrap();
        assert!(
            reduced.initial_state.gpr[0].get().count_ones()
                < fixture.initial_state.gpr[0].get().count_ones()
        );
        assert_eq!(reduced.initial_state.gpr[0].get(), 0);
        for index in 1..16 {
            assert_eq!(
                reduced.initial_state.gpr[index],
                fixture.initial_state.gpr[index]
            );
        }
        assert_eq!(reduced.initial_state.rip, fixture.initial_state.rip);
        assert_eq!(reduced.initial_state.rflags, fixture.initial_state.rflags);
        assert_eq!(reduced.memory, fixture.memory);
        assert_eq!(reduced.stop_rip, fixture.stop_rip);
        assert_eq!(reduced.expected_exit, fixture.expected_exit);

        let expected_fingerprint = Some(crate::mov_memory_reducer::MovMemoryFingerprint {
            wrong_destination_byte_offsets: vec![0],
        });
        assert_eq!(reduction.initial_fingerprint, expected_fingerprint);
        assert_eq!(reduction.final_fingerprint, expected_fingerprint);
        assert_eq!(reduction.final_hamming_weight, Some(0));
    }

    #[test]
    fn native_adapter_fails_closed_on_wrong_backend_for_candidate_and_final_pairs() {
        let fixture = fixture();
        let digest = sha256_hex(FIXTURE);

        let mut calls = 0;
        let wrong_candidate =
            reduce_mov_memory_fixture_with_native_backend(&fixture, &digest, 132, |candidate| {
                let candidate_digest = if candidate == &fixture {
                    digest.clone()
                } else {
                    sha256_hex(&candidate.to_json().unwrap())
                };
                calls += 1;
                let backend = if calls <= 2 {
                    Backend::NativeX86
                } else {
                    Backend::Interpreter
                };
                Ok(result_for_backend(
                    candidate,
                    &candidate_digest,
                    true,
                    false,
                    backend,
                ))
            })
            .unwrap();
        assert_eq!(wrong_candidate.stop, MovMemoryReductionStop::CallbackError);
        assert_eq!(wrong_candidate.candidate_proposals, 1);
        assert!(wrong_candidate.reduced_fixture.is_none());

        let final_pair_start = 2 + fixture.initial_state.gpr[0].get().count_ones() as usize * 2;
        let mut calls = 0;
        let wrong_final =
            reduce_mov_memory_fixture_with_native_backend(&fixture, &digest, 132, |candidate| {
                let candidate_digest = if candidate == &fixture {
                    digest.clone()
                } else {
                    sha256_hex(&candidate.to_json().unwrap())
                };
                let backend = if calls < final_pair_start {
                    Backend::NativeX86
                } else {
                    Backend::Interpreter
                };
                calls += 1;
                Ok(result_for_backend(
                    candidate,
                    &candidate_digest,
                    true,
                    false,
                    backend,
                ))
            })
            .unwrap();
        assert_eq!(
            wrong_final.stop,
            MovMemoryReductionStop::FinalConfirmationFailed
        );
        assert!(wrong_final.reduced_fixture.is_none());
    }
}
