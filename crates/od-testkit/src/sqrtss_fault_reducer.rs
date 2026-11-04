//! Bounded local reducer for one exact unmasked `SQRTSS XMM0, XMM1` SNaN fault.

use crate::HexU64;
use crate::result::{Backend, ExceptionSnapshot, Observation};
use crate::sha256_hex;
use crate::v2::{
    ExpectedExit, Fixture, IntegerState, Mode, Outcome, Profile, RunResult as V2RunResult,
};
use serde::{Deserialize, Serialize};

pub const SQRTSS_FAULT_REDUCER_VERSION: &str = "od-testkit-s2-42o-sqrtss-snan-fault-v1";
pub const SQRTSS_FAULT_REDUCER_MAX_CALLBACKS: usize = 48;
pub const SQRTSS_FAULT_REDUCER_DEFAULT_BUDGET: usize = SQRTSS_FAULT_REDUCER_MAX_CALLBACKS;
const IE: u32 = 1;
const QUIET_BIT: u32 = 1 << 22;
const PAYLOAD_MASK: u32 = (1 << 22) - 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SqrtssFaultFingerprint {
    /// Changed observable bit locations, in stable order.
    pub differing_bits: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SqrtssFaultReductionStop {
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
pub struct SqrtssFaultExpectedResult {
    pub case_id: String,
    pub profile: Profile,
    pub input_sha256: String,
    pub outcome: Outcome,
    pub instructions_executed: HexU64,
    pub state: IntegerState,
    pub rflags_defined: HexU64,
    pub exception: ExceptionSnapshot,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SqrtssFaultReduction {
    pub stop: SqrtssFaultReductionStop,
    pub stop_detail: Option<String>,
    pub original_fixture: Fixture,
    pub reduced_fixture: Option<Fixture>,
    pub initial_results: Vec<V2RunResult>,
    pub final_results: Vec<V2RunResult>,
    pub expected_original: SqrtssFaultExpectedResult,
    pub expected_reduced: Option<SqrtssFaultExpectedResult>,
    pub initial_fingerprint: Option<SqrtssFaultFingerprint>,
    pub final_fingerprint: Option<SqrtssFaultFingerprint>,
    pub actual_callbacks: usize,
    pub candidate_proposals: usize,
    pub accepted_candidates: usize,
    pub initial_payload: u32,
    pub final_payload: Option<u32>,
}

/// Clear SNaN payload bits from bit 21 down to bit 0. Sign, exponent, quiet bit,
/// masks and every other fixture field stay fixed. Each proposed input is run twice.
pub fn reduce_sqrtss_fault_fixture<F>(
    original: &Fixture,
    original_input_sha256: &str,
    call_budget: usize,
    mut execute: F,
) -> Result<SqrtssFaultReduction, String>
where
    F: FnMut(&Fixture) -> Result<V2RunResult, String>,
{
    if !(4..=SQRTSS_FAULT_REDUCER_MAX_CALLBACKS).contains(&call_budget) {
        return Err(format!(
            "SQRTSS fault reducer budget must be 4..={SQRTSS_FAULT_REDUCER_MAX_CALLBACKS}"
        ));
    }
    if !is_sha256(original_input_sha256) {
        return Err("original input digest must be lowercase SHA-256".into());
    }
    validate_fixture(original)?;
    let initial_payload = snan_payload(original)?;
    let expected_original = expected_result(original, original_input_sha256)?;
    let mut report = SqrtssFaultReduction {
        stop: SqrtssFaultReductionStop::NoInitialMismatch,
        stop_detail: None,
        original_fixture: original.clone(),
        reduced_fixture: None,
        initial_results: Vec::new(),
        final_results: Vec::new(),
        expected_original,
        expected_reduced: None,
        initial_fingerprint: None,
        final_fingerprint: None,
        actual_callbacks: 0,
        candidate_proposals: 0,
        accepted_candidates: 0,
        initial_payload,
        final_payload: None,
    };

    let (first, second) = match invoke_pair(&mut execute, original, &mut report.actual_callbacks) {
        Ok(pair) => pair,
        Err((first, error)) => {
            report.initial_results.extend(first);
            return callback_error(report, error);
        }
    };
    report
        .initial_results
        .extend([first.clone(), second.clone()]);
    if !same_semantic_result(&first, &second) {
        report.stop = SqrtssFaultReductionStop::FlakyInitial;
        report.stop_detail = Some("initial callback results differ semantically".into());
        return Ok(report);
    }
    if !same_execution_provenance(&first, &second) {
        report.stop = SqrtssFaultReductionStop::FlakyInitial;
        report.stop_detail = Some("initial build or machine provenance differs".into());
        return Ok(report);
    }
    let fp1 = match mismatch_fingerprint(original, original_input_sha256, &first) {
        Ok(fp) => fp,
        Err(error) => return callback_error(report, error),
    };
    let fp2 = match mismatch_fingerprint(original, original_input_sha256, &second) {
        Ok(fp) => fp,
        Err(error) => return callback_error(report, error),
    };
    if fp1 != fp2 {
        report.stop = SqrtssFaultReductionStop::FlakyInitial;
        report.stop_detail = Some("initial mismatch fingerprints differ".into());
        return Ok(report);
    }
    let Some(initial_fp) = fp1 else {
        return Ok(report);
    };
    report.initial_fingerprint = Some(initial_fp.clone());

    let mut current = original.clone();
    let mut budget_exhausted = false;
    for bit in (0..22).rev() {
        let payload = snan_payload(&current)?;
        if payload & (1 << bit) == 0 {
            continue;
        }
        if payload == (1 << bit) {
            continue;
        }
        if report.actual_callbacks.saturating_add(4) > call_budget {
            budget_exhausted = true;
            break;
        }
        let mut candidate = current.clone();
        set_snan_payload(&mut candidate, payload & !(1 << bit))?;
        candidate
            .validate()
            .map_err(|error| format!("generated candidate is invalid: {error}"))?;
        report.candidate_proposals += 1;
        let digest = fixture_digest(&candidate)?;
        let (a, b) = match invoke_pair(&mut execute, &candidate, &mut report.actual_callbacks) {
            Ok(pair) => pair,
            Err((_, error)) => return callback_error(report, error),
        };
        if !same_semantic_result(&a, &b) {
            report.stop = SqrtssFaultReductionStop::FlakyCandidate;
            report.stop_detail = Some("candidate callback results differ semantically".into());
            return Ok(report);
        }
        if !same_execution_provenance(&first, &a) || !same_execution_provenance(&first, &b) {
            return callback_error(
                report,
                "candidate build or machine provenance changed".into(),
            );
        }
        let a_fp = match mismatch_fingerprint(&candidate, &digest, &a) {
            Ok(fp) => fp,
            Err(e) => return callback_error(report, e),
        };
        let b_fp = match mismatch_fingerprint(&candidate, &digest, &b) {
            Ok(fp) => fp,
            Err(e) => return callback_error(report, e),
        };
        if a_fp != b_fp {
            report.stop = SqrtssFaultReductionStop::FlakyCandidate;
            report.stop_detail = Some("candidate mismatch fingerprints differ".into());
            return Ok(report);
        }
        if a_fp.as_ref() == Some(&initial_fp) {
            current = candidate;
            report.accepted_candidates += 1;
        }
    }
    if report.actual_callbacks.saturating_add(2) > call_budget {
        report.stop = SqrtssFaultReductionStop::CallbackBudgetExhausted;
        report.stop_detail = Some("insufficient callbacks for final confirmation".into());
        return Ok(report);
    }
    let final_digest = if current == *original {
        original_input_sha256.to_string()
    } else {
        fixture_digest(&current)?
    };
    let (a, b) = match invoke_pair(&mut execute, &current, &mut report.actual_callbacks) {
        Ok(pair) => pair,
        Err((first, error)) => {
            report.final_results.extend(first);
            report.stop = SqrtssFaultReductionStop::FinalConfirmationFailed;
            report.stop_detail = Some(error);
            return Ok(report);
        }
    };
    report.final_results.extend([a.clone(), b.clone()]);
    if !same_semantic_result(&a, &b) {
        report.stop = SqrtssFaultReductionStop::FinalConfirmationFailed;
        report.stop_detail = Some("final callback results differ semantically".into());
        return Ok(report);
    }
    if !same_execution_provenance(&first, &a) || !same_execution_provenance(&first, &b) {
        report.stop = SqrtssFaultReductionStop::FinalConfirmationFailed;
        report.stop_detail = Some("final build or machine provenance changed".into());
        return Ok(report);
    }
    let a_fp = match mismatch_fingerprint(&current, &final_digest, &a) {
        Ok(fp) => fp,
        Err(e) => {
            report.stop = SqrtssFaultReductionStop::FinalConfirmationFailed;
            report.stop_detail = Some(e);
            return Ok(report);
        }
    };
    let b_fp = match mismatch_fingerprint(&current, &final_digest, &b) {
        Ok(fp) => fp,
        Err(e) => {
            report.stop = SqrtssFaultReductionStop::FinalConfirmationFailed;
            report.stop_detail = Some(e);
            return Ok(report);
        }
    };
    if a_fp.as_ref() != Some(&initial_fp) || b_fp.as_ref() != Some(&initial_fp) {
        report.stop = SqrtssFaultReductionStop::FinalConfirmationFailed;
        report.stop_detail =
            Some("final confirmation did not preserve the initial mismatch fingerprint".into());
        return Ok(report);
    }
    report.final_fingerprint = Some(initial_fp);
    report.final_payload = Some(snan_payload(&current)?);
    report.expected_reduced = Some(expected_result(&current, &final_digest)?);
    if budget_exhausted {
        report.stop = SqrtssFaultReductionStop::CallbackBudgetExhausted;
        report.stop_detail = Some(
            "candidate schedule exceeded the callback budget; no reduced fixture is published"
                .into(),
        );
        return Ok(report);
    }
    if current == *original {
        report.stop = SqrtssFaultReductionStop::NoReduction;
    } else {
        report.stop = SqrtssFaultReductionStop::Reduced;
        report.reduced_fixture = Some(current);
    }
    Ok(report)
}

pub fn sqrtss_fault_oracle(
    fixture: &Fixture,
    digest: &str,
) -> Result<SqrtssFaultExpectedResult, String> {
    validate_fixture(fixture)?;
    let mut state = fixture.initial_state.clone();
    let sse = state.sse.as_mut().ok_or("missing SSE state")?;
    sse.mxcsr = HexU64(sse.mxcsr.get() | u64::from(IE));
    Ok(SqrtssFaultExpectedResult {
        case_id: fixture.case_id.clone(),
        profile: fixture.profile,
        input_sha256: digest.into(),
        outcome: Outcome::Exception {},
        instructions_executed: HexU64(0),
        state,
        rflags_defined: HexU64(0x0cd7),
        exception: ExceptionSnapshot {
            vector: 19,
            error_code: Observation::Unavailable {
                reason: "exception has no architectural error code".into(),
            },
            faulting_rip: fixture.initial_state.rip,
            fault_address: Observation::Unavailable {
                reason: "exception has no reported linear address".into(),
            },
        },
    })
}

fn expected_result(fixture: &Fixture, digest: &str) -> Result<SqrtssFaultExpectedResult, String> {
    sqrtss_fault_oracle(fixture, digest)
}
fn validate_fixture(f: &Fixture) -> Result<(), String> {
    f.validate()
        .map_err(|e| format!("invalid schema-v2 fixture: {e}"))?;
    if f.profile != Profile::LongSqrtssRegistersV1
        || f.initial_state.mode != Mode::Long64
        || f.expected_exit != ExpectedExit::Exception
        || f.instruction_budget != 1
    {
        return Err(
            "fixture is outside the exact Long64 SQRTSS unmasked register-fault profile".into(),
        );
    }
    let code = f
        .memory
        .iter()
        .find(|r| r.region_id == "code")
        .ok_or("missing code mapping")?;
    if f.memory.len() != 1
        || code.guest_address != f.initial_state.rip
        || code.length != 4
        || code.initial_bytes_hex != "f30f51c1"
        || !code.permissions.read
        || code.permissions.write
        || !code.permissions.execute
        || code.comparison_mask_hex != "ffffffff"
        || f.stop_rip.map(HexU64::get) != f.initial_state.rip.get().checked_add(4)
    {
        return Err(
            "fixture must preserve the exact single RX F3 0F 51 C1 mapping, mask and stop".into(),
        );
    }
    let controls = f.sqrtss_controls.ok_or("missing SQRTSS controls")?;
    if !controls.sse_available
        || controls.cr0_em
        || controls.cr0_ts
        || !controls.cr4_osfxsr
        || !controls.cr4_osxmmexcpt
    {
        return Err("SQRTSS controls do not enable the required SSE fault path".into());
    }
    let sse = f.initial_state.sse.as_ref().ok_or("missing SSE state")?;
    let mxcsr = u32::try_from(sse.mxcsr.get()).map_err(|_| "MXCSR exceeds 32 bits")?;
    if mxcsr & !0xffbf != 0
        || mxcsr & 0x8040 != 0
        || mxcsr & (1 << 7) != 0
        || mxcsr & (0x3e << 7) != (0x3e << 7)
        || mxcsr & (1 << 6) != 0
        || mxcsr & (1 << 15) != 0
    {
        return Err(
            "MXCSR must use supported bits, DAZ/FTZ=0, IM=0 and all remaining exception masks=1"
                .into(),
        );
    }
    let _ = snan_payload(f)?;
    Ok(())
}
fn snan_payload(f: &Fixture) -> Result<u32, String> {
    let sse = f.initial_state.sse.as_ref().ok_or("missing SSE state")?;
    let bytes = decode_hex(&sse.xmm[1])?;
    let bits = u32::from_le_bytes(
        bytes[..4]
            .try_into()
            .map_err(|_| "XMM1 low dword missing")?,
    );
    if bits & 0x7f80_0000 != 0x7f80_0000 || bits & QUIET_BIT != 0 || bits & PAYLOAD_MASK == 0 {
        return Err("XMM1.low32 must be a nonzero-payload binary32 SNaN".into());
    }
    Ok(bits & PAYLOAD_MASK)
}
fn set_snan_payload(f: &mut Fixture, payload: u32) -> Result<(), String> {
    if payload == 0 || payload > PAYLOAD_MASK {
        return Err("candidate would cease to be a nonzero-payload SNaN".into());
    }
    let sse = f.initial_state.sse.as_mut().ok_or("missing SSE state")?;
    let mut bytes = decode_hex(&sse.xmm[1])?;
    let old = u32::from_le_bytes(
        bytes[..4]
            .try_into()
            .map_err(|_| "XMM1 low dword missing")?,
    );
    let next = (old & !PAYLOAD_MASK) | payload;
    bytes[..4].copy_from_slice(&next.to_le_bytes());
    sse.xmm[1] = encode_hex(&bytes);
    Ok(())
}
fn mismatch_fingerprint(
    f: &Fixture,
    digest: &str,
    r: &V2RunResult,
) -> Result<Option<SqrtssFaultFingerprint>, String> {
    validate_callback_result(f, digest, r)?;
    if r.outcome != (Outcome::Exception {})
        || r.instructions_executed != (Observation::Available { value: HexU64(0) })
    {
        return Err("callback did not report the expected zero-retirement exception".into());
    }
    let Observation::Available { value: actual } = &r.snapshot else {
        return Err("callback has no complete snapshot".into());
    };
    let Observation::Available {
        value: Some(exception),
    } = &actual.exception
    else {
        return Err("callback has no complete exception metadata".into());
    };
    if exception.vector != 19
        || exception.faulting_rip != f.initial_state.rip
        || exception.error_code
            != (Observation::Unavailable {
                reason: "exception has no architectural error code".into(),
            })
        || exception.fault_address
            != (Observation::Unavailable {
                reason: "exception has no reported linear address".into(),
            })
    {
        return Err("callback reported wrong #XM metadata".into());
    }
    let expected = sqrtss_fault_oracle(f, digest)?;
    if actual.rflags_defined != expected.rflags_defined
        || actual.memory.len() != 1
        || actual.exception
            != (Observation::Available {
                value: Some(exception.clone()),
            })
    {
        return Err("callback changed definedness, memory scope, or fault metadata".into());
    }
    let code = &f.memory[0];
    let observed = &actual.memory[0];
    if observed.region_id != "code"
        || observed.guest_address != code.guest_address
        || observed.bytes_hex != code.initial_bytes_hex
    {
        return Err("callback changed code mapping or bytes".into());
    }
    let controls = f.sqrtss_controls;
    if actual.sqrtss_controls != controls {
        return Err("callback changed or omitted SQRTSS controls".into());
    }
    let actual_sse = actual
        .state
        .sse
        .as_ref()
        .ok_or("callback omitted SSE snapshot")?;
    let expected_sse = expected
        .state
        .sse
        .as_ref()
        .ok_or("oracle missing SSE state")?
        .clone();
    // Expected XMM1 is unchanged from the candidate; only XMM0.low32 and MXCSR.IE may differ.
    let mut differing_bits = Vec::new();
    let actual_xmm0 = decode_hex(&actual_sse.xmm[0])?;
    let expected_xmm0 = decode_hex(&expected_sse.xmm[0])?;
    for bit in 0..32 {
        if ((actual_xmm0[bit / 8] ^ expected_xmm0[bit / 8]) >> (bit % 8)) & 1 != 0 {
            differing_bits.push(format!("xmm0.low32.bit{bit}"));
        }
    }
    let expected_xmm1 = f.initial_state.sse.as_ref().unwrap().xmm[1].clone();
    if actual_sse.xmm[1] != expected_xmm1 {
        return Err("callback changed XMM1 source state".into());
    }
    for i in 1..16 {
        if i != 1 && actual_sse.xmm[i] != expected_sse.xmm[i] {
            return Err(format!("callback changed XMM{i}"));
        }
    }
    let actual_mxcsr =
        u32::try_from(actual_sse.mxcsr.get()).map_err(|_| "callback MXCSR exceeds 32 bits")?;
    let expected_mxcsr =
        u32::try_from(expected_sse.mxcsr.get()).map_err(|_| "oracle MXCSR exceeds 32 bits")?;
    if (actual_mxcsr ^ expected_mxcsr) & !IE != 0 {
        return Err("callback changed MXCSR outside IE".into());
    }
    if (actual_mxcsr ^ expected_mxcsr) & IE != 0 {
        differing_bits.push("mxcsr.ie".into());
    }
    let mut normalized = actual.state.clone();
    let normalized_sse = normalized
        .sse
        .as_mut()
        .ok_or("callback omitted SSE snapshot")?;
    normalized_sse.xmm[0].replace_range(..8, &expected_sse.xmm[0][..8]);
    normalized_sse.mxcsr = expected_sse.mxcsr;
    if normalized != expected.state {
        return Err("callback changed architectural state outside XMM0.low32 or MXCSR.IE".into());
    }
    if differing_bits.is_empty() {
        Ok(None)
    } else {
        Ok(Some(SqrtssFaultFingerprint { differing_bits }))
    }
}
fn validate_callback_result(f: &Fixture, digest: &str, r: &V2RunResult) -> Result<(), String> {
    r.validate()
        .map_err(|e| format!("malformed callback result: {e}"))?;
    if r.schema_version != 2
        || r.case_id != f.case_id
        || r.input_sha256 != digest
        || r.profile != f.profile
        || r.backend != Backend::Interpreter
    {
        return Err("callback identity/backend does not match the fixture and interpreter".into());
    }
    // Verify provenance through strict input binding, while deliberately avoiding validate_for:
    // its exact expected-state comparison rejects the mismatch this reducer measures.
    if !is_sha256(digest) || f.provenance.source.is_empty() || f.provenance.specification.is_empty()
    {
        return Err("fixture provenance or digest is incomplete".into());
    }
    Ok(())
}
fn same_semantic_result(a: &V2RunResult, b: &V2RunResult) -> bool {
    a == b
}
fn same_execution_provenance(a: &V2RunResult, b: &V2RunResult) -> bool {
    a.profile == b.profile && a.backend == b.backend && a.build == b.build && a.machine == b.machine
}
fn invoke_pair<F: FnMut(&Fixture) -> Result<V2RunResult, String>>(
    execute: &mut F,
    fixture: &Fixture,
    calls: &mut usize,
) -> Result<(V2RunResult, V2RunResult), (Vec<V2RunResult>, String)> {
    *calls += 1;
    let a = execute(fixture).map_err(|e| (Vec::new(), e))?;
    *calls += 1;
    let b = execute(fixture).map_err(|e| (vec![a.clone()], e))?;
    Ok((a, b))
}
fn callback_error(mut r: SqrtssFaultReduction, e: String) -> Result<SqrtssFaultReduction, String> {
    r.stop = SqrtssFaultReductionStop::CallbackError;
    r.stop_detail = Some(e);
    Ok(r)
}
fn fixture_digest(f: &Fixture) -> Result<String, String> {
    Ok(sha256_hex(&f.to_json().map_err(|e| e.to_string())?))
}
fn is_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("hex string has odd length".into());
    }
    s.as_bytes()
        .chunks_exact(2)
        .map(|p| {
            u8::from_str_radix(std::str::from_utf8(p).map_err(|e| e.to_string())?, 16)
                .map_err(|e| e.to_string())
        })
        .collect()
}
fn encode_hex(b: &[u8]) -> String {
    b.iter().map(|v| format!("{v:02x}")).collect()
}
