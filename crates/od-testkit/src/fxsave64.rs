//! Independent raw evidence and byte comparator for the successful CPL3
//! `FXSAVE64 [RAX+0x40]` oracle. This module does not execute or serialize CPU state.

use serde::{Deserialize, Serialize};

pub const EVIDENCE_VERSION: u32 = 1;
pub const INSTRUCTION: [u8; 5] = [0x48, 0x0f, 0xae, 0x40, 0x40];
pub const REPETITION_ORDER: [Fxsave64Case; 4] = [
    Fxsave64Case::A,
    Fxsave64Case::B,
    Fxsave64Case::A,
    Fxsave64Case::B,
];
pub const COMPARISON_RANGES: [&str; 9] = [
    "[0,5): raw/excluded x87 control/status/tag",
    "[5,8): raw/excluded reserved header",
    "[8,24): raw/excluded x87 pointer fields",
    "[24,28): compare MXCSR exact bytes",
    "[28,32): raw-only MXCSR_MASK",
    "[32,160): raw/excluded x87 payload",
    "[160,416): compare XMM0-XMM15 exact bytes",
    "[416,464): raw/excluded reserved/unused bytes",
    "[464,512): compare software tail against initial canary",
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fxsave64Case {
    A,
    B,
}

impl Fxsave64Case {
    pub const fn mxcsr(self) -> [u8; 4] {
        match self {
            Self::A => [0x80, 0x1f, 0x00, 0x00],
            Self::B => [0x80, 0x7f, 0x00, 0x00],
        }
    }

    pub const fn image_canary(self) -> u8 {
        match self {
            Self::A => 0xa5,
            Self::B => 0x5a,
        }
    }

    pub const fn left_guard(self) -> u8 {
        match self {
            Self::A => 0x3c,
            Self::B => 0xc3,
        }
    }

    pub const fn right_guard(self) -> u8 {
        match self {
            Self::A => 0xc3,
            Self::B => 0x3c,
        }
    }

    /// Independent protocol literals, in increasing memory address order.
    pub const fn xmm(self, register: usize) -> [u8; 16] {
        let mut bytes = [0; 16];
        let mut lane = 0;
        while lane < 16 {
            let value = register * 16 + lane;
            bytes[lane] = match self {
                Self::A => value as u8,
                Self::B => 255 - value as u8,
            };
            lane += 1;
        }
        bytes
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fxsave64Hashes {
    pub harness_source_sha256: String,
    pub literal_table_sha256: String,
    pub emitted_bytes_sha256: String,
    pub executable_sha256: String,
    pub comparator_sha256: String,
    pub protocol_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fxsave64Environment {
    pub cpu_vendor: String,
    pub cpu_family: u32,
    pub cpu_model: u32,
    pub cpu_stepping: u32,
    pub cpuid_fxsr: bool,
    pub cpuid_sse: bool,
    pub cpuid_leaf1_eax: u32,
    pub cpuid_leaf1_edx: u32,
    pub os: String,
    pub kernel: String,
    pub virtualization: String,
    pub execution_kind: String,
    pub affinity_cpu: u32,
    pub toolchain: String,
    pub build_flags: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fxsave64Run {
    pub case: Fxsave64Case,
    pub repetition: u8,
    pub code_address: u64,
    pub input_address: u64,
    pub pre_observation_address: u64,
    pub post_observation_address: u64,
    pub image_address: u64,
    pub left_guard_address: u64,
    pub right_guard_address: u64,
    pub rax: u64,
    pub code_permissions: String,
    pub input_permissions: String,
    pub image_permissions: String,
    pub guard_permissions: String,
    pub entry_rip: u64,
    pub raw_stop_rip: Option<u64>,
    pub stop_rip_adjustment: u8,
    pub observed_rip: Option<u64>,
    pub completion_marker: bool,
    pub exit_status: Option<i32>,
    pub signal: Option<i32>,
    pub signal_code: Option<i32>,
    pub initial_image: Vec<u8>,
    pub image: Vec<u8>,
    pub left_guard_before: Vec<u8>,
    pub left_guard_after: Vec<u8>,
    pub right_guard_before: Vec<u8>,
    pub right_guard_after: Vec<u8>,
    pub xmm_before: Vec<u8>,
    pub xmm_after: Vec<u8>,
    pub mxcsr_before: Vec<u8>,
    pub mxcsr_after: Vec<u8>,
    pub mxcsr_mask_raw: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fxsave64Evidence {
    pub schema_version: u32,
    pub status: String,
    pub source_revision: String,
    pub hashes: Fxsave64Hashes,
    pub environment: Fxsave64Environment,
    pub instruction_bytes: Vec<u8>,
    pub instruction_length: u8,
    pub emitted_code_address: u64,
    pub emitted_bytes: Vec<u8>,
    pub test_instruction_offset: usize,
    pub comparison_ranges: Vec<String>,
    pub stop_mechanism: String,
    pub retirement_counter_claimed: bool,
    pub runs: Vec<Fxsave64Run>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fxsave64Comparison {
    pub passed: bool,
    pub errors: Vec<String>,
}

/// Validate raw evidence and compare only the protocol-defined image slices.
pub fn compare_fxsave64(evidence: &Fxsave64Evidence) -> Fxsave64Comparison {
    let mut errors = Vec::new();
    if evidence.schema_version != EVIDENCE_VERSION {
        errors.push("unsupported evidence schema version".into());
    }
    if evidence.status != "not_run" && evidence.status != "observed" {
        errors.push("status must be not_run or observed".into());
    }
    if evidence.instruction_bytes != INSTRUCTION || evidence.instruction_length != 5 {
        errors.push("instruction bytes/length do not match the exact S2-39d form".into());
    }
    if evidence
        .test_instruction_offset
        .checked_add(6)
        .is_none_or(|end| {
            end > evidence.emitted_bytes.len()
                || evidence.emitted_bytes[evidence.test_instruction_offset..end]
                    != [INSTRUCTION.as_slice(), &[0xcc]].concat()
        })
    {
        errors
            .push("raw emitted code does not contain exact FXSAVE64 bytes followed by INT3".into());
    }
    if evidence.emitted_code_address == 0
        || evidence.emitted_bytes.is_empty()
        || crate::sha256_hex(&evidence.emitted_bytes) != evidence.hashes.emitted_bytes_sha256
    {
        errors.push("raw emitted code address/bytes/hash are missing or inconsistent".into());
    }
    if evidence.retirement_counter_claimed {
        errors.push("retirement-counter claims are outside this protocol".into());
    }
    if evidence
        .comparison_ranges
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        != COMPARISON_RANGES
    {
        errors.push("comparison/excluded byte ranges differ from the protocol".into());
    }
    if evidence.runs.len() != REPETITION_ORDER.len() {
        errors.push("exactly four A/B/A/B repetitions are required".into());
    }
    if evidence.stop_mechanism.trim().is_empty() {
        errors.push("stop mechanism must be recorded".into());
    }
    for (name, digest) in [
        ("harness source", &evidence.hashes.harness_source_sha256),
        ("literal table", &evidence.hashes.literal_table_sha256),
        ("emitted bytes", &evidence.hashes.emitted_bytes_sha256),
        ("executable", &evidence.hashes.executable_sha256),
        ("comparator", &evidence.hashes.comparator_sha256),
        ("protocol", &evidence.hashes.protocol_sha256),
    ] {
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            errors.push(format!("{name} SHA-256 must be 64 hexadecimal characters"));
        }
    }
    if evidence.source_revision.trim().is_empty()
        || evidence.environment.cpu_vendor.trim().is_empty()
        || evidence.environment.os.trim().is_empty()
        || evidence.environment.kernel.trim().is_empty()
        || evidence.environment.toolchain.trim().is_empty()
        || evidence.environment.build_flags.trim().is_empty()
        || evidence.environment.execution_kind.trim().is_empty()
        || evidence.environment.virtualization.trim().is_empty()
    {
        errors.push(
            "source, CPU, OS, virtualization, execution and toolchain metadata are required".into(),
        );
    }
    for (index, run) in evidence.runs.iter().enumerate() {
        let prefix = format!("run {index}");
        if REPETITION_ORDER.get(index) != Some(&run.case) {
            errors.push(format!("{prefix}: repetition order must be A/B/A/B"));
        }
        if run.repetition != index as u8 {
            errors.push(format!("{prefix}: repetition index is inconsistent"));
        }
        if run.signal.is_some() {
            errors.push(format!("{prefix}: unexpected signal"));
        }
        if let Some(code) = run.signal_code {
            if ![0, 1, 0x80].contains(&code) {
                errors.push(format!("{prefix}: unsupported SIGTRAP source code"));
            }
        } else {
            errors.push(format!("{prefix}: marker SIGTRAP source code is missing"));
        }
        if run.exit_status != Some(0) {
            errors.push(format!("{prefix}: unexpected or missing exit status"));
        }
        if !run.completion_marker
            || run.raw_stop_rip != run.entry_rip.checked_add(6)
            || run.stop_rip_adjustment != 1
            || run.observed_rip != run.entry_rip.checked_add(5)
        {
            errors.push(format!(
                "{prefix}: missing INT3 completion stop or wrong raw/adjusted E+5 RIP"
            ));
        }
        if run.code_address != run.entry_rip {
            errors.push(format!("{prefix}: code address and entry RIP differ"));
        }
        if evidence
            .emitted_code_address
            .checked_add(evidence.test_instruction_offset as u64)
            != Some(run.code_address)
        {
            errors.push(format!(
                "{prefix}: test entry is inconsistent with emitted-code address"
            ));
        }
        if run.image_address & 15 != 0 || run.rax.checked_add(0x40) != Some(run.image_address) {
            errors.push(format!(
                "{prefix}: image is misaligned or RAX does not target D"
            ));
        }
        if !canonical48(run.image_address)
            || !canonical48(run.image_address.saturating_add(543))
            || !canonical48(run.code_address)
            || !canonical48(run.input_address)
        {
            errors.push(format!(
                "{prefix}: code/input/image span is not canonical48"
            ));
        }
        if run.input_address == 0
            || run.pre_observation_address == 0
            || run.post_observation_address == 0
            || run.pre_observation_address == run.post_observation_address
        {
            errors.push(format!(
                "{prefix}: independent input/observation addresses are missing or aliased"
            ));
        }
        let image_range = run.image_address..run.image_address.saturating_add(512);
        let other_ranges = [
            run.input_address..run.input_address.saturating_add(256),
            run.pre_observation_address..run.pre_observation_address.saturating_add(256),
            run.post_observation_address..run.post_observation_address.saturating_add(256),
        ];
        if other_ranges
            .iter()
            .any(|range| range.start < image_range.end && image_range.start < range.end)
        {
            errors.push(format!(
                "{prefix}: input/observations overlap the saved image"
            ));
        }
        if run.left_guard_address.checked_add(32) != Some(run.image_address)
            || run.image_address.checked_add(512) != Some(run.right_guard_address)
        {
            errors.push(format!(
                "{prefix}: guard addresses do not bracket the 512-byte image"
            ));
        }
        if !run.code_permissions.starts_with("r-x")
            || !run.input_permissions.starts_with("rw-")
            || !run.image_permissions.starts_with("rw-")
            || !run.guard_permissions.starts_with("rw-")
        {
            errors.push(format!(
                "{prefix}: mapping permissions are missing or unexpected"
            ));
        }
        check_len(
            &mut errors,
            &prefix,
            "initial image",
            &run.initial_image,
            512,
        );
        check_len(&mut errors, &prefix, "image", &run.image, 512);
        check_len(
            &mut errors,
            &prefix,
            "left guard before",
            &run.left_guard_before,
            32,
        );
        check_len(
            &mut errors,
            &prefix,
            "left guard after",
            &run.left_guard_after,
            32,
        );
        check_len(
            &mut errors,
            &prefix,
            "right guard before",
            &run.right_guard_before,
            32,
        );
        check_len(
            &mut errors,
            &prefix,
            "right guard after",
            &run.right_guard_after,
            32,
        );
        check_len(
            &mut errors,
            &prefix,
            "pre XMM observations",
            &run.xmm_before,
            256,
        );
        check_len(
            &mut errors,
            &prefix,
            "post XMM observations",
            &run.xmm_after,
            256,
        );
        check_len(
            &mut errors,
            &prefix,
            "pre MXCSR observation",
            &run.mxcsr_before,
            4,
        );
        check_len(
            &mut errors,
            &prefix,
            "post MXCSR observation",
            &run.mxcsr_after,
            4,
        );
        check_len(
            &mut errors,
            &prefix,
            "raw MXCSR_MASK",
            &run.mxcsr_mask_raw,
            4,
        );

        let case = run.case;
        if run.initial_image.len() == 512 {
            if run
                .initial_image
                .iter()
                .any(|byte| *byte != case.image_canary())
            {
                errors.push(format!("{prefix}: initial image is not the case canary"));
            }
        }
        if run.left_guard_before.len() == 32
            && run
                .left_guard_before
                .iter()
                .any(|byte| *byte != case.left_guard())
        {
            errors.push(format!("{prefix}: left guard prefill is wrong"));
        }
        if run.right_guard_before.len() == 32
            && run
                .right_guard_before
                .iter()
                .any(|byte| *byte != case.right_guard())
        {
            errors.push(format!("{prefix}: right guard prefill is wrong"));
        }
        if run.left_guard_before != run.left_guard_after
            || run.right_guard_before != run.right_guard_after
        {
            errors.push(format!("{prefix}: guard corruption"));
        }
        if run.xmm_before.len() == 256 && run.xmm_after.len() == 256 {
            for register in 0..16 {
                let expected = case.xmm(register);
                let range = register * 16..(register + 1) * 16;
                if run.xmm_before[range.clone()] != expected {
                    errors.push(format!("{prefix}: pre-observation XMM{register} mismatch"));
                }
                if run.xmm_after[range] != expected {
                    errors.push(format!("{prefix}: post-observation XMM{register} mismatch"));
                }
            }
        }
        if run.mxcsr_before.as_slice() != case.mxcsr() || run.mxcsr_after.as_slice() != case.mxcsr()
        {
            errors.push(format!("{prefix}: independent MXCSR observation mismatch"));
        }
        if run.image.len() == 512 {
            if run.image[24..28] != case.mxcsr() {
                errors.push(format!("{prefix}: compared MXCSR image bytes mismatch"));
            }
            for register in 0..16 {
                let start = 160 + register * 16;
                if run.image[start..start + 16] != case.xmm(register) {
                    errors.push(format!(
                        "{prefix}: compared XMM{register} image bytes mismatch"
                    ));
                }
            }
            if run.initial_image.len() == 512 && run.image[464..512] != run.initial_image[464..512]
            {
                errors.push(format!("{prefix}: software tail [464,512) changed"));
            }
        }
        // MXCSR_MASK is deliberately only required to be captured as four raw bytes.
    }
    Fxsave64Comparison {
        passed: errors.is_empty(),
        errors,
    }
}

fn check_len(errors: &mut Vec<String>, prefix: &str, label: &str, bytes: &[u8], expected: usize) {
    if bytes.len() != expected {
        errors.push(format!(
            "{prefix}: {label} must contain {expected} raw bytes"
        ));
    }
}

fn canonical48(address: u64) -> bool {
    let sign = (address >> 47) & 1;
    let upper = address >> 48;
    upper == if sign == 0 { 0 } else { 0xffff }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_evidence() -> Fxsave64Evidence {
        let runs = REPETITION_ORDER
            .iter()
            .enumerate()
            .map(|(index, case)| {
                let mut image = vec![case.image_canary(); 512];
                image[24..28].copy_from_slice(&case.mxcsr());
                for register in 0..16 {
                    let start = 160 + register * 16;
                    image[start..start + 16].copy_from_slice(&case.xmm(register));
                }
                let xmm: Vec<u8> = (0..16).flat_map(|register| case.xmm(register)).collect();
                let d = 0x1000_1000 + index as u64 * 0x1000;
                Fxsave64Run {
                    case: *case,
                    repetition: index as u8,
                    code_address: 0x400000,
                    input_address: 0x500000,
                    pre_observation_address: 0x600000,
                    post_observation_address: 0x700000,
                    image_address: d,
                    left_guard_address: d - 32,
                    right_guard_address: d + 512,
                    rax: d - 0x40,
                    code_permissions: "r-xp".into(),
                    input_permissions: "rw-s".into(),
                    image_permissions: "rw-s".into(),
                    guard_permissions: "rw-s".into(),
                    entry_rip: 0x400000,
                    raw_stop_rip: Some(0x400006),
                    stop_rip_adjustment: 1,
                    observed_rip: Some(0x400005),
                    completion_marker: true,
                    exit_status: Some(0),
                    signal: None,
                    signal_code: Some(1),
                    initial_image: vec![case.image_canary(); 512],
                    image,
                    left_guard_before: vec![case.left_guard(); 32],
                    left_guard_after: vec![case.left_guard(); 32],
                    right_guard_before: vec![case.right_guard(); 32],
                    right_guard_after: vec![case.right_guard(); 32],
                    xmm_before: xmm.clone(),
                    xmm_after: xmm,
                    mxcsr_before: case.mxcsr().to_vec(),
                    mxcsr_after: case.mxcsr().to_vec(),
                    mxcsr_mask_raw: vec![0; 4],
                }
            })
            .collect();
        let emitted_bytes = [vec![0x90; 0x100], INSTRUCTION.to_vec(), vec![0xcc]].concat();
        let emitted_digest = crate::sha256_hex(&emitted_bytes);
        Fxsave64Evidence {
            schema_version: EVIDENCE_VERSION,
            status: "observed".into(),
            source_revision: "fixture".into(),
            hashes: Fxsave64Hashes {
                harness_source_sha256: "0".repeat(64),
                literal_table_sha256: "0".repeat(64),
                emitted_bytes_sha256: emitted_digest,
                executable_sha256: "0".repeat(64),
                comparator_sha256: "0".repeat(64),
                protocol_sha256: "0".repeat(64),
            },
            environment: Fxsave64Environment {
                cpu_vendor: "test".into(),
                cpu_family: 1,
                cpu_model: 1,
                cpu_stepping: 1,
                cpuid_fxsr: true,
                cpuid_sse: true,
                cpuid_leaf1_eax: 0,
                cpuid_leaf1_edx: 0,
                os: "test".into(),
                kernel: "test".into(),
                virtualization: "unknown".into(),
                execution_kind: "physical".into(),
                affinity_cpu: 0,
                toolchain: "test".into(),
                build_flags: "test".into(),
            },
            instruction_bytes: INSTRUCTION.to_vec(),
            instruction_length: 5,
            emitted_code_address: 0x3fff00,
            emitted_bytes,
            test_instruction_offset: 0x100,
            comparison_ranges: COMPARISON_RANGES
                .iter()
                .map(|range| (*range).into())
                .collect(),
            stop_mechanism: "post-instruction marker".into(),
            retirement_counter_claimed: false,
            runs,
        }
    }

    #[test]
    fn accepts_defined_slices_and_raw_non_target_mask() {
        let evidence = valid_evidence();
        assert!(compare_fxsave64(&evidence).passed);
    }

    #[test]
    fn rejects_wrong_instruction_and_missing_repetitions() {
        let mut evidence = valid_evidence();
        evidence.instruction_bytes[0] = 0x90;
        evidence.runs.pop();
        let comparison = compare_fxsave64(&evidence);
        assert!(
            comparison
                .errors
                .iter()
                .any(|error| error.contains("instruction"))
        );
        assert!(
            comparison
                .errors
                .iter()
                .any(|error| error.contains("four A/B/A/B"))
        );
    }

    #[test]
    fn rejects_changed_compared_bytes_and_tail_but_ignores_reserved_holes() {
        let mut evidence = valid_evidence();
        evidence.runs[0].image[416] ^= 0xff;
        assert!(compare_fxsave64(&evidence).passed);
        evidence.runs[0].image[24] ^= 1;
        assert!(!compare_fxsave64(&evidence).passed);
    }

    #[test]
    fn rejects_guard_corruption_signal_and_wrong_stop_rip() {
        let mut evidence = valid_evidence();
        evidence.runs[1].left_guard_after[0] ^= 1;
        evidence.runs[1].signal = Some(11);
        evidence.runs[1].observed_rip = Some(0x400004);
        let errors = compare_fxsave64(&evidence).errors.join(" ");
        assert!(errors.contains("guard corruption"));
        assert!(errors.contains("unexpected signal"));
        assert!(errors.contains("wrong raw/adjusted E+5 RIP"));
    }

    #[test]
    fn rejects_malformed_observations() {
        let mut evidence = valid_evidence();
        evidence.runs[0].xmm_after.pop();
        evidence.runs[0].mxcsr_mask_raw.clear();
        let errors = compare_fxsave64(&evidence).errors.join(" ");
        assert!(errors.contains("post XMM observations"));
        assert!(errors.contains("raw MXCSR_MASK"));
    }
}
