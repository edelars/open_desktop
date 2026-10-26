use crate::case::{CpuProfile, ExpectedExit};
use crate::{
    FormatError, HexU64, MAX_MEMORY_BYTES, MAX_MEMORY_REGIONS, MAX_RESULT_JSON_BYTES, sha256_hex,
};
use serde::{Deserialize, Serialize};

pub const RESULT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Interpreter,
    Jit,
    JitIrBlock,
    NativeX86,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionStatus {
    Completed,
    Unsupported,
    TimedOut,
    Signaled,
    HarnessError,
    InvalidInput,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitReason {
    ReachedCodeEnd,
    InstructionBudgetExhausted,
    StopRequested,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Observation<T> {
    Available { value: T },
    Unavailable { reason: String },
}

impl<T> Observation<T> {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GprSnapshot {
    pub rax: HexU64,
    pub rcx: HexU64,
    pub rdx: HexU64,
    pub rbx: HexU64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRegionSnapshot {
    pub region_id: String,
    pub guest_address: HexU64,
    pub bytes_hex: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExceptionSnapshot {
    pub vector: u8,
    pub error_code: Observation<HexU64>,
    pub faulting_rip: HexU64,
    pub fault_address: Observation<HexU64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CpuSnapshot {
    pub gpr: Observation<GprSnapshot>,
    pub rip: Observation<HexU64>,
    pub rflags: Observation<HexU64>,
    pub memory: Observation<Vec<MemoryRegionSnapshot>>,
    pub exception: Observation<ExceptionSnapshot>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildMetadata {
    pub revision: String,
    pub target: String,
    pub profile: String,
    pub compiler: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MachineMetadata {
    pub host_os: String,
    pub host_arch: String,
    pub cpu_model: Option<String>,
    pub kernel: Option<String>,
    pub page_size: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunResult {
    pub schema_version: u32,
    pub case_id: String,
    /// SHA-256 of the exact fixture JSON input bytes.
    pub input_sha256: String,
    pub profile: CpuProfile,
    pub backend: Backend,
    pub status: CompletionStatus,
    pub expected_exit: ExpectedExit,
    pub exit_reason: Observation<ExitReason>,
    pub instructions_executed: Observation<HexU64>,
    pub snapshot: CpuSnapshot,
    pub build: BuildMetadata,
    pub machine: MachineMetadata,
}

impl RunResult {
    pub fn from_json(bytes: &[u8]) -> Result<Self, FormatError> {
        if bytes.len() > MAX_RESULT_JSON_BYTES {
            return Err(FormatError::TooLarge {
                kind: "result JSON",
                max_bytes: MAX_RESULT_JSON_BYTES,
            });
        }
        let result: Self =
            serde_json::from_slice(bytes).map_err(|error| FormatError::Json(error.to_string()))?;
        result.validate()?;
        Ok(result)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, FormatError> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| FormatError::Json(error.to_string()))?;
        if bytes.len() > MAX_RESULT_JSON_BYTES {
            return Err(FormatError::TooLarge {
                kind: "result JSON",
                max_bytes: MAX_RESULT_JSON_BYTES,
            });
        }
        Ok(bytes)
    }

    pub fn validate(&self) -> Result<(), FormatError> {
        if self.schema_version != RESULT_SCHEMA_VERSION {
            return Err(FormatError::UnsupportedSchema(self.schema_version));
        }
        if self.case_id.is_empty()
            || self.case_id.len() > 128
            || !self
                .case_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err(invalid("case_id", "invalid identifier"));
        }
        if !is_sha256(&self.input_sha256) {
            return Err(invalid("input_sha256", "must be 64 lowercase hex digits"));
        }
        for (field, value) in [
            ("build.revision", self.build.revision.as_str()),
            ("build.target", self.build.target.as_str()),
            ("build.profile", self.build.profile.as_str()),
            ("build.compiler", self.build.compiler.as_str()),
            ("machine.host_os", self.machine.host_os.as_str()),
            ("machine.host_arch", self.machine.host_arch.as_str()),
        ] {
            if value.trim().is_empty() || value.len() > 512 {
                return Err(invalid(field, "must be nonempty and at most 512 bytes"));
            }
        }
        if matches!(self.status, CompletionStatus::Completed)
            && (!self.snapshot.gpr.is_available()
                || !self.snapshot.rip.is_available()
                || !self.snapshot.rflags.is_available()
                || !self.exit_reason.is_available()
                || (self.backend != Backend::NativeX86
                    && !self.instructions_executed.is_available()))
        {
            return Err(invalid(
                "snapshot",
                "completed runs require observed GPR, RIP, RFLAGS and exit reason; non-native runs also require instruction count",
            ));
        }
        if let Observation::Available { value } = &self.snapshot.memory {
            let mut total = 0usize;
            let mut ids = std::collections::HashSet::new();
            if value.len() > MAX_MEMORY_REGIONS {
                return Err(FormatError::TooLarge {
                    kind: "memory region count",
                    max_bytes: MAX_MEMORY_REGIONS,
                });
            }
            for region in value {
                if region.region_id.trim().is_empty()
                    || region.region_id.len() > 128
                    || !region
                        .region_id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
                {
                    return Err(invalid("snapshot.memory.region_id", "invalid identifier"));
                }
                if !ids.insert(region.region_id.as_str()) {
                    return Err(invalid(
                        "snapshot.memory.region_id",
                        "contains a duplicate ID",
                    ));
                }
                validate_bytes_hex(&region.bytes_hex)?;
                total = total.saturating_add(region.bytes_hex.len() / 2);
            }
            if total > MAX_MEMORY_BYTES {
                return Err(FormatError::TooLarge {
                    kind: "snapshot memory",
                    max_bytes: MAX_MEMORY_BYTES,
                });
            }
        }
        if let Observation::Unavailable { reason } = &self.snapshot.gpr {
            validate_reason(reason, "snapshot.gpr")?;
        }
        if let Observation::Unavailable { reason } = &self.snapshot.rip {
            validate_reason(reason, "snapshot.rip")?;
        }
        if let Observation::Unavailable { reason } = &self.snapshot.rflags {
            validate_reason(reason, "snapshot.rflags")?;
        }
        if let Observation::Unavailable { reason } = &self.snapshot.memory {
            validate_reason(reason, "snapshot.memory")?;
        }
        if let Observation::Unavailable { reason } = &self.snapshot.exception {
            validate_reason(reason, "snapshot.exception")?;
        }
        if let Observation::Unavailable { reason } = &self.exit_reason {
            validate_reason(reason, "exit_reason")?;
        }
        if let Observation::Unavailable { reason } = &self.instructions_executed {
            validate_reason(reason, "instructions_executed")?;
        }
        if let Observation::Available { value } = &self.snapshot.exception {
            if value.vector > 31 {
                return Err(invalid(
                    "snapshot.exception.vector",
                    "must be an x86 exception vector",
                ));
            }
            if let Observation::Unavailable { reason } = &value.error_code {
                validate_reason(reason, "snapshot.exception.error_code")?;
            }
            if let Observation::Unavailable { reason } = &value.fault_address {
                validate_reason(reason, "snapshot.exception.fault_address")?;
            }
        }
        if let Some(page_size) = self.machine.page_size
            && page_size == 0
        {
            return Err(invalid(
                "machine.page_size",
                "must be nonzero when available",
            ));
        }
        Ok(())
    }

    pub fn validate_for_case(
        &self,
        case_id: &str,
        input_bytes: &[u8],
        profile: CpuProfile,
    ) -> Result<(), FormatError> {
        if self.case_id != case_id {
            return Err(FormatError::CaseMismatch("case_id"));
        }
        if self.input_sha256 != sha256_hex(input_bytes) {
            return Err(FormatError::CaseMismatch("input_sha256"));
        }
        if self.profile != profile {
            return Err(FormatError::CaseMismatch("profile"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result() -> RunResult {
        RunResult {
            schema_version: RESULT_SCHEMA_VERSION,
            case_id: "case-1".to_string(),
            input_sha256: "a".repeat(64),
            profile: CpuProfile::S0_09RegisterSubsetV1,
            backend: Backend::Interpreter,
            status: CompletionStatus::Completed,
            expected_exit: ExpectedExit::ReachedCodeEnd,
            exit_reason: Observation::Available {
                value: ExitReason::ReachedCodeEnd,
            },
            instructions_executed: Observation::Available { value: HexU64(1) },
            snapshot: CpuSnapshot {
                gpr: Observation::Available {
                    value: GprSnapshot {
                        rax: HexU64(1),
                        rcx: HexU64(0),
                        rdx: HexU64(0),
                        rbx: HexU64(0),
                    },
                },
                rip: Observation::Available { value: HexU64(3) },
                rflags: Observation::Available { value: HexU64(2) },
                memory: Observation::Unavailable {
                    reason: "not supported by this backend".to_string(),
                },
                exception: Observation::Unavailable {
                    reason: "not supported by this backend".to_string(),
                },
            },
            build: BuildMetadata {
                revision: "working-tree".to_string(),
                target: "aarch64-apple-darwin".to_string(),
                profile: "debug".to_string(),
                compiler: "rustc test".to_string(),
            },
            machine: MachineMetadata {
                host_os: "macos".to_string(),
                host_arch: "aarch64".to_string(),
                cpu_model: None,
                kernel: None,
                page_size: None,
            },
        }
    }

    #[test]
    fn result_round_trip_keeps_unavailable_distinct_from_zero() {
        let mut expected = result();
        expected.status = CompletionStatus::TimedOut;
        expected.snapshot.gpr = Observation::Unavailable {
            reason: "child timed out".to_string(),
        };
        expected.snapshot.rip = Observation::Unavailable {
            reason: "child timed out".to_string(),
        };
        expected.snapshot.rflags = Observation::Unavailable {
            reason: "child timed out".to_string(),
        };
        expected.exit_reason = Observation::Unavailable {
            reason: "child timed out".to_string(),
        };
        expected.instructions_executed = Observation::Unavailable {
            reason: "child timed out".to_string(),
        };
        let bytes = expected.to_json().unwrap();
        let actual = RunResult::from_json(&bytes).unwrap();
        assert_eq!(actual, expected);
        assert_ne!(
            actual.snapshot.rip,
            Observation::Available { value: HexU64(0) }
        );
    }

    #[test]
    fn ir_block_backend_uses_a_distinct_stable_result_tag() {
        let mut value = result();
        value.backend = Backend::JitIrBlock;
        let bytes = value.to_json().unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("\"backend\": \"jit_ir_block\""));
        assert_eq!(RunResult::from_json(&bytes).unwrap(), value);
    }

    #[test]
    fn completed_result_requires_the_core_snapshot() {
        let mut value = result();
        value.snapshot.rip = Observation::Unavailable {
            reason: "not exposed".to_string(),
        };
        assert!(value.validate().is_err());
    }

    #[test]
    fn observations_reject_unknown_and_conflicting_fields() {
        for (field, extra) in [("rip", "reason"), ("memory", "value")] {
            let mut value = serde_json::to_value(result()).unwrap();
            value["snapshot"][field][extra] = serde_json::json!("hidden data");
            assert!(RunResult::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
        }
    }

    #[test]
    fn completed_native_result_may_leave_hardware_instruction_count_unavailable() {
        let mut result = result();
        result.backend = Backend::NativeX86;
        result.instructions_executed = Observation::Unavailable {
            reason: "native retired instruction count is not sampled".to_string(),
        };
        assert!(result.to_json().is_ok());
    }

    #[test]
    fn result_memory_is_bounded() {
        let mut value = result();
        value.snapshot.memory = Observation::Available {
            value: (0..=MAX_MEMORY_REGIONS)
                .map(|_| MemoryRegionSnapshot {
                    region_id: "r".to_string(),
                    guest_address: HexU64(0),
                    bytes_hex: String::new(),
                })
                .collect(),
        };
        assert!(matches!(
            value.validate(),
            Err(FormatError::TooLarge {
                kind: "memory region count",
                ..
            })
        ));
    }

    #[test]
    fn result_must_match_exact_case_input() {
        let value = result();
        assert!(
            value
                .validate_for_case(
                    "case-1",
                    b"exact fixture",
                    CpuProfile::S0_09RegisterSubsetV1
                )
                .is_err()
        );
    }
}

fn validate_bytes_hex(value: &str) -> Result<(), FormatError> {
    if value.len() % 2 != 0
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(
            "snapshot.memory.bytes_hex",
            "must contain contiguous lowercase hex byte pairs",
        ));
    }
    Ok(())
}

fn validate_reason(reason: &str, field: &'static str) -> Result<(), FormatError> {
    if reason.trim().is_empty() || reason.len() > 512 {
        return Err(invalid(
            field,
            "unavailable reason must be nonempty and at most 512 bytes",
        ));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn invalid(field: &'static str, reason: impl Into<String>) -> FormatError {
    FormatError::InvalidField {
        field,
        reason: reason.into(),
    }
}
