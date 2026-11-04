//! Bounded AL-only reducer for successful Long64 REP STOSB fixtures.

use crate::HexU64;
use crate::result::{Backend, MemoryRegionSnapshot, Observation};
use crate::sha256_hex;
use crate::v2::{
    ExpectedExit, Fixture, IntegerState, LoadedFixture, Mode, Outcome, Profile, RunResult,
};
use serde::{Deserialize, Serialize};

pub const REP_STOSB_REDUCER_VERSION: &str = "od-testkit-s2-42ai-rep-stosb-success-v1";
pub const REP_STOSB_REDUCER_DEFAULT_BUDGET: usize = 20;
pub const REP_STOSB_REDUCER_MAX_BUDGET: usize = 20;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepStosbFingerprint {
    /// Byte offsets in the data region whose values differ from the independent fill oracle.
    pub wrong_destination_byte_offsets: Vec<usize>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepStosbReductionStop {
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
pub struct RepStosbExpectedResult {
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
pub struct RepStosbReduction {
    pub stop: RepStosbReductionStop,
    pub stop_detail: Option<String>,
    pub original_fixture: Fixture,
    pub reduced_fixture: Option<Fixture>,
    pub initial_results: Vec<RunResult>,
    pub final_results: Vec<RunResult>,
    pub expected_original: RepStosbExpectedResult,
    pub expected_reduced: Option<RepStosbExpectedResult>,
    pub initial_fingerprint: Option<RepStosbFingerprint>,
    pub final_fingerprint: Option<RepStosbFingerprint>,
    pub actual_callbacks: usize,
    pub candidate_proposals: usize,
    pub candidate_runs: usize,
    pub accepted_candidates: usize,
    pub initial_hamming_weight: u32,
    pub final_hamming_weight: Option<u32>,
}

/// Interpreter callback entry point. The callback owns execution and cannot be changed implicitly.
pub fn reduce_rep_stosb_fixture<F>(
    fixture: &Fixture,
    input_sha256: &str,
    call_budget: usize,
    execute: F,
) -> Result<RepStosbReduction, String>
where
    F: FnMut(&Fixture) -> Result<RunResult, String>,
{
    reduce_with_backend(
        fixture,
        input_sha256,
        call_budget,
        Backend::Interpreter,
        execute,
    )
}

/// Native-x86 adapter entry point. Every callback result must identify NativeX86.
pub fn reduce_rep_stosb_fixture_native<F>(
    fixture: &Fixture,
    input_sha256: &str,
    call_budget: usize,
    execute: F,
) -> Result<RepStosbReduction, String>
where
    F: FnMut(&Fixture) -> Result<RunResult, String>,
{
    reduce_with_backend(
        fixture,
        input_sha256,
        call_budget,
        Backend::NativeX86,
        execute,
    )
}

fn reduce_with_backend<F>(
    original: &Fixture,
    original_digest: &str,
    budget: usize,
    backend: Backend,
    mut execute: F,
) -> Result<RepStosbReduction, String>
where
    F: FnMut(&Fixture) -> Result<RunResult, String>,
{
    if !(4..=REP_STOSB_REDUCER_MAX_BUDGET).contains(&budget) {
        return Err(format!(
            "REP STOSB reducer callback budget must be 4..={REP_STOSB_REDUCER_MAX_BUDGET}"
        ));
    }
    if !is_sha256(original_digest) {
        return Err("original input digest must be lowercase SHA-256".into());
    }
    validate_fixture(original)?;
    let expected_original = rep_stosb_oracle(original, original_digest)?;
    let mut report = RepStosbReduction {
        stop: RepStosbReductionStop::NoInitialMismatch,
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
        initial_hamming_weight: (original.initial_state.gpr[0].get() as u8).count_ones(),
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
        report.stop = RepStosbReductionStop::FlakyInitial;
        report.stop_detail = Some("initial callback results differ semantically".into());
        return Ok(report);
    }
    let first_fp = match mismatch_fingerprint(original, original_digest, backend, &first) {
        Ok(value) => value,
        Err(error) => return callback_error(&mut report, error),
    };
    let second_fp = match mismatch_fingerprint(original, original_digest, backend, &second) {
        Ok(value) => value,
        Err(error) => return callback_error(&mut report, error),
    };
    if first_fp != second_fp {
        report.stop = RepStosbReductionStop::FlakyInitial;
        report.stop_detail = Some("initial mismatch fingerprints differ".into());
        return Ok(report);
    }
    let Some(initial_fp) = first_fp else {
        return Ok(report);
    };
    report.initial_fingerprint = Some(initial_fp.clone());

    let mut current = original.clone();
    let mut budget_exhausted = false;
    for bit in (0..8).rev() {
        if current.initial_state.gpr[0].get() & (1 << bit) == 0 {
            continue;
        }
        if report.actual_callbacks.saturating_add(4) > budget {
            budget_exhausted = true;
            break;
        }
        let mut candidate = current.clone();
        candidate.initial_state.gpr[0].0 &= !(1u64 << bit);
        candidate
            .validate()
            .map_err(|e| format!("generated REP STOSB candidate is invalid: {e}"))?;
        report.candidate_proposals += 1;
        report.candidate_runs += 1;
        let digest = sha256_hex(&candidate.to_json().map_err(|e| e.to_string())?);
        let a = match invoke(&mut execute, &candidate, &mut report.actual_callbacks) {
            Ok(value) => value,
            Err(error) => return callback_error(&mut report, error),
        };
        let b = match invoke(&mut execute, &candidate, &mut report.actual_callbacks) {
            Ok(value) => value,
            Err(error) => return callback_error(&mut report, error),
        };
        if !same_semantic_result(&a, &b) {
            report.stop = RepStosbReductionStop::FlakyCandidate;
            report.stop_detail = Some("candidate callback results differ semantically".into());
            return Ok(report);
        }
        let af = match mismatch_fingerprint(&candidate, &digest, backend, &a) {
            Ok(value) => value,
            Err(error) => return callback_error(&mut report, error),
        };
        let bf = match mismatch_fingerprint(&candidate, &digest, backend, &b) {
            Ok(value) => value,
            Err(error) => return callback_error(&mut report, error),
        };
        if af != bf {
            report.stop = RepStosbReductionStop::FlakyCandidate;
            report.stop_detail = Some("candidate mismatch fingerprints differ".into());
            return Ok(report);
        }
        if af.as_ref() == Some(&initial_fp) {
            current = candidate;
            report.accepted_candidates += 1;
        }
    }
    if report.actual_callbacks.saturating_add(2) > budget {
        report.stop = RepStosbReductionStop::CallbackBudgetExhausted;
        report.stop_detail = Some("insufficient callback budget for final confirmation".into());
        return Ok(report);
    }
    let final_digest = if current == *original {
        original_digest.to_string()
    } else {
        sha256_hex(&current.to_json().map_err(|e| e.to_string())?)
    };
    let a = match invoke(&mut execute, &current, &mut report.actual_callbacks) {
        Ok(value) => value,
        Err(error) => return callback_error(&mut report, error),
    };
    let b = match invoke(&mut execute, &current, &mut report.actual_callbacks) {
        Ok(value) => value,
        Err(error) => {
            report.final_results.push(a);
            return callback_error(&mut report, error);
        }
    };
    report.final_results.extend([a.clone(), b.clone()]);
    if !same_semantic_result(&a, &b) {
        report.stop = RepStosbReductionStop::FinalConfirmationFailed;
        report.stop_detail = Some("final callback results differ semantically".into());
        return Ok(report);
    }
    let af = match mismatch_fingerprint(&current, &final_digest, backend, &a) {
        Ok(value) => value,
        Err(error) => return callback_error(&mut report, error),
    };
    let bf = match mismatch_fingerprint(&current, &final_digest, backend, &b) {
        Ok(value) => value,
        Err(error) => return callback_error(&mut report, error),
    };
    if af.as_ref() != Some(&initial_fp) || bf.as_ref() != Some(&initial_fp) {
        report.stop = RepStosbReductionStop::FinalConfirmationFailed;
        report.stop_detail =
            Some("final results do not preserve the initial destination-byte mismatch".into());
        return Ok(report);
    }
    report.stop = if budget_exhausted {
        RepStosbReductionStop::CallbackBudgetExhausted
    } else if current == *original {
        RepStosbReductionStop::NoReduction
    } else {
        RepStosbReductionStop::Reduced
    };
    report.stop_detail = budget_exhausted
        .then(|| "candidate budget exhausted; current mismatch was finally confirmed".into());
    report.final_fingerprint = Some(initial_fp);
    report.final_hamming_weight = Some((current.initial_state.gpr[0].get() as u8).count_ones());
    report.expected_reduced = Some(rep_stosb_oracle(&current, &final_digest)?);
    report.reduced_fixture = Some(current);
    Ok(report)
}

/// Independent byte-fill oracle for the exact successful F3 AA profile.
pub fn rep_stosb_oracle(fixture: &Fixture, digest: &str) -> Result<RepStosbExpectedResult, String> {
    if !is_sha256(digest) {
        return Err("fixture digest must be lowercase SHA-256".into());
    }
    validate_fixture(fixture)?;
    let mut state = fixture.initial_state.clone();
    let count = state.gpr[1].get() as usize;
    let df = state.rflags.get() & (1 << 10) != 0;
    let start = fixture.initial_state.gpr[7].get();
    let data = fixture
        .memory
        .iter()
        .find(|r| r.region_id == "data")
        .unwrap();
    let mut memory = fixture
        .memory
        .iter()
        .map(|region| {
            let bytes = decode_hex(&region.initial_bytes_hex)?;
            Ok(MemoryRegionSnapshot {
                region_id: region.region_id.clone(),
                guest_address: region.guest_address,
                bytes_hex: encode_hex(&bytes),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let data_snapshot = memory.iter_mut().find(|r| r.region_id == "data").unwrap();
    let mut bytes = decode_hex(&data_snapshot.bytes_hex)?;
    for n in 0..count {
        let addr = if df {
            start - n as u64
        } else {
            start + n as u64
        };
        bytes[(addr - data.guest_address.get()) as usize] =
            fixture.initial_state.gpr[0].get() as u8;
    }
    data_snapshot.bytes_hex = encode_hex(&bytes);
    state.gpr[1] = HexU64(0);
    state.gpr[7] = HexU64(if df {
        start.wrapping_sub(count as u64)
    } else {
        start.wrapping_add(count as u64)
    });
    state.rip = HexU64(fixture.initial_state.rip.get() + 2);
    Ok(RepStosbExpectedResult {
        case_id: fixture.case_id.clone(),
        profile: fixture.profile,
        input_sha256: digest.into(),
        outcome: Outcome::StopAddress {},
        instructions_executed: HexU64(1),
        state,
        rflags_defined: fixture.rflags_mask,
        memory,
    })
}

fn validate_fixture(f: &Fixture) -> Result<(), String> {
    f.validate().map_err(|e| e.to_string())?;
    if f.profile != Profile::LongIntegerV1 || f.initial_state.mode != Mode::Long64 {
        return Err("REP STOSB reducer accepts only Long64 s2-long-integer-v1 fixtures".into());
    }
    if f.expected_exit != ExpectedExit::StopAddress || f.instruction_budget != 1 {
        return Err(
            "REP STOSB reducer requires a successful stop-address fixture and budget 1".into(),
        );
    }
    let entry = f.initial_state.rip.get();
    if f.stop_rip.map(HexU64::get) != entry.checked_add(2) {
        return Err("REP STOSB reducer requires stop_rip=entry+2".into());
    }
    if f.memory.len() != 2 {
        return Err("REP STOSB reducer requires exactly code and data regions".into());
    }
    let code = f
        .memory
        .iter()
        .find(|r| r.region_id == "code")
        .ok_or("missing code region")?;
    let data = f
        .memory
        .iter()
        .find(|r| r.region_id == "data")
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
    let n = f.initial_state.gpr[1].get();
    if !(1..=8).contains(&n) {
        return Err("REP STOSB reducer requires count 1..=8".into());
    }
    if data.length == 0
        || data.length > 64
        || data.permissions
            != (crate::case::MemoryPermissions {
                read: true,
                write: true,
                execute: false,
            })
        || decode_hex(&data.comparison_mask_hex)? != vec![0xff; data.length as usize]
    {
        return Err("data must be a fully observed RW region of 1..=64 bytes".into());
    }
    let start = f.initial_state.gpr[7].get();
    let end = data
        .guest_address
        .get()
        .checked_add(data.length as u64)
        .ok_or("data region overflows")?;
    if start < data.guest_address.get() || start >= end {
        return Err("initial RDI must address a byte in data".into());
    }
    let df = f.initial_state.rflags.get() & (1 << 10) != 0;
    let last = if df {
        start.checked_sub(n - 1)
    } else {
        start.checked_add(n - 1)
    }
    .ok_or("destination span overflows")?;
    if last < data.guest_address.get() || last >= end {
        return Err("all REP destination bytes must be mapped".into());
    }
    Ok(())
}

fn mismatch_fingerprint(
    f: &Fixture,
    digest: &str,
    backend: Backend,
    result: &RunResult,
) -> Result<Option<RepStosbFingerprint>, String> {
    if result.profile != Profile::LongIntegerV1
        || result.backend != backend
        || result.case_id != f.case_id
        || result.input_sha256 != digest
    {
        return Err(
            "callback result identity does not match requested REP STOSB fixture/backend".into(),
        );
    }
    result
        .validate_for(&LoadedFixture {
            fixture: f.clone(),
            input_sha256: digest.into(),
        })
        .map_err(|e| format!("invalid callback result: {e}"))?;
    let expected = rep_stosb_oracle(f, digest)?;
    let count = f.initial_state.gpr[1].get() as usize;
    let start = f.initial_state.gpr[7].get();
    let df = f.initial_state.rflags.get() & (1 << 10) != 0;
    let data_base = f
        .memory
        .iter()
        .find(|r| r.region_id == "data")
        .unwrap()
        .guest_address
        .get();
    let written = (0..count)
        .map(|n| {
            let address = if df {
                start - n as u64
            } else {
                start + n as u64
            };
            (address - data_base) as usize
        })
        .collect::<std::collections::BTreeSet<_>>();
    if result.outcome != expected.outcome
        || result.instructions_executed
            != (Observation::Available {
                value: expected.instructions_executed,
            })
    {
        return Err("callback did not complete at stop_rip with exactly one retirement".into());
    }
    let Observation::Available { value: actual } = &result.snapshot else {
        return Err("callback omitted full architectural snapshot".into());
    };
    if actual.state != expected.state
        || actual.rflags_defined != expected.rflags_defined
        || actual.memory.len() != expected.memory.len()
    {
        return Err(
            "callback state, flags-defined mask, or memory layout differs from oracle".into(),
        );
    }
    let mut wrong = Vec::new();
    for (a, e) in actual.memory.iter().zip(&expected.memory) {
        if a.region_id != e.region_id || a.guest_address != e.guest_address {
            return Err("callback memory layout changed".into());
        }
        let ab = decode_hex(&a.bytes_hex)?;
        let eb = decode_hex(&e.bytes_hex)?;
        if ab.len() != eb.len() {
            return Err("callback memory length changed".into());
        }
        if a.region_id == "data" {
            for (i, (x, y)) in ab.iter().zip(eb).enumerate() {
                if *x != y && written.contains(&i) {
                    wrong.push(i);
                } else if *x != y {
                    return Err("callback changed an untouched data byte/canary".into());
                }
            }
        } else if ab != eb {
            return Err("callback changed code bytes".into());
        }
    }
    Ok((!wrong.is_empty()).then_some(RepStosbFingerprint {
        wrong_destination_byte_offsets: wrong,
    }))
}

fn invoke<F: FnMut(&Fixture) -> Result<RunResult, String>>(
    f: &mut F,
    fixture: &Fixture,
    count: &mut usize,
) -> Result<RunResult, String> {
    *count += 1;
    f(fixture)
}
fn callback_error(r: &mut RepStosbReduction, e: String) -> Result<RepStosbReduction, String> {
    r.stop = RepStosbReductionStop::CallbackError;
    r.stop_detail = Some(e);
    Ok(r.clone())
}
fn same_semantic_result(a: &RunResult, b: &RunResult) -> bool {
    a.schema_version == b.schema_version
        && a.case_id == b.case_id
        && a.input_sha256 == b.input_sha256
        && a.profile == b.profile
        && a.backend == b.backend
        && a.outcome == b.outcome
        && a.instructions_executed == b.instructions_executed
        && a.snapshot == b.snapshot
}
fn is_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("hex byte string has odd length".into());
    };
    s.as_bytes()
        .chunks_exact(2)
        .map(|p| {
            u8::from_str_radix(
                std::str::from_utf8(p).map_err(|_| "invalid hex encoding")?,
                16,
            )
            .map_err(|_| "invalid hex encoding".into())
        })
        .collect()
}
fn encode_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
