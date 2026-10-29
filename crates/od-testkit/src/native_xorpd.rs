//! Deterministic generated inputs for the bounded native XORPD profile.

use crate::HexU64;
use crate::case::{MemoryPermissions, MemoryRegionSpec, Provenance};
use crate::sha256_hex;
use crate::v2::{
    ExpectedExit, Fixture, IntegerState, Mode, PROFILE_NATIVE_XORPD_XMM0_XMM1, Profile,
    SCHEMA_VERSION, SseSnapshot,
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
            fs_base: None,
            gs_base: None,
            fs_selector: None,
            gs_selector: None,
            ds_selector: None,
            es_selector: None,
            ds_unusable: None,
            es_unusable: None,
            gdtr_base: None,
            gdtr_limit: None,
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
        },
        xorpd_prerequisites: None,
        minss_invalid_controls: None,
        fxsave64_cr0_controls: None,
        fxrstor64_cr0_controls: None,
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
