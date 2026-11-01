//! Deterministic generated inputs for the bounded native XORPD profile.

use crate::HexU64;
use crate::case::{MemoryPermissions, MemoryRegionSpec, Provenance};
use crate::sha256_hex;
use crate::v2::{
    ExpectedExit, Fixture, IntegerState, Mode, Outcome, PROFILE_NATIVE_XORPD_XMM0_XMM1, Profile,
    RunResult, SCHEMA_VERSION, SseSnapshot,
};
use serde::{Deserialize, Serialize};

pub const NATIVE_XORPD_GENERATOR_VERSION: &str = "od-testkit-native-xorpd-v1";
pub const NATIVE_XORPD_CASES_PER_SEED: u32 = 256;
pub const NATIVE_XORPD_FIXED_SEEDS: [u64; 4] = [
    0x243f_6a88_85a3_08d3,
    0x1319_8a2e_0370_7344,
    0xa409_3822_299f_31d0,
    0x082e_fa98_ec4e_6c89,
];
pub const NATIVE_XORPD_MANIFEST_VERSION: u32 = 1;
pub const NATIVE_XORPD_REDUCER_DEFAULT_BUDGET: usize = 5120;
pub const NATIVE_XORPD_REDUCER_MAX_BUDGET: usize = 5120;

/// A mismatch fingerprint for this reducer is the exact set of XMM byte positions
/// that differ from the independent bytewise-XOR oracle.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeXorpdMismatchFingerprint {
    pub mismatching_xmm_bytes: Vec<[u8; 2]>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeXorpdReductionStop {
    Reduced,
    NoReduction,
    NoInitialMismatch,
    FlakyInitial,
    FlakyCandidate,
    CandidateBudgetExhausted,
    CallbackError,
    FinalConfirmationFailed,
}

/// The bounded result keeps all callback evidence needed by the runner to write
/// a reproducible local report. `reduced_fixture` is absent unless the initial
/// mismatch reproduced and both final confirmations succeeded.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeXorpdReduction {
    pub stop: NativeXorpdReductionStop,
    pub stop_detail: Option<String>,
    pub original_fixture: Fixture,
    pub reduced_fixture: Option<Fixture>,
    pub initial_results: Vec<RunResult>,
    pub final_results: Vec<RunResult>,
    pub initial_fingerprint: Option<NativeXorpdMismatchFingerprint>,
    pub final_fingerprint: Option<NativeXorpdMismatchFingerprint>,
    pub actual_callbacks: usize,
    pub candidate_proposals: usize,
    pub candidate_runs: usize,
    pub accepted_candidates: usize,
    pub initial_hamming_weight: usize,
    pub final_hamming_weight: Option<usize>,
}

/// Reduce only the 16 initial XMM registers of one exact S2-40b fixture.
/// The callback is deliberately supplied by the caller so this crate does not
/// choose or launch a native/hardware backend.
pub fn reduce_native_xorpd_fixture<F>(
    original: &Fixture,
    call_budget: usize,
    execute: F,
) -> Result<NativeXorpdReduction, String>
where
    F: FnMut(&Fixture) -> Result<RunResult, String>,
{
    let original_json = original.to_json().map_err(|error| error.to_string())?;
    reduce_native_xorpd_fixture_with_input_digest(
        original,
        &sha256_hex(&original_json),
        call_budget,
        execute,
    )
}

/// Variant used when the original fixture was loaded from user-provided JSON
/// bytes. Its exact digest binds the two original-input callback pairs, while
/// generated candidates use their own canonical serialized bytes.
pub fn reduce_native_xorpd_fixture_with_input_digest<F>(
    original: &Fixture,
    original_input_sha256: &str,
    call_budget: usize,
    mut execute: F,
) -> Result<NativeXorpdReduction, String>
where
    F: FnMut(&Fixture) -> Result<RunResult, String>,
{
    if !(4..=NATIVE_XORPD_REDUCER_MAX_BUDGET).contains(&call_budget) {
        return Err(format!(
            "native XORPD reducer call budget must be 4..={NATIVE_XORPD_REDUCER_MAX_BUDGET}"
        ));
    }
    validate_reducer_fixture(original)?;
    let original_weight = xmm_hamming_weight(original)?;
    let mut report = NativeXorpdReduction {
        stop: NativeXorpdReductionStop::NoInitialMismatch,
        stop_detail: None,
        original_fixture: original.clone(),
        reduced_fixture: None,
        initial_results: Vec::with_capacity(2),
        final_results: Vec::new(),
        initial_fingerprint: None,
        final_fingerprint: None,
        actual_callbacks: 0,
        candidate_proposals: 0,
        candidate_runs: 0,
        accepted_candidates: 0,
        initial_hamming_weight: original_weight,
        final_hamming_weight: None,
    };

    let first = match invoke(&mut execute, original, &mut report.actual_callbacks) {
        Ok(result) => result,
        Err(error) => {
            report.stop = NativeXorpdReductionStop::CallbackError;
            report.stop_detail = Some(error);
            return Ok(report);
        }
    };
    let second = match invoke(&mut execute, original, &mut report.actual_callbacks) {
        Ok(result) => result,
        Err(error) => {
            report.initial_results.push(first);
            report.stop = NativeXorpdReductionStop::CallbackError;
            report.stop_detail = Some(error);
            return Ok(report);
        }
    };
    report
        .initial_results
        .extend([first.clone(), second.clone()]);
    if !same_semantic_result(&first, &second) {
        report.stop = NativeXorpdReductionStop::FlakyInitial;
        report.stop_detail = Some("the two initial callback results differ semantically".into());
        return Ok(report);
    }
    if let Some(detail) = callback_infrastructure_error(&first) {
        report.stop = NativeXorpdReductionStop::CallbackError;
        report.stop_detail = Some(detail);
        return Ok(report);
    }
    let fingerprints =
        mismatch_fingerprint_with_input_digest(original, &first, original_input_sha256).and_then(
            |first| {
                mismatch_fingerprint_with_input_digest(original, &second, original_input_sha256)
                    .map(|second| (first, second))
            },
        );
    let (first_fingerprint, second_fingerprint) = match fingerprints {
        Ok(fingerprints) => fingerprints,
        Err(error) => {
            report.stop = NativeXorpdReductionStop::CallbackError;
            report.stop_detail = Some(format!("invalid initial callback result: {error}"));
            return Ok(report);
        }
    };
    if first_fingerprint.is_none() || first_fingerprint != second_fingerprint {
        return Ok(report);
    }
    let target_fingerprint = first_fingerprint.expect("checked above");
    report.initial_fingerprint = Some(target_fingerprint.clone());

    let mut accepted = original.clone();
    let mut accepted_weight = original_weight;
    'registers: for register in 0..16 {
        let mut ranges = vec![(0usize, 16usize)];
        for block_size in [8usize, 4, 2, 1] {
            ranges.extend((0..16).step_by(block_size).map(|start| (start, block_size)));
        }
        for (start, length) in ranges {
            let current_bytes = xmm_bytes(&accepted, register)?;
            if current_bytes[start..start + length]
                .iter()
                .all(|byte| *byte == 0)
            {
                continue;
            }
            let mut candidate = accepted.clone();
            let mut candidate_bytes = current_bytes;
            candidate_bytes[start..start + length].fill(0);
            set_xmm_bytes(&mut candidate, register, &candidate_bytes)?;
            if !proposal_decreases_weight(&candidate, accepted_weight)? {
                continue;
            }
            report.candidate_proposals += 1;
            if !has_room_for_candidate(call_budget, report.actual_callbacks) {
                report.stop = NativeXorpdReductionStop::CandidateBudgetExhausted;
                report.stop_detail = Some("call budget exhausted before candidate pair".into());
                break 'registers;
            }
            match try_candidate(&candidate, &target_fingerprint, &mut execute, &mut report)? {
                CandidateDecision::Accept => {
                    accepted_weight = xmm_hamming_weight(&candidate)?;
                    accepted = candidate;
                    report.accepted_candidates += 1;
                }
                CandidateDecision::Reject => {}
                CandidateDecision::Flaky(detail) => {
                    report.stop = NativeXorpdReductionStop::FlakyCandidate;
                    report.stop_detail = Some(detail);
                    break 'registers;
                }
                CandidateDecision::Error(detail) => {
                    report.stop = NativeXorpdReductionStop::CallbackError;
                    report.stop_detail = Some(detail);
                    break 'registers;
                }
            }
        }
        // Clear each remaining set bit in deterministic byte order, MSB first.
        for byte_index in 0..16 {
            for bit in (0..8).rev() {
                let current_bytes = xmm_bytes(&accepted, register)?;
                let mask = 1u8 << bit;
                if current_bytes[byte_index] & mask == 0 {
                    continue;
                }
                let mut candidate = accepted.clone();
                let mut candidate_bytes = current_bytes;
                candidate_bytes[byte_index] &= !mask;
                set_xmm_bytes(&mut candidate, register, &candidate_bytes)?;
                report.candidate_proposals += 1;
                if !has_room_for_candidate(call_budget, report.actual_callbacks) {
                    report.stop = NativeXorpdReductionStop::CandidateBudgetExhausted;
                    report.stop_detail = Some("call budget exhausted before candidate pair".into());
                    break 'registers;
                }
                match try_candidate(&candidate, &target_fingerprint, &mut execute, &mut report)? {
                    CandidateDecision::Accept => {
                        accepted_weight = xmm_hamming_weight(&candidate)?;
                        accepted = candidate;
                        report.accepted_candidates += 1;
                    }
                    CandidateDecision::Reject => {}
                    CandidateDecision::Flaky(detail) => {
                        report.stop = NativeXorpdReductionStop::FlakyCandidate;
                        report.stop_detail = Some(detail);
                        break 'registers;
                    }
                    CandidateDecision::Error(detail) => {
                        report.stop = NativeXorpdReductionStop::CallbackError;
                        report.stop_detail = Some(detail);
                        break 'registers;
                    }
                }
            }
        }
    }

    if matches!(
        report.stop,
        NativeXorpdReductionStop::FlakyCandidate | NativeXorpdReductionStop::CallbackError
    ) {
        return Ok(report);
    }
    // The final two calls are reserved from the beginning and never consumed by
    // candidates. A reduction is published only after both confirmations.
    let final_first = match invoke(&mut execute, &accepted, &mut report.actual_callbacks) {
        Ok(result) => result,
        Err(error) => {
            report.stop = NativeXorpdReductionStop::CallbackError;
            report.stop_detail = Some(error);
            return Ok(report);
        }
    };
    let final_second = match invoke(&mut execute, &accepted, &mut report.actual_callbacks) {
        Ok(result) => result,
        Err(error) => {
            report.final_results.push(final_first);
            report.stop = NativeXorpdReductionStop::CallbackError;
            report.stop_detail = Some(error);
            return Ok(report);
        }
    };
    report
        .final_results
        .extend([final_first.clone(), final_second.clone()]);
    if !same_semantic_result(&final_first, &final_second) {
        report.stop = NativeXorpdReductionStop::FinalConfirmationFailed;
        report.stop_detail = Some("the two final callback results differ semantically".into());
        return Ok(report);
    }
    if let Some(detail) = callback_infrastructure_error(&final_first) {
        report.stop = NativeXorpdReductionStop::CallbackError;
        report.stop_detail = Some(detail);
        return Ok(report);
    }
    let accepted_input_digest = if &accepted == original {
        original_input_sha256.to_string()
    } else {
        sha256_hex(&accepted.to_json().map_err(|error| error.to_string())?)
    };
    let final_fingerprint = match mismatch_fingerprint_with_input_digest(
        &accepted,
        &final_first,
        &accepted_input_digest,
    ) {
        Ok(fingerprint) => fingerprint,
        Err(error) => {
            report.stop = NativeXorpdReductionStop::CallbackError;
            report.stop_detail = Some(format!("invalid final callback result: {error}"));
            return Ok(report);
        }
    };
    if final_fingerprint.as_ref() != Some(&target_fingerprint) {
        report.stop = NativeXorpdReductionStop::FinalConfirmationFailed;
        report.stop_detail =
            Some("final confirmations did not preserve the initial fingerprint".into());
        return Ok(report);
    }
    report.final_fingerprint = final_fingerprint;
    report.final_hamming_weight = Some(accepted_weight);
    if report.stop == NativeXorpdReductionStop::CandidateBudgetExhausted {
        if report.accepted_candidates > 0 {
            report.reduced_fixture = Some(accepted);
        }
    } else {
        if report.accepted_candidates > 0 {
            report.reduced_fixture = Some(accepted);
            report.stop = NativeXorpdReductionStop::Reduced;
        } else {
            report.stop = NativeXorpdReductionStop::NoReduction;
        }
    }
    Ok(report)
}

fn invoke<F>(execute: &mut F, fixture: &Fixture, actual: &mut usize) -> Result<RunResult, String>
where
    F: FnMut(&Fixture) -> Result<RunResult, String>,
{
    *actual += 1;
    execute(fixture)
}

enum CandidateDecision {
    Accept,
    Reject,
    Flaky(String),
    Error(String),
}

fn try_candidate<F>(
    candidate: &Fixture,
    expected: &NativeXorpdMismatchFingerprint,
    execute: &mut F,
    report: &mut NativeXorpdReduction,
) -> Result<CandidateDecision, String>
where
    F: FnMut(&Fixture) -> Result<RunResult, String>,
{
    report.candidate_runs += 1;
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
            "two candidate callback results differ semantically".into(),
        ));
    }
    if let Some(detail) = callback_infrastructure_error(&first) {
        return Ok(CandidateDecision::Error(detail));
    }
    let fingerprints = mismatch_fingerprint(candidate, &first)
        .and_then(|first| mismatch_fingerprint(candidate, &second).map(|second| (first, second)));
    let (first_fp, second_fp) = match fingerprints {
        Ok(fingerprints) => fingerprints,
        Err(error) => {
            return Ok(CandidateDecision::Error(format!(
                "invalid candidate callback result: {error}"
            )));
        }
    };
    Ok(
        if first_fp.as_ref() == Some(expected) && second_fp == first_fp {
            CandidateDecision::Accept
        } else {
            CandidateDecision::Reject
        },
    )
}

fn has_room_for_candidate(budget: usize, actual: usize) -> bool {
    actual.saturating_add(4) <= budget
}

fn proposal_decreases_weight(candidate: &Fixture, current: usize) -> Result<bool, String> {
    Ok(xmm_hamming_weight(candidate)? < current)
}

fn validate_reducer_fixture(fixture: &Fixture) -> Result<(), String> {
    fixture.validate().map_err(|error| error.to_string())?;
    if fixture.profile != Profile::LongNativeXorpdXmm0Xmm1V1 {
        return Err("reducer accepts only s2-long-native-xorpd-xmm0-xmm1-v1".into());
    }
    Ok(())
}

fn xmm_bytes(fixture: &Fixture, register: usize) -> Result<Vec<u8>, String> {
    let sse = fixture
        .initial_state
        .sse
        .as_ref()
        .ok_or_else(|| "reducer fixture is missing SSE state".to_string())?;
    decode_hex(&sse.xmm[register])
}

fn set_xmm_bytes(fixture: &mut Fixture, register: usize, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() != 16 {
        return Err("reducer XMM register must contain exactly 16 bytes".into());
    }
    let sse = fixture
        .initial_state
        .sse
        .as_mut()
        .ok_or_else(|| "reducer fixture is missing SSE state".to_string())?;
    sse.xmm[register] = encode_hex(bytes);
    Ok(())
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if value.len() % 2 != 0 {
        return Err("XMM hex value has an odd character count".into());
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

fn xmm_hamming_weight(fixture: &Fixture) -> Result<usize, String> {
    (0..16).try_fold(0usize, |weight, register| {
        Ok(weight
            + xmm_bytes(fixture, register)?
                .iter()
                .map(|byte| byte.count_ones() as usize)
                .sum::<usize>())
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

fn callback_infrastructure_error(result: &RunResult) -> Option<String> {
    match &result.outcome {
        Outcome::HarnessError { reason } => Some(format!("callback harness error: {reason}")),
        Outcome::Signaled { signal, reason } => {
            Some(format!("callback process signaled ({signal}): {reason}"))
        }
        _ => None,
    }
}

/// Return a fingerprint only for successful one-instruction results whose
/// complete non-XMM state matches the independent XOR oracle and whose XMM
/// mismatch is nonempty. Refusal/fault/timeout/infrastructure outcomes do not
/// preserve a mismatch.
pub fn mismatch_fingerprint(
    fixture: &Fixture,
    result: &RunResult,
) -> Result<Option<NativeXorpdMismatchFingerprint>, String> {
    let fixture_json = fixture.to_json().map_err(|error| error.to_string())?;
    mismatch_fingerprint_with_input_digest(fixture, result, &sha256_hex(&fixture_json))
}

fn mismatch_fingerprint_with_input_digest(
    fixture: &Fixture,
    result: &RunResult,
    expected_input_sha256: &str,
) -> Result<Option<NativeXorpdMismatchFingerprint>, String> {
    use crate::result::Observation;

    if result.profile != Profile::LongNativeXorpdXmm0Xmm1V1
        || result.backend != crate::result::Backend::Interpreter
        || result.case_id != fixture.case_id
        || result.input_sha256 != expected_input_sha256
    {
        return Err("callback result identity does not match its exact fixture".into());
    }
    let success = matches!(result.outcome, Outcome::StopAddress {})
        && result.instructions_executed == (Observation::Available { value: HexU64(1) });
    if !success {
        return Ok(None);
    }
    let Observation::Available { value: snapshot } = &result.snapshot else {
        return Err("successful callback result has no snapshot".into());
    };
    let Some(initial_sse) = fixture.initial_state.sse.as_ref() else {
        return Ok(None);
    };
    let mut expected = fixture.initial_state.clone();
    let mut xmm0 = decode_hex(&initial_sse.xmm[0])?;
    let xmm1 = decode_hex(&initial_sse.xmm[1])?;
    if xmm0.len() != 16 || xmm1.len() != 16 {
        return Ok(None);
    }
    for (destination, source) in xmm0.iter_mut().zip(xmm1) {
        *destination ^= source;
    }
    expected.sse.as_mut().expect("SSE checked").xmm[0] = encode_hex(&xmm0);
    expected.rip = fixture
        .stop_rip
        .ok_or_else(|| "strict fixture has no stop RIP".to_string())?;
    let mut actual_non_xmm = snapshot.state.clone();
    let Some(actual_sse) = actual_non_xmm.sse.as_mut() else {
        return Err("successful callback result has no SSE state".into());
    };
    let Some(expected_sse) = expected.sse.as_ref() else {
        return Ok(None);
    };
    let mut mismatches = Vec::new();
    for register in 0..16 {
        let actual_bytes = decode_hex(&actual_sse.xmm[register])?;
        let expected_bytes = decode_hex(&expected_sse.xmm[register])?;
        if actual_bytes.len() != 16 || expected_bytes.len() != 16 {
            return Err(format!(
                "successful callback result has invalid XMM{register} width"
            ));
        }
        for byte in 0..16 {
            if actual_bytes[byte] != expected_bytes[byte] {
                mismatches.push([register as u8, byte as u8]);
            }
        }
        actual_sse.xmm[register] = expected_sse.xmm[register].clone();
    }
    if actual_non_xmm != expected
        || snapshot.rflags_defined != fixture.rflags_mask
        || !matches!(snapshot.exception, Observation::Available { value: None })
        || snapshot.xorpd_prerequisites.is_some()
        || snapshot.minss_invalid_controls.is_some()
        || snapshot.fxsave64_cr0_controls.is_some()
        || snapshot.fxrstor64_cr0_controls.is_some()
        || snapshot.memory.len() != 1
        || snapshot.memory[0].region_id != "code"
        || snapshot.memory[0].guest_address != fixture.memory[0].guest_address
        || snapshot.memory[0].bytes_hex != fixture.memory[0].initial_bytes_hex
    {
        return Ok(None);
    }
    if mismatches.is_empty() {
        return Ok(None);
    }
    Ok(Some(NativeXorpdMismatchFingerprint {
        mismatching_xmm_bytes: mismatches,
    }))
}

/// One deterministic schema-v2 input for the exact native XORPD profile.
pub fn generate_native_xorpd_fixture(seed: u64, sequence: u32) -> Fixture {
    let mut random = SplitMix64(seed ^ u64::from(sequence).wrapping_mul(0x9e37_79b9_7f4a_7c15));
    let mut xmm = std::array::from_fn(|_| String::new());
    for register in &mut xmm {
        let mut bytes = [0; 16];
        for chunk in bytes.chunks_exact_mut(8) {
            chunk.copy_from_slice(&random.next().to_le_bytes());
        }
        *register = encode_hex(&bytes);
    }

    let rip = 0x0010_0000;
    Fixture {
        schema_version: SCHEMA_VERSION,
        case_id: format!("s2_42a_xorpd_{seed:016x}_{sequence:04x}"),
        profile: Profile::LongNativeXorpdXmm0Xmm1V1,
        cli_cpl: None,
        efer_cpl: None,
        efer_value: None,
        provenance: Provenance {
            kind: "deterministic_generator".into(),
            source: format!(
                "{NATIVE_XORPD_GENERATOR_VERSION}; seed=0x{seed:016x}; sequence={sequence}"
            ),
            specification: "docs/specs/CPU_CONTRACT.md".into(),
            specification_version: "S2-40b/S2-42a; Intel SDM Rev. 093".into(),
        },
        seed: Some(HexU64(seed)),
        converted_from_v1_sha256: None,
        initial_state: IntegerState {
            mode: Mode::Long64,
            gpr: std::array::from_fn(|index| HexU64(0x1020_3040_5060_7080 ^ index as u64)),
            rip: HexU64(rip),
            rflags: HexU64(2),
            cr0: None,
            cr4: None,
            fs_base: None,
            gs_base: None,
            kernel_gs_base: None,
            fs_selector: None,
            gs_selector: None,
            ds_selector: None,
            es_selector: None,
            ds_unusable: None,
            es_unusable: None,
            gdtr_base: None,
            gdtr_limit: None,
            cs_selector: None,
            cs_cache_base: None,
            cs_cache_limit: None,
            cs_cache_access: None,
            cs_cache_flags: None,
            ss_selector: None,
            ss_cache_base: None,
            ss_cache_limit: None,
            ss_cache_access: None,
            ss_cache_flags: None,
            fs_cache_limit: None,
            fs_cache_access: None,
            fs_cache_flags: None,
            gs_cache_limit: None,
            gs_cache_access: None,
            gs_cache_flags: None,
            x87: None,
            sse: Some(SseSnapshot {
                xmm,
                mxcsr: HexU64(0x1f80),
                diagnostic_mxcsr_mask: HexU64(0x0000_ffbf),
            }),
            sti_shadow: None,

            activity: None,
        },
        xorpd_prerequisites: None,
        comisd_memory_rax_controls: None,
        ucomisd_memory_rax_controls: None,
        comiss_memory_rax_controls: None,
        ucomiss_memory_rax_controls: None,
        cmpss_memory_rax_controls: None,
        cmpsd_memory_rax_controls: None,
        cmppd_memory_rax_controls: None,
        cmpps_memory_rax_controls: None,
        minsd_memory_rax_controls: None,
        maxss_memory_rax_controls: None,
        maxsd_memory_rax_controls: None,
        maxpd_memory_rax_controls: None,
        minss_invalid_controls: None,
        maxss_unmasked_controls: None,
        minsd_unmasked_controls: None,
        maxsd_unmasked_controls: None,
        minpd_unmasked_controls: None,
        minps_unmasked_controls: None,
        maxps_unmasked_controls: None,
        maxpd_unmasked_controls: None,
        cvtdq2pd_controls: None,
        cvtdq2pd_memory_controls: None,
        cvtdq2ps_controls: None,
        cvtdq2ps_memory_controls: None,
        cvtps2dq_controls: None,
        cvttps2dq_controls: None,
        cvttps2dq_m128_controls: None,
        cvtps2dq_m128_controls: None,
        cvtpd2dq_controls: None,
        cvttpd2dq_controls: None,
        cvttpd2dq_m128_controls: None,
        cvtpd2dq_m128_controls: None,
        sqrtss_controls: None,
        sqrtsd_controls: None,
        sqrtps_controls: None,
        sqrtpd_controls: None,
        subpd_controls: None,
        mulpd_controls: None,
        addpd_controls: None,
        cvtpd2ps_controls: None,
        addsd_controls: None,
        addss_controls: None,
        addps_controls: None,
        divpd_controls: None,
        divsd_controls: None,
        divps_controls: None,
        divss_controls: None,
        mulsd_controls: None,
        mulss_controls: None,
        mulps_controls: None,
        subps_controls: None,
        subsd_controls: None,
        subss_controls: None,
        cvtss2sd_controls: None,
        cvtsd2ss_controls: None,
        cvtpi2pd_memory_controls: None,
        cvtpi2pd_register_controls: None,
        cvtpi2ps_memory_controls: None,
        cvtpi2ps_register_controls: None,
        cvttps2pi_memory_controls: None,
        cvtps2pi_memory_controls: None,
        cvtps2pi_register_controls: None,
        cvtpd2pi_register_controls: None,
        cvttpd2pi_register_controls: None,
        cvttpd2pi_memory_controls: None,
        cvtpd2pi_memory_controls: None,
        cvtps2pd_controls: None,
        cvtps2pd_m64_controls: None,
        cvtss2sd_m32_controls: None,
        cvtsd2ss_m64_controls: None,
        cvtpd2ps_m128_controls: None,
        comiss_ucomiss_controls: None,
        cmpss_controls: None,
        cmpsd_controls: None,
        cmppd_controls: None,
        cmpps_controls: None,
        comisd_ucomisd_controls: None,
        cvtsi2sd_r32_controls: None,
        cvtsi2sd_r32_memory_controls: None,
        cvtsi2ss_r32_memory_controls: None,
        cvtsi2ss_r64_memory_controls: None,
        cvtsi2sd_r64_controls: None,
        cvtsi2sd_r64_memory_controls: None,
        cvtsi2ss_controls: None,
        cvtss2si_controls: None,
        cvtsd2si_controls: None,
        cvtsd2si_r32_memory_controls: None,
        cvtsd2si_r64_memory_controls: None,
        cvttsd2si_r32_memory_controls: None,
        cvttsd2si_r64_memory_controls: None,
        cvtss2si_r32_memory_controls: None,
        cvtss2si_r64_memory_controls: None,
        cvttss2si_r32_memory_controls: None,
        cvttss2si_r64_memory_controls: None,
        fxsave64_cr0_controls: None,
        fxrstor64_cr0_controls: None,
        rdtsc_controls: None,
        memory: vec![MemoryRegionSpec {
            region_id: "code".into(),
            guest_address: HexU64(rip),
            length: 4,
            permissions: MemoryPermissions {
                read: true,
                write: false,
                execute: true,
            },
            initial_bytes_hex: "660f57c1".into(),
            comparison_mask_hex: "ff".repeat(4),
        }],
        stop_rip: Some(HexU64(rip + 4)),
        instruction_budget: 1,
        wall_timeout_ms: 2_000,
        rflags_mask: HexU64(0x0000_0cd7),
        expected_exit: ExpectedExit::StopAddress,
    }
}

/// Ordered manifest for a fixed native XORPD corpus.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeXorpdCorpusManifest {
    pub manifest_version: u32,
    pub generator_version: String,
    pub profile: String,
    pub seeds: Vec<String>,
    pub sequence: String,
    pub case_digests: Vec<String>,
    pub corpus_digest: String,
}

impl NativeXorpdCorpusManifest {
    pub fn from_ordered_case_bytes(case_bytes: &[Vec<u8>]) -> Result<Self, String> {
        let expected_count = NATIVE_XORPD_FIXED_SEEDS.len() * NATIVE_XORPD_CASES_PER_SEED as usize;
        if case_bytes.len() != expected_count {
            return Err(format!(
                "native XORPD corpus requires {expected_count} ordered cases, got {}",
                case_bytes.len()
            ));
        }
        let case_digests = case_bytes
            .iter()
            .map(|bytes| sha256_hex(bytes))
            .collect::<Vec<_>>();
        Ok(Self {
            manifest_version: NATIVE_XORPD_MANIFEST_VERSION,
            generator_version: NATIVE_XORPD_GENERATOR_VERSION.into(),
            profile: PROFILE_NATIVE_XORPD_XMM0_XMM1.into(),
            seeds: NATIVE_XORPD_FIXED_SEEDS
                .iter()
                .map(|seed| format!("0x{seed:016x}"))
                .collect(),
            sequence: format!("0..{} (exclusive)", NATIVE_XORPD_CASES_PER_SEED),
            corpus_digest: corpus_digest(&case_digests),
            case_digests,
        })
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        let value: Self = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        value.validate()?;
        Ok(value)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        serde_json::to_vec_pretty(self).map_err(|error| error.to_string())
    }

    pub fn validate(&self) -> Result<(), String> {
        let expected_count = NATIVE_XORPD_FIXED_SEEDS.len() * NATIVE_XORPD_CASES_PER_SEED as usize;
        let seeds = NATIVE_XORPD_FIXED_SEEDS
            .iter()
            .map(|seed| format!("0x{seed:016x}"))
            .collect::<Vec<_>>();
        if self.manifest_version != NATIVE_XORPD_MANIFEST_VERSION
            || self.generator_version != NATIVE_XORPD_GENERATOR_VERSION
            || self.profile != PROFILE_NATIVE_XORPD_XMM0_XMM1
            || self.seeds != seeds
            || self.sequence != format!("0..{} (exclusive)", NATIVE_XORPD_CASES_PER_SEED)
            || self.case_digests.len() != expected_count
            || self.case_digests.iter().any(|digest| !is_sha256(digest))
            || self.corpus_digest != corpus_digest(&self.case_digests)
        {
            return Err("native XORPD corpus manifest is invalid or inconsistent".into());
        }
        Ok(())
    }
}

pub fn native_xorpd_case_id(seed: u64, sequence: u32) -> String {
    format!("s2_42a_xorpd_{seed:016x}_{sequence:04x}")
}

pub fn corpus_digest(case_digests: &[String]) -> String {
    sha256_hex(case_digests.join("\n").as_bytes())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(result, "{byte:02x}").expect("writing to String cannot fail");
    }
    result
}

struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::{
        Backend, BuildMetadata, MachineMetadata, MemoryRegionSnapshot, Observation,
    };
    use crate::v2::Snapshot;
    use std::collections::HashSet;

    fn fixed_case_bytes() -> Vec<Vec<u8>> {
        NATIVE_XORPD_FIXED_SEEDS
            .iter()
            .flat_map(|&seed| {
                (0..NATIVE_XORPD_CASES_PER_SEED).map(move |sequence| {
                    generate_native_xorpd_fixture(seed, sequence)
                        .to_json()
                        .unwrap()
                })
            })
            .collect()
    }

    #[test]
    fn fixed_corpus_has_deterministic_distinct_inputs_and_digests() {
        let first = fixed_case_bytes();
        let mut input_states = HashSet::new();
        let mut digests = HashSet::new();
        let mut index = 0usize;
        for &seed in &NATIVE_XORPD_FIXED_SEEDS {
            for sequence in 0..NATIVE_XORPD_CASES_PER_SEED {
                let bytes = &first[index];
                let regenerated = generate_native_xorpd_fixture(seed, sequence)
                    .to_json()
                    .unwrap();
                assert_eq!(*bytes, regenerated, "nondeterministic case {index}");
                let fixture = Fixture::from_json(bytes).unwrap().fixture;
                let sse = fixture.initial_state.sse.unwrap();
                let input = sse.xmm.join("");
                assert!(input_states.insert(input), "duplicate XMM input state");
                assert!(digests.insert(sha256_hex(bytes)), "duplicate case digest");
                index += 1;
            }
        }
        let manifest = NativeXorpdCorpusManifest::from_ordered_case_bytes(&first).unwrap();
        assert_eq!(
            manifest,
            NativeXorpdCorpusManifest::from_ordered_case_bytes(&first).unwrap()
        );
        assert_eq!(
            NativeXorpdCorpusManifest::from_json(&manifest.to_json().unwrap()).unwrap(),
            manifest
        );
    }

    fn reducer_fixture() -> Fixture {
        let mut fixture = generate_native_xorpd_fixture(NATIVE_XORPD_FIXED_SEEDS[0], 0);
        let sse = fixture.initial_state.sse.as_mut().unwrap();
        for register in &mut sse.xmm {
            *register = "00000000000000000000000000000000".into();
        }
        sse.xmm[0] = "01000000000000000000000000000000".into();
        sse.xmm[1] = "02000000000000000000000000000000".into();
        sse.xmm[15] = "01000000000000000000000000000000".into();
        fixture
    }

    fn synthetic_result(fixture: &Fixture, mismatch_byte: Option<usize>) -> RunResult {
        let mut state = fixture.initial_state.clone();
        let sse = state.sse.as_mut().unwrap();
        let mut x0 = decode_hex(&sse.xmm[0]).unwrap();
        let x1 = decode_hex(&sse.xmm[1]).unwrap();
        for (left, right) in x0.iter_mut().zip(x1) {
            *left ^= right;
        }
        sse.xmm[0] = encode_hex(&x0);
        state.rip = fixture.stop_rip.unwrap();
        if let Some(byte) = mismatch_byte {
            let mut bytes = decode_hex(&sse.xmm[0]).unwrap();
            bytes[byte] ^= 0x80;
            sse.xmm[0] = encode_hex(&bytes);
        }
        RunResult {
            schema_version: crate::v2::SCHEMA_VERSION,
            case_id: fixture.case_id.clone(),
            input_sha256: sha256_hex(&fixture.to_json().unwrap()),
            profile: fixture.profile,
            backend: Backend::Interpreter,
            outcome: Outcome::StopAddress {},
            instructions_executed: Observation::Available { value: HexU64(1) },
            snapshot: Observation::Available {
                value: Snapshot {
                    state,
                    rflags_defined: fixture.rflags_mask,
                    memory: vec![MemoryRegionSnapshot {
                        region_id: "code".into(),
                        guest_address: fixture.memory[0].guest_address,
                        bytes_hex: fixture.memory[0].initial_bytes_hex.clone(),
                    }],
                    xorpd_prerequisites: None,
                    minss_invalid_controls: None,
                    maxss_unmasked_controls: None,
                    minsd_unmasked_controls: None,
                    maxsd_unmasked_controls: None,
                    minpd_unmasked_controls: None,
                    minps_unmasked_controls: None,
                    maxps_unmasked_controls: None,
                    maxpd_unmasked_controls: None,
                    cvtdq2pd_controls: None,
                    cvtdq2pd_memory_controls: None,
                    cvtdq2ps_controls: None,
                    cvtdq2ps_memory_controls: None,
                    cvtps2dq_controls: None,
                    cvttps2dq_controls: None,
                    cvttps2dq_m128_controls: None,
                    cvtps2dq_m128_controls: None,
                    cvtpd2dq_controls: None,
                    cvttpd2dq_controls: None,
                    cvttpd2dq_m128_controls: None,
                    cvtpd2dq_m128_controls: None,
                    sqrtss_controls: None,
                    sqrtsd_controls: None,
                    sqrtps_controls: None,
                    sqrtpd_controls: None,
                    subpd_controls: None,
                    mulpd_controls: None,
                    addpd_controls: None,
                    cvtpd2ps_controls: None,
                    addsd_controls: None,
                    addss_controls: None,
                    addps_controls: None,
                    divpd_controls: None,
                    divsd_controls: None,
                    divps_controls: None,
                    divss_controls: None,
                    mulsd_controls: None,
                    mulss_controls: None,
                    mulps_controls: None,
                    subps_controls: None,
                    subsd_controls: None,
                    subss_controls: None,
                    cvtss2sd_controls: None,
                    cvtsd2ss_controls: None,
                    cvtpi2pd_memory_controls: None,
                    cvtpi2pd_register_controls: None,
                    cvtpi2ps_memory_controls: None,
                    cvtpi2ps_register_controls: None,
                    cvttps2pi_memory_controls: None,
                    cvtps2pi_memory_controls: None,
                    cvtps2pi_register_controls: None,
                    cvtpd2pi_register_controls: None,
                    cvttpd2pi_register_controls: None,
                    cvttpd2pi_memory_controls: None,
                    cvtpd2pi_memory_controls: None,
                    cvtps2pd_controls: None,
                    cvtps2pd_m64_controls: None,
                    cvtss2sd_m32_controls: None,
                    cvtsd2ss_m64_controls: None,
                    cvtpd2ps_m128_controls: None,
                    comiss_ucomiss_controls: None,
                    cmpss_controls: None,
                    cmpsd_controls: None,
                    cmppd_controls: None,
                    cmpps_controls: None,
                    comisd_ucomisd_controls: None,
                    comisd_memory_rax_controls: None,
                    ucomisd_memory_rax_controls: None,
                    comiss_memory_rax_controls: None,
                    ucomiss_memory_rax_controls: None,
                    cmpss_memory_rax_controls: None,
                    cmpsd_memory_rax_controls: None,
                    cmppd_memory_rax_controls: None,
                    cmpps_memory_rax_controls: None,
                    minsd_memory_rax_controls: None,
                    maxss_memory_rax_controls: None,
                    maxsd_memory_rax_controls: None,
                    maxpd_memory_rax_controls: None,
                    cvtsi2sd_r32_controls: None,
                    cvtsi2sd_r32_memory_controls: None,
                    cvtsi2ss_r32_memory_controls: None,
                    cvtsi2ss_r64_memory_controls: None,
                    cvtsi2sd_r64_controls: None,
                    cvtsi2sd_r64_memory_controls: None,
                    cvtsi2ss_controls: None,
                    cvtss2si_controls: None,
                    cvtsd2si_controls: None,
                    cvtsd2si_r32_memory_controls: None,
                    cvtsd2si_r64_memory_controls: None,
                    cvttsd2si_r32_memory_controls: None,
                    cvttsd2si_r64_memory_controls: None,
                    cvtss2si_r32_memory_controls: None,
                    cvtss2si_r64_memory_controls: None,
                    cvttss2si_r32_memory_controls: None,
                    cvttss2si_r64_memory_controls: None,
                    fxsave64_cr0_controls: None,
                    fxrstor64_cr0_controls: None,
                    rdtsc_source_calls: None,
                    exception: Observation::Available { value: None },
                },
            },
            build: BuildMetadata {
                revision: "synthetic-test-only".into(),
                target: "test".into(),
                profile: "test".into(),
                compiler: "test".into(),
            },
            machine: MachineMetadata {
                host_os: "test".into(),
                host_arch: "test".into(),
                cpu_model: None,
                kernel: None,
                page_size: None,
            },
        }
    }

    fn synthetic_xmm15_bug(fixture: &Fixture) -> RunResult {
        let has_xmm15_input = decode_hex(&fixture.initial_state.sse.as_ref().unwrap().xmm[15])
            .unwrap()
            .iter()
            .any(|byte| *byte != 0);
        synthetic_result(fixture, has_xmm15_input.then_some(0))
    }

    fn synthetic_upper_xmm_bug(fixture: &Fixture) -> RunResult {
        let sse = fixture.initial_state.sse.as_ref().unwrap();
        let has_upper_input = (2..16).any(|register| {
            decode_hex(&sse.xmm[register])
                .unwrap()
                .iter()
                .any(|byte| *byte != 0)
        });
        synthetic_result(fixture, has_upper_input.then_some(0))
    }

    #[test]
    fn reducer_is_deterministic_and_reduces_full_xmm_state() {
        let fixture = reducer_fixture();
        let first =
            reduce_native_xorpd_fixture(&fixture, 5120, |input| Ok(synthetic_xmm15_bug(input)))
                .unwrap();
        let second =
            reduce_native_xorpd_fixture(&fixture, 5120, |input| Ok(synthetic_xmm15_bug(input)))
                .unwrap();
        assert_eq!(first.stop, NativeXorpdReductionStop::Reduced);
        assert_eq!(first.reduced_fixture, second.reduced_fixture);
        assert_eq!(first.final_fingerprint, second.final_fingerprint);
        let reduced = first.reduced_fixture.unwrap();
        assert_eq!(
            reduced.initial_state.sse.as_ref().unwrap().xmm[0],
            "00000000000000000000000000000000"
        );
        assert_eq!(
            reduced.initial_state.sse.as_ref().unwrap().xmm[1],
            "00000000000000000000000000000000"
        );
        let mut normalized = reduced.clone();
        normalized.initial_state.sse.as_mut().unwrap().xmm =
            fixture.initial_state.sse.as_ref().unwrap().xmm.clone();
        assert_eq!(
            normalized, fixture,
            "reduction changed non-XMM fixture fields"
        );
        assert_ne!(
            reduced.initial_state.sse.as_ref().unwrap().xmm[15],
            "00000000000000000000000000000000"
        );
        assert!(first.actual_callbacks <= 5092);
        assert_eq!(first.initial_results.len(), 2);
        assert_eq!(first.final_results.len(), 2);
    }

    #[test]
    fn reducer_checks_xmm2_through_xmm15_dependencies() {
        let mut fixture = reducer_fixture();
        let sse = fixture.initial_state.sse.as_mut().unwrap();
        for register in 2..16 {
            sse.xmm[register] = "01000000000000000000000000000000".into();
        }
        let report =
            reduce_native_xorpd_fixture(&fixture, 5120, |input| Ok(synthetic_upper_xmm_bug(input)))
                .unwrap();
        assert_eq!(report.stop, NativeXorpdReductionStop::Reduced);
        let reduced = report.reduced_fixture.unwrap();
        for register in 2..15 {
            assert_eq!(
                reduced.initial_state.sse.as_ref().unwrap().xmm[register],
                "00000000000000000000000000000000",
                "XMM{register} was not considered by the reducer"
            );
        }
        assert_ne!(
            reduced.initial_state.sse.as_ref().unwrap().xmm[15],
            "00000000000000000000000000000000"
        );
    }

    #[test]
    fn mismatch_fingerprint_keeps_positions_but_ignores_wrong_values() {
        let fixture = reducer_fixture();
        let mut a = synthetic_result(&fixture, Some(0));
        let mut b = a.clone();
        let Observation::Available { value: snapshot } = &mut b.snapshot else {
            unreachable!()
        };
        let sse = snapshot.state.sse.as_mut().unwrap();
        let mut bytes = decode_hex(&sse.xmm[0]).unwrap();
        bytes[0] ^= 0x40;
        sse.xmm[0] = encode_hex(&bytes);
        assert_eq!(
            mismatch_fingerprint(&fixture, &a).unwrap(),
            mismatch_fingerprint(&fixture, &b).unwrap()
        );
        let Observation::Available { value: snapshot } = &mut b.snapshot else {
            unreachable!()
        };
        let sse = snapshot.state.sse.as_mut().unwrap();
        let mut bytes = decode_hex(&sse.xmm[0]).unwrap();
        bytes[1] ^= 0x80;
        sse.xmm[0] = encode_hex(&bytes);
        assert_ne!(
            mismatch_fingerprint(&fixture, &a).unwrap(),
            mismatch_fingerprint(&fixture, &b).unwrap()
        );
        a.build.revision = "different-metadata-does-not-matter".into();
    }

    #[test]
    fn no_initial_mismatch_does_not_create_reduced_case() {
        let fixture = reducer_fixture();
        let report =
            reduce_native_xorpd_fixture(&fixture, 4, |input| Ok(synthetic_result(input, None)))
                .unwrap();
        assert_eq!(report.stop, NativeXorpdReductionStop::NoInitialMismatch);
        assert!(report.reduced_fixture.is_none());
        assert_eq!(report.actual_callbacks, 2);
    }

    #[test]
    fn callback_budget_reserves_initial_and_final_pairs() {
        for budget in [4, 5, 6] {
            let fixture = reducer_fixture();
            let report = reduce_native_xorpd_fixture(&fixture, budget, |input| {
                Ok(synthetic_xmm15_bug(input))
            })
            .unwrap();
            assert!(report.actual_callbacks <= budget);
            assert_eq!(report.initial_results.len(), 2);
            if budget < 6 {
                assert_eq!(report.candidate_runs, 0);
                assert_eq!(report.final_results.len(), 2);
            }
        }
    }

    #[test]
    fn zero_proposals_skip_noops_and_refusals_do_not_preserve_mismatch() {
        let mut zero = reducer_fixture();
        let sse = zero.initial_state.sse.as_mut().unwrap();
        for register in &mut sse.xmm {
            *register = "00000000000000000000000000000000".into();
        }
        let no_op =
            reduce_native_xorpd_fixture(&zero, 32, |input| Ok(synthetic_result(input, Some(0))))
                .unwrap();
        assert_eq!(no_op.stop, NativeXorpdReductionStop::NoReduction);
        assert_eq!(no_op.candidate_proposals, 0);
        assert!(no_op.reduced_fixture.is_none());

        let fixture = reducer_fixture();
        let initial_digest = sha256_hex(&fixture.to_json().unwrap());
        let refused = reduce_native_xorpd_fixture(&fixture, 20, |candidate| {
            let mut result = synthetic_xmm15_bug(candidate);
            if sha256_hex(&candidate.to_json().unwrap()) != initial_digest {
                result.outcome = Outcome::Unsupported {
                    reason: "synthetic refusal".into(),
                };
            }
            Ok(result)
        })
        .unwrap();
        assert_eq!(
            refused.stop,
            NativeXorpdReductionStop::CandidateBudgetExhausted
        );
        assert_eq!(refused.accepted_candidates, 0);
        assert!(refused.reduced_fixture.is_none());

        for outcome in [
            Outcome::TimedOut {
                reason: "synthetic timeout".into(),
            },
            Outcome::Exception {},
        ] {
            let fixture = reducer_fixture();
            let original_digest = sha256_hex(&fixture.to_json().unwrap());
            let report = reduce_native_xorpd_fixture(&fixture, 6, |candidate| {
                let mut result = synthetic_xmm15_bug(candidate);
                if sha256_hex(&candidate.to_json().unwrap()) != original_digest {
                    result.outcome = outcome.clone();
                }
                Ok(result)
            })
            .unwrap();
            assert_eq!(report.candidate_runs, 1);
            assert_eq!(report.accepted_candidates, 0);
            assert_eq!(report.final_results.len(), 2);
            assert!(report.reduced_fixture.is_none());
        }
    }

    #[test]
    fn flaky_initial_candidate_final_and_callback_error_are_reported() {
        let fixture = reducer_fixture();
        let mut calls = 0usize;
        let initial_flaky = reduce_native_xorpd_fixture(&fixture, 4, |input| {
            calls += 1;
            Ok(synthetic_result(input, (calls == 2).then_some(1)))
        })
        .unwrap();
        assert_eq!(initial_flaky.stop, NativeXorpdReductionStop::FlakyInitial);

        let mut calls = 0usize;
        let candidate_flaky = reduce_native_xorpd_fixture(&fixture, 8, |input| {
            calls += 1;
            let mut result = synthetic_xmm15_bug(input);
            if calls == 4 {
                let Observation::Available { value: snapshot } = &mut result.snapshot else {
                    unreachable!()
                };
                let xmm = &mut snapshot.state.sse.as_mut().unwrap().xmm[0];
                *xmm = "00000000000000000000000000000080".into();
            }
            Ok(result)
        })
        .unwrap();
        assert_eq!(
            candidate_flaky.stop,
            NativeXorpdReductionStop::FlakyCandidate
        );
        assert!(candidate_flaky.reduced_fixture.is_none());

        let mut calls = 0usize;
        let final_flaky = reduce_native_xorpd_fixture(&fixture, 4, |input| {
            calls += 1;
            let mut result = synthetic_xmm15_bug(input);
            if calls == 4 {
                let Observation::Available { value: snapshot } = &mut result.snapshot else {
                    unreachable!()
                };
                snapshot.state.gpr[0].0 ^= 1;
            }
            Ok(result)
        })
        .unwrap();
        assert_eq!(
            final_flaky.stop,
            NativeXorpdReductionStop::FinalConfirmationFailed
        );
        assert!(final_flaky.reduced_fixture.is_none());

        let mut calls = 0usize;
        let callback_error = reduce_native_xorpd_fixture(&fixture, 4, |_| {
            calls += 1;
            if calls == 2 {
                Err("synthetic timeout/infra error".into())
            } else {
                Ok(synthetic_xmm15_bug(&fixture))
            }
        })
        .unwrap();
        assert_eq!(callback_error.stop, NativeXorpdReductionStop::CallbackError);
        assert!(callback_error.reduced_fixture.is_none());

        let harness_error = reduce_native_xorpd_fixture(&fixture, 4, |input| {
            let mut result = synthetic_xmm15_bug(input);
            result.outcome = Outcome::HarnessError {
                reason: "synthetic callback failure".into(),
            };
            Ok(result)
        })
        .unwrap();
        assert_eq!(harness_error.stop, NativeXorpdReductionStop::CallbackError);
        assert!(harness_error.reduced_fixture.is_none());

        for malformed_xmm in ["00".to_string(), "0".repeat(33)] {
            let malformed = reduce_native_xorpd_fixture(&fixture, 4, |input| {
                let mut result = synthetic_xmm15_bug(input);
                let Observation::Available { value: snapshot } = &mut result.snapshot else {
                    unreachable!()
                };
                snapshot.state.sse.as_mut().unwrap().xmm[0] = malformed_xmm.clone();
                Ok(result)
            })
            .unwrap();
            assert_eq!(malformed.stop, NativeXorpdReductionStop::CallbackError);
            assert!(
                malformed
                    .stop_detail
                    .unwrap()
                    .contains("invalid initial callback result")
            );
        }
    }

    #[test]
    fn manifest_rejects_missing_cases_and_digest_corruption() {
        let bytes = fixed_case_bytes();
        assert!(
            NativeXorpdCorpusManifest::from_ordered_case_bytes(&bytes[..bytes.len() - 1]).is_err()
        );
        let mut manifest = NativeXorpdCorpusManifest::from_ordered_case_bytes(&bytes).unwrap();
        manifest.case_digests[0].replace_range(0..1, "0");
        assert!(manifest.validate().is_err());
        let manifest = NativeXorpdCorpusManifest::from_ordered_case_bytes(&bytes).unwrap();
        let mut json: serde_json::Value =
            serde_json::from_slice(&manifest.to_json().unwrap()).unwrap();
        json["corpus_digest"] = serde_json::Value::String("0".repeat(64));
        assert!(NativeXorpdCorpusManifest::from_json(&serde_json::to_vec(&json).unwrap()).is_err());
    }
}
