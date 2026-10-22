use crate::case::{
    ExceptionComparisonScope, ExpectedExit, Fixture, LoadedFixture, MemoryComparisonScope,
    MemoryRegionSpec, Register,
};
use crate::result::{
    Backend, CompletionStatus, CpuSnapshot, ExceptionSnapshot, MemoryRegionSnapshot, Observation,
    RESULT_SCHEMA_VERSION, RunResult,
};
use crate::{FormatError, HexU64};
use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum ResultSide {
    Left,
    Right,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompareError {
    InvalidFixture(FormatError),
    InvalidResult {
        side: ResultSide,
        error: FormatError,
    },
    CaseMismatch(&'static str),
    Incomplete {
        side: ResultSide,
        status: CompletionStatus,
    },
    MissingObservation {
        side: ResultSide,
        field: &'static str,
    },
}

impl std::fmt::Display for CompareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFixture(error) => write!(f, "invalid fixture: {error}"),
            Self::InvalidResult { side, error } => {
                write!(f, "invalid {side:?} result: {error}")
            }
            Self::CaseMismatch(field) => write!(f, "results are incompatible at {field}"),
            Self::Incomplete { side, status } => {
                write!(f, "{side:?} result is not complete ({status:?})")
            }
            Self::MissingObservation { side, field } => {
                write!(f, "{side:?} result lacks required observation {field}")
            }
        }
    }
}

impl std::error::Error for CompareError {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Difference {
    pub field: String,
    pub mask: Option<HexU64>,
    pub expected: String,
    pub actual: String,
    pub context: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ComparisonReport {
    pub case_id: String,
    pub input_sha256: String,
    pub left_backend: Backend,
    pub right_backend: Backend,
    pub differences: Vec<Difference>,
}

impl ComparisonReport {
    pub fn matches(&self) -> bool {
        self.differences.is_empty()
    }
}

/// Compare two completed backend results using only the fixture's declared scope.
pub fn compare_results(
    fixture: &LoadedFixture,
    left: &RunResult,
    right: &RunResult,
) -> Result<ComparisonReport, CompareError> {
    fixture
        .fixture
        .validate()
        .map_err(CompareError::InvalidFixture)?;
    validate_result(left, ResultSide::Left)?;
    validate_result(right, ResultSide::Right)?;
    validate_result_case(&fixture.fixture, &fixture.input_sha256, left)?;
    validate_result_case(&fixture.fixture, &fixture.input_sha256, right)?;
    if left.status != CompletionStatus::Completed {
        return Err(CompareError::Incomplete {
            side: ResultSide::Left,
            status: left.status,
        });
    }
    if right.status != CompletionStatus::Completed {
        return Err(CompareError::Incomplete {
            side: ResultSide::Right,
            status: right.status,
        });
    }

    let context = format!(
        "case={} profile={} left={:?} right={:?}",
        fixture.fixture.case_id,
        fixture.fixture.profile.as_str(),
        left.backend,
        right.backend
    );
    let mut differences = Vec::new();
    compare_exit_reason(
        fixture.fixture.expected_exit,
        left,
        right,
        &context,
        &mut differences,
    )?;
    compare_instruction_count(left, right, &context, &mut differences)?;
    compare_gpr(&fixture.fixture, left, right, &context, &mut differences)?;
    compare_rip(&fixture.fixture, left, right, &context, &mut differences)?;
    compare_flags(&fixture.fixture, left, right, &context, &mut differences)?;

    match fixture.fixture.comparison.memory {
        MemoryComparisonScope::Unsupported => {}
        MemoryComparisonScope::Compare { ref regions } => {
            for region in regions {
                compare_memory_region(
                    region,
                    &left.snapshot,
                    &right.snapshot,
                    &context,
                    &mut differences,
                )?;
            }
        }
    }
    if fixture.fixture.comparison.exceptions == ExceptionComparisonScope::Compare {
        compare_exception(&left.snapshot, &right.snapshot, &context, &mut differences)?;
    }

    Ok(ComparisonReport {
        case_id: fixture.fixture.case_id.clone(),
        input_sha256: fixture.input_sha256.clone(),
        left_backend: left.backend,
        right_backend: right.backend,
        differences,
    })
}

fn validate_result(result: &RunResult, side: ResultSide) -> Result<(), CompareError> {
    if result.schema_version != RESULT_SCHEMA_VERSION {
        return Err(CompareError::InvalidResult {
            side,
            error: FormatError::UnsupportedSchema(result.schema_version),
        });
    }
    result
        .validate()
        .map_err(|error| CompareError::InvalidResult { side, error })
}

fn validate_result_case(
    fixture: &Fixture,
    input_sha256: &str,
    result: &RunResult,
) -> Result<(), CompareError> {
    if result.case_id != fixture.case_id {
        return Err(CompareError::CaseMismatch("case_id"));
    }
    if result.input_sha256 != input_sha256 {
        return Err(CompareError::CaseMismatch("input_sha256"));
    }
    if result.profile != fixture.profile {
        return Err(CompareError::CaseMismatch("profile"));
    }
    if result.expected_exit != fixture.expected_exit {
        return Err(CompareError::CaseMismatch("expected_exit"));
    }
    Ok(())
}

fn compare_exit_reason(
    expected: ExpectedExit,
    left: &RunResult,
    right: &RunResult,
    context: &str,
    output: &mut Vec<Difference>,
) -> Result<(), CompareError> {
    let expected_reason = match expected {
        ExpectedExit::ReachedCodeEnd => crate::result::ExitReason::ReachedCodeEnd,
        ExpectedExit::InstructionBudgetExhausted => {
            crate::result::ExitReason::InstructionBudgetExhausted
        }
    };
    let left_value = available(&left.exit_reason, ResultSide::Left, "exit_reason")?;
    let right_value = available(&right.exit_reason, ResultSide::Right, "exit_reason")?;
    if left_value != &expected_reason {
        push_difference(
            output,
            "exit_reason.left",
            None,
            format!("{expected_reason:?}"),
            format!("{left_value:?}"),
            context,
        );
    }
    if right_value != &expected_reason {
        push_difference(
            output,
            "exit_reason.right",
            None,
            format!("{expected_reason:?}"),
            format!("{right_value:?}"),
            context,
        );
    }
    if left_value != right_value {
        push_difference(
            output,
            "exit_reason",
            None,
            format!("{left_value:?}"),
            format!("{right_value:?}"),
            context,
        );
    }
    Ok(())
}

fn compare_instruction_count(
    left: &RunResult,
    right: &RunResult,
    context: &str,
    output: &mut Vec<Difference>,
) -> Result<(), CompareError> {
    if let (
        Observation::Available { value: left_value },
        Observation::Available { value: right_value },
    ) = (&left.instructions_executed, &right.instructions_executed)
    {
        if left_value != right_value {
            push_difference(
                output,
                "instructions_executed",
                None,
                format_hex(left_value.get()),
                format_hex(right_value.get()),
                context,
            );
        }
    }
    Ok(())
}

fn compare_gpr(
    fixture: &Fixture,
    left: &RunResult,
    right: &RunResult,
    context: &str,
    output: &mut Vec<Difference>,
) -> Result<(), CompareError> {
    let left_gpr = available(&left.snapshot.gpr, ResultSide::Left, "snapshot.gpr")?;
    let right_gpr = available(&right.snapshot.gpr, ResultSide::Right, "snapshot.gpr")?;
    for register in &fixture.comparison.registers {
        let left_value = register_value(left_gpr, *register);
        let right_value = register_value(right_gpr, *register);
        if left_value != right_value {
            push_difference(
                output,
                &format!("gpr.{}", register_name(*register)),
                None,
                format_hex(left_value),
                format_hex(right_value),
                context,
            );
        }
    }
    Ok(())
}

fn compare_rip(
    fixture: &Fixture,
    left: &RunResult,
    right: &RunResult,
    context: &str,
    output: &mut Vec<Difference>,
) -> Result<(), CompareError> {
    if !fixture.comparison.rip {
        return Ok(());
    }
    let left_value = available(&left.snapshot.rip, ResultSide::Left, "snapshot.rip")?;
    let right_value = available(&right.snapshot.rip, ResultSide::Right, "snapshot.rip")?;
    if left_value != right_value {
        push_difference(
            output,
            "rip",
            None,
            format_hex(left_value.get()),
            format_hex(right_value.get()),
            context,
        );
    }
    Ok(())
}

fn compare_flags(
    fixture: &Fixture,
    left: &RunResult,
    right: &RunResult,
    context: &str,
    output: &mut Vec<Difference>,
) -> Result<(), CompareError> {
    let mask = fixture.comparison.rflags_mask.get();
    if mask == 0 {
        return Ok(());
    }
    let left_value = available(&left.snapshot.rflags, ResultSide::Left, "snapshot.rflags")?.get();
    let right_value =
        available(&right.snapshot.rflags, ResultSide::Right, "snapshot.rflags")?.get();
    let masked_left = left_value & mask;
    let masked_right = right_value & mask;
    if masked_left != masked_right {
        push_difference(
            output,
            "rflags",
            Some(HexU64(mask)),
            format_hex(masked_left),
            format_hex(masked_right),
            context,
        );
    }
    Ok(())
}

fn compare_memory_region(
    expected: &MemoryRegionSpec,
    left_snapshot: &CpuSnapshot,
    right_snapshot: &CpuSnapshot,
    context: &str,
    output: &mut Vec<Difference>,
) -> Result<(), CompareError> {
    let left_regions = available(&left_snapshot.memory, ResultSide::Left, "snapshot.memory")?;
    let right_regions = available(&right_snapshot.memory, ResultSide::Right, "snapshot.memory")?;
    let left_region = find_region(left_regions, &expected.region_id, ResultSide::Left)?;
    let right_region = find_region(right_regions, &expected.region_id, ResultSide::Right)?;
    if left_region.guest_address != right_region.guest_address {
        push_difference(
            output,
            &format!("memory.{}.guest_address", expected.region_id),
            None,
            format_hex(left_region.guest_address.get()),
            format_hex(right_region.guest_address.get()),
            context,
        );
    }
    let expected_address = expected.guest_address.get();
    for (side, region) in [
        (ResultSide::Left, left_region),
        (ResultSide::Right, right_region),
    ] {
        if region.guest_address.get() != expected_address {
            return Err(CompareError::MissingObservation {
                side,
                field: "snapshot.memory.expected_guest_address",
            });
        }
    }

    let left_bytes = decode_hex_bytes(&left_region.bytes_hex, ResultSide::Left)?;
    let right_bytes = decode_hex_bytes(&right_region.bytes_hex, ResultSide::Right)?;
    if left_bytes.len() != expected.length as usize || right_bytes.len() != expected.length as usize
    {
        push_difference(
            output,
            &format!("memory.{}.length", expected.region_id),
            None,
            left_bytes.len().to_string(),
            right_bytes.len().to_string(),
            context,
        );
        return Ok(());
    }
    let masks = decode_hex_bytes(&expected.comparison_mask_hex, ResultSide::Left)?;
    for ((index, (left_byte, right_byte)), mask) in
        left_bytes.iter().zip(&right_bytes).enumerate().zip(masks)
    {
        let masked_left = left_byte & mask;
        let masked_right = right_byte & mask;
        if masked_left != masked_right {
            push_difference(
                output,
                &format!("memory.{}[{}]", expected.region_id, index),
                Some(HexU64(u64::from(mask))),
                format!("0x{masked_left:02x}"),
                format!("0x{masked_right:02x}"),
                context,
            );
        }
    }
    Ok(())
}

fn compare_exception(
    left_snapshot: &CpuSnapshot,
    right_snapshot: &CpuSnapshot,
    context: &str,
    output: &mut Vec<Difference>,
) -> Result<(), CompareError> {
    let left = available(
        &left_snapshot.exception,
        ResultSide::Left,
        "snapshot.exception",
    )?;
    let right = available(
        &right_snapshot.exception,
        ResultSide::Right,
        "snapshot.exception",
    )?;
    compare_exception_snapshots(left, right, context, output)
}

fn compare_exception_snapshots(
    left: &ExceptionSnapshot,
    right: &ExceptionSnapshot,
    context: &str,
    output: &mut Vec<Difference>,
) -> Result<(), CompareError> {
    compare_scalar(
        "exception.vector",
        u64::from(left.vector),
        u64::from(right.vector),
        None,
        context,
        output,
    );
    compare_scalar(
        "exception.faulting_rip",
        left.faulting_rip.get(),
        right.faulting_rip.get(),
        None,
        context,
        output,
    );
    compare_observed_scalar(
        "exception.error_code",
        &left.error_code,
        &right.error_code,
        ResultSide::Left,
        ResultSide::Right,
        context,
        output,
    )?;
    compare_observed_scalar(
        "exception.fault_address",
        &left.fault_address,
        &right.fault_address,
        ResultSide::Left,
        ResultSide::Right,
        context,
        output,
    )
}

fn compare_observed_scalar(
    field: &'static str,
    left: &Observation<HexU64>,
    right: &Observation<HexU64>,
    left_side: ResultSide,
    right_side: ResultSide,
    context: &str,
    output: &mut Vec<Difference>,
) -> Result<(), CompareError> {
    let left_value = available(left, left_side, field)?;
    let right_value = available(right, right_side, field)?;
    compare_scalar(
        field,
        left_value.get(),
        right_value.get(),
        None,
        context,
        output,
    );
    Ok(())
}

fn compare_scalar(
    field: &str,
    left: u64,
    right: u64,
    mask: Option<HexU64>,
    context: &str,
    output: &mut Vec<Difference>,
) {
    if left != right {
        push_difference(
            output,
            field,
            mask,
            format_hex(left),
            format_hex(right),
            context,
        );
    }
}

fn find_region<'a>(
    regions: &'a [MemoryRegionSnapshot],
    region_id: &str,
    side: ResultSide,
) -> Result<&'a MemoryRegionSnapshot, CompareError> {
    regions
        .iter()
        .find(|region| region.region_id == region_id)
        .ok_or(CompareError::MissingObservation {
            side,
            field: "snapshot.memory.region",
        })
}

fn decode_hex_bytes(value: &str, side: ResultSide) -> Result<Vec<u8>, CompareError> {
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16).map_err(|_| {
                CompareError::MissingObservation {
                    side,
                    field: "snapshot.memory.bytes_hex",
                }
            })
        })
        .collect()
}

fn register_value(snapshot: &crate::result::GprSnapshot, register: Register) -> u64 {
    match register {
        Register::Rax => snapshot.rax.get(),
        Register::Rcx => snapshot.rcx.get(),
        Register::Rdx => snapshot.rdx.get(),
        Register::Rbx => snapshot.rbx.get(),
    }
}

fn register_name(register: Register) -> &'static str {
    match register {
        Register::Rax => "rax",
        Register::Rcx => "rcx",
        Register::Rdx => "rdx",
        Register::Rbx => "rbx",
    }
}

fn available<'a, T>(
    observation: &'a Observation<T>,
    side: ResultSide,
    field: &'static str,
) -> Result<&'a T, CompareError> {
    match observation {
        Observation::Available { value } => Ok(value),
        Observation::Unavailable { .. } => Err(CompareError::MissingObservation { side, field }),
    }
}

fn format_hex(value: u64) -> String {
    format!("0x{value:016x}")
}

fn push_difference(
    output: &mut Vec<Difference>,
    field: &str,
    mask: Option<HexU64>,
    expected: String,
    actual: String,
    context: &str,
) {
    output.push(Difference {
        field: field.to_string(),
        mask,
        expected,
        actual,
        context: context.to_string(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::case::{
        ComparisonScope, CpuProfile, ExceptionComparisonScope, ExpectedExit, InitialState,
        MemoryPermissions, Program, Provenance,
    };
    use crate::result::{
        BuildMetadata, CpuSnapshot, GprSnapshot, MachineMetadata, MemoryRegionSnapshot,
    };
    use crate::{HexU64, sha256_hex};

    fn loaded_fixture() -> LoadedFixture {
        let code = [0x48, 0xB8, 1, 0, 0, 0, 0, 0, 0, 0];
        LoadedFixture {
            fixture: Fixture {
                schema_version: crate::case::FIXTURE_SCHEMA_VERSION,
                case_id: "case-1".to_string(),
                profile: CpuProfile::S0_09RegisterSubsetV1,
                provenance: Provenance {
                    kind: "project_fixture".to_string(),
                    source: "tests/case.hex".to_string(),
                    specification: "docs/specs/CPU_CONTRACT.md".to_string(),
                    specification_version: "v0.1".to_string(),
                },
                seed: None,
                program: Program {
                    bytes_hex: "48b80100000000000000".to_string(),
                    sha256: sha256_hex(&code),
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
                    rflags_mask: HexU64(0x8d5),
                    memory: MemoryComparisonScope::Unsupported,
                    exceptions: ExceptionComparisonScope::Unsupported,
                },
                expected_exit: ExpectedExit::ReachedCodeEnd,
            },
            input_sha256: "a".repeat(64),
        }
    }

    fn result(backend: Backend) -> RunResult {
        RunResult {
            schema_version: RESULT_SCHEMA_VERSION,
            case_id: "case-1".to_string(),
            input_sha256: "a".repeat(64),
            profile: CpuProfile::S0_09RegisterSubsetV1,
            backend,
            status: CompletionStatus::Completed,
            expected_exit: ExpectedExit::ReachedCodeEnd,
            exit_reason: Observation::Available {
                value: crate::result::ExitReason::ReachedCodeEnd,
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
                rip: Observation::Available { value: HexU64(10) },
                rflags: Observation::Available { value: HexU64(2) },
                memory: Observation::Unavailable {
                    reason: "not exposed by this backend".to_string(),
                },
                exception: Observation::Unavailable {
                    reason: "not exposed by this backend".to_string(),
                },
            },
            build: BuildMetadata {
                revision: "test".to_string(),
                target: "test-target".to_string(),
                profile: "test".to_string(),
                compiler: "test compiler".to_string(),
            },
            machine: MachineMetadata {
                host_os: "test-os".to_string(),
                host_arch: "test-arch".to_string(),
                cpu_model: None,
                kernel: None,
                page_size: None,
            },
        }
    }

    #[test]
    fn compares_only_declared_registers_and_flags() {
        let fixture = loaded_fixture();
        let left = result(Backend::Interpreter);
        let mut right = result(Backend::Jit);
        right.snapshot.rflags = Observation::Available {
            value: HexU64(0xffff_ffff_ffff_0202),
        };
        assert!(compare_results(&fixture, &left, &right).unwrap().matches());

        right.snapshot.gpr = Observation::Available {
            value: GprSnapshot {
                rax: HexU64(2),
                rcx: HexU64(0),
                rdx: HexU64(0),
                rbx: HexU64(0),
            },
        };
        let report = compare_results(&fixture, &left, &right).unwrap();
        assert_eq!(report.differences[0].field, "gpr.rax");
        assert_eq!(report.differences[0].expected, "0x0000000000000001");
        assert_eq!(report.differences[0].actual, "0x0000000000000002");
        assert!(report.differences[0].context.contains("case=case-1"));
    }

    #[test]
    fn compares_native_snapshot_when_hardware_instruction_count_is_unavailable() {
        let fixture = loaded_fixture();
        let left = result(Backend::Interpreter);
        let mut native = result(Backend::NativeX86);
        native.instructions_executed = Observation::Unavailable {
            reason: "native retired instruction count is not sampled".to_string(),
        };
        assert!(compare_results(&fixture, &left, &native).unwrap().matches());
    }

    #[test]
    fn rejects_two_backends_that_agree_on_the_wrong_expected_exit() {
        let fixture = loaded_fixture();
        let mut left = result(Backend::Interpreter);
        let mut right = result(Backend::Jit);
        left.exit_reason = Observation::Available {
            value: crate::result::ExitReason::InstructionBudgetExhausted,
        };
        right.exit_reason = left.exit_reason.clone();

        let report = compare_results(&fixture, &left, &right).unwrap();
        assert!(!report.matches());
        assert_eq!(report.differences[0].field, "exit_reason.left");
        assert_eq!(report.differences[0].expected, "ReachedCodeEnd");
        assert_eq!(report.differences[0].actual, "InstructionBudgetExhausted");
    }

    #[test]
    fn rejects_different_digest_profile_or_incomplete_results_before_state_compare() {
        let fixture = loaded_fixture();
        let left = result(Backend::Interpreter);
        let mut wrong_input = result(Backend::Jit);
        wrong_input.input_sha256 = "b".repeat(64);
        assert_eq!(
            compare_results(&fixture, &left, &wrong_input),
            Err(CompareError::CaseMismatch("input_sha256"))
        );

        let mut incomplete = result(Backend::NativeX86);
        incomplete.status = CompletionStatus::TimedOut;
        assert!(matches!(
            compare_results(&fixture, &left, &incomplete),
            Err(CompareError::Incomplete { .. })
        ));
    }

    #[test]
    fn absent_required_state_is_an_error_not_a_match() {
        let fixture = loaded_fixture();
        let left = result(Backend::Interpreter);
        let mut right = result(Backend::NativeX86);
        right.snapshot.rip = Observation::Unavailable {
            reason: "wrapper did not capture RIP".to_string(),
        };
        assert!(matches!(
            compare_results(&fixture, &left, &right),
            Err(CompareError::InvalidResult {
                side: ResultSide::Right,
                ..
            })
        ));
    }

    #[test]
    fn compares_memory_bytes_using_the_fixture_mask() {
        let region = MemoryRegionSpec {
            region_id: "data".to_string(),
            guest_address: HexU64(0x1000),
            length: 2,
            permissions: MemoryPermissions {
                read: true,
                write: true,
                execute: false,
            },
            initial_bytes_hex: "0000".to_string(),
            comparison_mask_hex: "ff0f".to_string(),
        };
        MemoryComparisonScope::Compare {
            regions: vec![region.clone()],
        }
        .validate()
        .unwrap();
        let left = vec![MemoryRegionSnapshot {
            region_id: "data".to_string(),
            guest_address: HexU64(0x1000),
            bytes_hex: "1201".to_string(),
        }];
        let right = vec![MemoryRegionSnapshot {
            region_id: "data".to_string(),
            guest_address: HexU64(0x1000),
            bytes_hex: "120f".to_string(),
        }];
        let mut differences = Vec::new();
        compare_memory_region(
            &region,
            &CpuSnapshot {
                memory: Observation::Available { value: left },
                ..result(Backend::Interpreter).snapshot
            },
            &CpuSnapshot {
                memory: Observation::Available { value: right },
                ..result(Backend::NativeX86).snapshot
            },
            "memory-test",
            &mut differences,
        )
        .unwrap();
        assert_eq!(differences.len(), 1);
        assert_eq!(differences[0].field, "memory.data[1]");
        assert_eq!(differences[0].mask, Some(HexU64(15)));
    }

    #[test]
    fn compares_exception_fault_metadata_when_the_adapter_exposes_it() {
        let left = ExceptionSnapshot {
            vector: 14,
            error_code: Observation::Available { value: HexU64(3) },
            faulting_rip: HexU64(0x1234),
            fault_address: Observation::Available {
                value: HexU64(0xdead),
            },
        };
        let mut right = left.clone();
        right.fault_address = Observation::Available {
            value: HexU64(0xbeef),
        };
        let mut differences = Vec::new();
        compare_exception_snapshots(&left, &right, "fault-case", &mut differences).unwrap();
        assert_eq!(differences.len(), 1);
        assert_eq!(differences[0].field, "exception.fault_address");
    }
}
