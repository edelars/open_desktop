use crate::{
    FormatError, HexU64, MAX_JSON_BYTES, MAX_MEMORY_BYTES, MAX_MEMORY_REGIONS, MAX_PROGRAM_BYTES,
    sha256_hex,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const FIXTURE_SCHEMA_VERSION: u32 = 1;
pub const REGISTER_PROFILE: &str = "s0_09_register_subset_v1";
pub const MAX_INSTRUCTION_BUDGET: u64 = 10_000_000;
pub const MAX_WALL_TIMEOUT_MS: u32 = 30_000;
const MODELED_FLAGS: u64 = (1 << 0) | (1 << 2) | (1 << 4) | (1 << 6) | (1 << 7) | (1 << 11);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuProfile {
    S0_09RegisterSubsetV1,
}

impl CpuProfile {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::S0_09RegisterSubsetV1 => REGISTER_PROFILE,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Register {
    Rax,
    Rcx,
    Rdx,
    Rbx,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub kind: String,
    pub source: String,
    pub specification: String,
    pub specification_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Program {
    /// Contiguous lowercase hexadecimal, two digits per byte, without separators.
    pub bytes_hex: String,
    /// SHA-256 over the decoded program bytes, independent of JSON whitespace.
    pub sha256: String,
}

impl Program {
    pub fn bytes(&self) -> Result<Vec<u8>, FormatError> {
        decode_program(&self.bytes_hex)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InitialState {
    pub rax: HexU64,
    pub rcx: HexU64,
    pub rdx: HexU64,
    pub rbx: HexU64,
    /// Byte offset from the first byte of the program.
    pub rip: HexU64,
    pub rflags: HexU64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ComparisonScope {
    pub registers: Vec<Register>,
    pub rip: bool,
    /// Only bits explicitly set here participate in comparison.
    pub rflags_mask: HexU64,
    pub memory: MemoryComparisonScope,
    pub exceptions: ExceptionComparisonScope,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemoryComparisonScope {
    Unsupported,
    Compare { regions: Vec<MemoryRegionSpec> },
}

impl<'de> Deserialize<'de> for MemoryComparisonScope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Tagged unit variants ignore extra fields in serde. Empty struct
        // variants enforce the same strict contract as the populated variant.
        #[derive(Deserialize)]
        #[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire {
            Unsupported {},
            Compare { regions: Vec<MemoryRegionSpec> },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Unsupported {} => Self::Unsupported,
            Wire::Compare { regions } => Self::Compare { regions },
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryPermissions {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRegionSpec {
    pub region_id: String,
    pub guest_address: HexU64,
    pub length: u32,
    pub permissions: MemoryPermissions,
    pub initial_bytes_hex: String,
    /// One byte mask per region byte; only set bits are compared.
    pub comparison_mask_hex: String,
}

impl MemoryComparisonScope {
    pub fn validate(&self) -> Result<(), FormatError> {
        let Self::Compare { regions } = self else {
            return Ok(());
        };
        if regions.len() > MAX_MEMORY_REGIONS {
            return Err(FormatError::TooLarge {
                kind: "fixture memory region count",
                max_bytes: MAX_MEMORY_REGIONS,
            });
        }
        let mut ids = HashSet::new();
        let mut ranges = Vec::with_capacity(regions.len());
        let mut total = 0usize;
        for region in regions {
            validate_case_id(&region.region_id).map_err(|_| {
                invalid(
                    "comparison.memory.regions.region_id",
                    "must use case-ID characters",
                )
            })?;
            if !ids.insert(region.region_id.as_str()) {
                return Err(invalid(
                    "comparison.memory.regions.region_id",
                    "contains a duplicate region ID",
                ));
            }
            if region.length == 0 {
                return Err(invalid(
                    "comparison.memory.regions.length",
                    "must be nonzero",
                ));
            }
            if region.length as usize > MAX_MEMORY_BYTES {
                return Err(FormatError::TooLarge {
                    kind: "fixture memory region",
                    max_bytes: MAX_MEMORY_BYTES,
                });
            }
            let start = region.guest_address.get();
            let end = start.checked_add(u64::from(region.length)).ok_or_else(|| {
                invalid("comparison.memory.regions.guest_address", "range overflows")
            })?;
            if ranges
                .iter()
                .any(|(other_start, other_end)| start < *other_end && *other_start < end)
            {
                return Err(invalid(
                    "comparison.memory.regions.guest_address",
                    "regions overlap",
                ));
            }
            ranges.push((start, end));
            let expected_hex_len = region.length as usize * 2;
            if region.initial_bytes_hex.len() != expected_hex_len {
                return Err(invalid(
                    "comparison.memory.regions.initial_bytes_hex",
                    "length must match region length",
                ));
            }
            validate_hex_bytes(
                &region.initial_bytes_hex,
                "comparison.memory.regions.initial_bytes_hex",
            )?;
            if region.comparison_mask_hex.len() != expected_hex_len {
                return Err(invalid(
                    "comparison.memory.regions.comparison_mask_hex",
                    "must contain one mask byte per region byte",
                ));
            }
            validate_hex_bytes(
                &region.comparison_mask_hex,
                "comparison.memory.regions.comparison_mask_hex",
            )?;
            total = total.saturating_add(region.length as usize);
            if total > MAX_MEMORY_BYTES {
                return Err(FormatError::TooLarge {
                    kind: "fixture memory",
                    max_bytes: MAX_MEMORY_BYTES,
                });
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExceptionComparisonScope {
    Unsupported,
    Compare,
}

impl<'de> Deserialize<'de> for ExceptionComparisonScope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire {
            Unsupported {},
            Compare {},
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Unsupported {} => Self::Unsupported,
            Wire::Compare {} => Self::Compare,
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedExit {
    ReachedCodeEnd,
    InstructionBudgetExhausted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub schema_version: u32,
    pub case_id: String,
    pub profile: CpuProfile,
    pub provenance: Provenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    pub program: Program,
    pub initial_state: InitialState,
    pub instruction_budget: u64,
    pub wall_timeout_ms: u32,
    pub comparison: ComparisonScope,
    pub expected_exit: ExpectedExit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedFixture {
    pub fixture: Fixture,
    /// SHA-256 of the exact JSON bytes loaded, including its original whitespace.
    pub input_sha256: String,
}

impl Fixture {
    pub fn from_json(bytes: &[u8]) -> Result<LoadedFixture, FormatError> {
        if bytes.len() > MAX_JSON_BYTES {
            return Err(FormatError::TooLarge {
                kind: "fixture JSON",
                max_bytes: MAX_JSON_BYTES,
            });
        }
        let fixture: Self =
            serde_json::from_slice(bytes).map_err(|error| FormatError::Json(error.to_string()))?;
        fixture.validate()?;
        Ok(LoadedFixture {
            fixture,
            input_sha256: sha256_hex(bytes),
        })
    }

    pub fn to_json(&self) -> Result<Vec<u8>, FormatError> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| FormatError::Json(error.to_string()))?;
        if bytes.len() > MAX_JSON_BYTES {
            return Err(FormatError::TooLarge {
                kind: "fixture JSON",
                max_bytes: MAX_JSON_BYTES,
            });
        }
        Ok(bytes)
    }

    pub fn validate(&self) -> Result<(), FormatError> {
        if self.schema_version != FIXTURE_SCHEMA_VERSION {
            return Err(FormatError::UnsupportedSchema(self.schema_version));
        }
        validate_case_id(&self.case_id)?;
        if self.profile != CpuProfile::S0_09RegisterSubsetV1 {
            return Err(invalid("profile", "unsupported CPU profile"));
        }
        for (name, value) in [
            ("provenance.kind", self.provenance.kind.as_str()),
            ("provenance.source", self.provenance.source.as_str()),
            (
                "provenance.specification",
                self.provenance.specification.as_str(),
            ),
            (
                "provenance.specification_version",
                self.provenance.specification_version.as_str(),
            ),
        ] {
            if value.trim().is_empty() || value.len() > 512 {
                return Err(invalid(name, "must be nonempty and at most 512 bytes"));
            }
        }

        let program = self.program.bytes()?;
        if program.len() > MAX_PROGRAM_BYTES {
            return Err(FormatError::TooLarge {
                kind: "program",
                max_bytes: MAX_PROGRAM_BYTES,
            });
        }
        if self.program.sha256.len() != 64
            || !self
                .program
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid("program.sha256", "must be 64 lowercase hex digits"));
        }
        if sha256_hex(&program) != self.program.sha256 {
            return Err(FormatError::DigestMismatch);
        }

        let rip = usize::try_from(self.initial_state.rip.get())
            .map_err(|_| invalid("initial_state.rip", "does not fit the host usize"))?;
        if program.is_empty() || rip >= program.len() {
            return Err(invalid(
                "initial_state.rip",
                "must point to a byte inside a nonempty program",
            ));
        }
        if self.initial_state.rflags.get() & (1 << 1) == 0 {
            return Err(invalid(
                "initial_state.rflags",
                "reserved bit 1 must be set in this profile",
            ));
        }
        if self.initial_state.rflags.get() & MODELED_FLAGS != 0 {
            return Err(invalid(
                "initial_state.rflags",
                "arithmetic flags must start clear in this profile",
            ));
        }
        if !(1..=MAX_INSTRUCTION_BUDGET).contains(&self.instruction_budget) {
            return Err(invalid(
                "instruction_budget",
                "must be between 1 and 10000000",
            ));
        }
        if !(1..=MAX_WALL_TIMEOUT_MS).contains(&self.wall_timeout_ms) {
            return Err(invalid("wall_timeout_ms", "must be between 1 and 30000"));
        }
        if self.comparison.registers.is_empty() {
            return Err(invalid("comparison.registers", "must not be empty"));
        }
        let registers: HashSet<_> = self.comparison.registers.iter().copied().collect();
        if registers.len() != self.comparison.registers.len() {
            return Err(invalid("comparison.registers", "contains duplicates"));
        }
        if !self.comparison.rip {
            return Err(invalid("comparison.rip", "RIP must be compared"));
        }
        if self.comparison.rflags_mask.get() & !MODELED_FLAGS != 0 {
            return Err(invalid(
                "comparison.rflags_mask",
                "contains flags outside CF/PF/AF/ZF/SF/OF",
            ));
        }
        self.comparison.memory.validate()?;
        if self.comparison.memory != MemoryComparisonScope::Unsupported {
            return Err(invalid("comparison.memory", "unsupported by this profile"));
        }
        if self.comparison.exceptions != ExceptionComparisonScope::Unsupported {
            return Err(invalid(
                "comparison.exceptions",
                "unsupported by this profile",
            ));
        }
        Ok(())
    }
}

impl LoadedFixture {
    pub fn to_json(&self) -> Result<Vec<u8>, FormatError> {
        self.fixture.to_json()
    }
}

fn decode_program(hex: &str) -> Result<Vec<u8>, FormatError> {
    if hex.len() % 2 != 0
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(
            "program.bytes_hex",
            "must contain contiguous lowercase hex byte pairs",
        ));
    }
    if hex.len() / 2 > MAX_PROGRAM_BYTES {
        return Err(FormatError::TooLarge {
            kind: "program",
            max_bytes: MAX_PROGRAM_BYTES,
        });
    }
    (0..hex.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&hex[index..index + 2], 16).map_err(|error| {
                FormatError::InvalidField {
                    field: "program.bytes_hex",
                    reason: error.to_string(),
                }
            })
        })
        .collect()
}

fn validate_hex_bytes(value: &str, field: &'static str) -> Result<(), FormatError> {
    if value.len() % 2 != 0
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(
            field,
            "must contain contiguous lowercase hex byte pairs",
        ));
    }
    Ok(())
}

fn validate_case_id(case_id: &str) -> Result<(), FormatError> {
    if case_id.is_empty()
        || case_id.len() > 128
        || !case_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(invalid(
            "case_id",
            "must contain 1–128 ASCII letters, digits, '_' or '-'",
        ));
    }
    Ok(())
}

fn invalid(field: &'static str, reason: impl Into<String>) -> FormatError {
    FormatError::InvalidField {
        field,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Fixture {
        let bytes = [0x48, 0xB8, 1, 0, 0, 0, 0, 0, 0, 0];
        Fixture {
            schema_version: FIXTURE_SCHEMA_VERSION,
            case_id: "mov-basic".to_string(),
            profile: CpuProfile::S0_09RegisterSubsetV1,
            provenance: Provenance {
                kind: "project_fixture".to_string(),
                source: "tests/fixtures/x86/mov.hex".to_string(),
                specification: "docs/specs/CPU_CONTRACT.md".to_string(),
                specification_version: "v0.1; Intel SDM 093".to_string(),
            },
            seed: None,
            program: Program {
                bytes_hex: "48b80100000000000000".to_string(),
                sha256: sha256_hex(&bytes),
            },
            initial_state: InitialState {
                rax: HexU64(0),
                rcx: HexU64(0),
                rdx: HexU64(0),
                rbx: HexU64(0),
                rip: HexU64(0),
                rflags: HexU64(2),
            },
            instruction_budget: 8,
            wall_timeout_ms: 1000,
            comparison: ComparisonScope {
                registers: vec![Register::Rax, Register::Rcx, Register::Rdx, Register::Rbx],
                rip: true,
                rflags_mask: HexU64(MODELED_FLAGS),
                memory: MemoryComparisonScope::Unsupported,
                exceptions: ExceptionComparisonScope::Unsupported,
            },
            expected_exit: ExpectedExit::ReachedCodeEnd,
        }
    }

    #[test]
    fn fixture_json_round_trip_preserves_architectural_u64_values() {
        let fixture = fixture();
        let bytes = fixture.to_json().unwrap();
        assert!(
            std::str::from_utf8(&bytes)
                .unwrap()
                .contains("\"rax\": \"0x0000000000000000\"")
        );
        let loaded = Fixture::from_json(&bytes).unwrap();
        assert_eq!(loaded.fixture, fixture);
        assert_eq!(loaded.input_sha256, sha256_hex(&bytes));
    }

    #[test]
    fn project_cases_match_their_preserved_hex_fixtures() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86");
        for name in [
            "s0_09_add_overflow",
            "s0_09_cmp_jz",
            "s0_09_mov_imm64",
            "s0_09_sub_jnz_loop",
        ] {
            let json = std::fs::read(root.join(format!("{name}.case.json"))).unwrap();
            let hex = std::fs::read_to_string(root.join(format!("{name}.hex"))).unwrap();
            let loaded = Fixture::from_json(&json).unwrap();
            let expected = hex
                .split_whitespace()
                .map(|byte| u8::from_str_radix(byte, 16).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(loaded.fixture.case_id, name);
            assert_eq!(loaded.fixture.program.bytes().unwrap(), expected);
            assert_eq!(loaded.input_sha256, sha256_hex(&json));
        }
    }

    #[test]
    fn rejects_unknown_schema_before_execution() {
        let mut fixture = fixture();
        fixture.schema_version = 2;
        assert!(matches!(
            fixture.validate(),
            Err(FormatError::UnsupportedSchema(2))
        ));
    }

    #[test]
    fn rejects_oversized_json_before_parsing() {
        let bytes = vec![b' '; MAX_JSON_BYTES + 1];
        assert!(matches!(
            Fixture::from_json(&bytes),
            Err(FormatError::TooLarge {
                kind: "fixture JSON",
                ..
            })
        ));
    }

    #[test]
    fn rejects_program_digest_mismatch_and_invalid_offset() {
        let mut bad_digest = fixture();
        bad_digest.program.sha256.replace_range(..2, "00");
        assert_eq!(bad_digest.validate(), Err(FormatError::DigestMismatch));

        let mut fixture = fixture();
        fixture.initial_state.rip = HexU64(10);
        assert!(matches!(
            fixture.validate(),
            Err(FormatError::InvalidField {
                field: "initial_state.rip",
                ..
            })
        ));
    }

    #[test]
    fn rejects_memory_or_exception_claims_for_register_only_profile() {
        let mut value: serde_json::Value =
            serde_json::from_slice(&fixture().to_json().unwrap()).unwrap();
        value["comparison"]["memory"]["status"] = serde_json::json!("available");
        assert!(Fixture::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn unsupported_scopes_reject_hidden_fields() {
        for field in ["memory", "exceptions"] {
            let mut value = serde_json::to_value(fixture()).unwrap();
            value["comparison"][field]["regions"] = serde_json::json!([]);
            assert!(Fixture::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
        }
    }
}
