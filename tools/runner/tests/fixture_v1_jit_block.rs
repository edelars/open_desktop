//! S3-13 end-to-end CLI tests for one bounded IR block.

use od_testkit::HexU64;
use od_testkit::case::{ExpectedExit, Fixture};
use od_testkit::result::{Backend, CompletionStatus, ExitReason, Observation, RunResult};
use std::fs;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

const MOV_FIXTURE: &[u8] = include_bytes!("../../../tests/fixtures/x86/s0_09_mov_imm64.case.json");
const V2_FIXTURE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json");

struct TempCase(std::path::PathBuf);

impl TempCase {
    fn new(bytes: &[u8]) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "od-runner-s3-13-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("case.json"), bytes).unwrap();
        Self(dir)
    }

    fn path(&self) -> std::path::PathBuf {
        self.0.join("case.json")
    }
}

impl Drop for TempCase {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixture(case_id: &str, code: &[u8], budget: u64, expected: ExpectedExit) -> Vec<u8> {
    let mut fixture = Fixture::from_json(MOV_FIXTURE).unwrap().fixture;
    fixture.case_id = case_id.to_string();
    fixture.program.bytes_hex = code.iter().map(|byte| format!("{byte:02x}")).collect();
    fixture.program.sha256 = od_testkit::sha256_hex(code);
    fixture.instruction_budget = budget;
    fixture.expected_exit = expected;
    fixture.initial_state.rax = HexU64(0);
    fixture.initial_state.rcx = HexU64(0);
    fixture.initial_state.rdx = HexU64(0);
    fixture.initial_state.rbx = HexU64(0);
    fixture.initial_state.rip = HexU64(0);
    fixture.initial_state.rflags = HexU64(2);
    fixture.to_json().unwrap()
}

fn run(bytes: &[u8], engine: &str) -> Output {
    let case = TempCase::new(bytes);
    Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .arg("--fixture")
        .arg(case.path())
        .arg("--engine")
        .arg(engine)
        .arg("--json")
        .output()
        .unwrap()
}

fn result(output: &Output, exit: i32) -> RunResult {
    assert_eq!(
        output.status.code(),
        Some(exit),
        "stderr: {}; stdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    RunResult::from_json(&output.stdout).unwrap()
}

#[test]
fn jit_block_child_executes_one_mov_and_legacy_jit_remains_available() {
    let code = [0x48, 0xB8, 0x78, 0x56, 0x34, 0x12, 0, 0, 0, 0];
    let bytes = fixture("s313_mov_end", &code, 8, ExpectedExit::ReachedCodeEnd);
    let native = run(&bytes, "jit-block");
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        let jit_result = result(&native, 0);
        assert_eq!(jit_result.backend, Backend::JitIrBlock);
        assert_eq!(jit_result.status, CompletionStatus::Completed);
        assert_eq!(
            jit_result.exit_reason,
            Observation::Available {
                value: ExitReason::ReachedCodeEnd
            }
        );
        assert_eq!(
            jit_result.instructions_executed,
            Observation::Available { value: HexU64(1) }
        );
        assert_eq!(
            jit_result.snapshot.gpr,
            Observation::Available {
                value: od_testkit::result::GprSnapshot {
                    rax: HexU64(0x1234_5678),
                    rcx: HexU64(0),
                    rdx: HexU64(0),
                    rbx: HexU64(0),
                }
            }
        );
        assert!(String::from_utf8_lossy(&native.stdout).contains("\"backend\": \"jit_ir_block\""));

        let legacy = result(&run(&bytes, "jit"), 0);
        assert_eq!(legacy.backend, Backend::Jit);
        assert_eq!(legacy.status, CompletionStatus::Completed);
    } else {
        let result = result(&native, 4);
        assert_eq!(result.backend, Backend::JitIrBlock);
        assert_eq!(result.status, CompletionStatus::Unsupported);
    }
}

#[test]
fn jit_block_child_reports_budget_and_terminal_branch_without_dispatch() {
    let mut movs = Vec::new();
    for (opcode, immediate) in [(0xB8, 0x1111_u64), (0xB9, 0x2222), (0xBA, 0x3333)] {
        movs.extend_from_slice(&[0x48, opcode]);
        movs.extend_from_slice(&immediate.to_le_bytes());
    }
    let budget_case = fixture(
        "s313_budget",
        &movs,
        1,
        ExpectedExit::InstructionBudgetExhausted,
    );
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        let budget = result(&run(&budget_case, "jit-block"), 0);
        assert_eq!(budget.backend, Backend::JitIrBlock);
        assert_eq!(budget.status, CompletionStatus::Completed);
        assert_eq!(
            budget.exit_reason,
            Observation::Available {
                value: ExitReason::InstructionBudgetExhausted
            }
        );
        assert_eq!(
            budget.instructions_executed,
            Observation::Available { value: HexU64(1) }
        );

        let terminal = [0x48, 0x03, 0xC1, 0x74, 0x00]; // ADD; JZ to code_end
        let terminal_case = fixture(
            "s313_terminal_jz",
            &terminal,
            8,
            ExpectedExit::ReachedCodeEnd,
        );
        let branch = result(&run(&terminal_case, "jit-block"), 0);
        assert_eq!(branch.backend, Backend::JitIrBlock);
        assert_eq!(branch.status, CompletionStatus::Completed);
        assert_eq!(
            branch.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        assert_eq!(
            branch.snapshot.rip,
            Observation::Available { value: HexU64(5) }
        );

        let mut not_taken_fixture = Fixture::from_json(&terminal_case).unwrap().fixture;
        not_taken_fixture.case_id = "s313_terminal_jz_not_taken".into();
        not_taken_fixture.initial_state.rax = HexU64(1);
        not_taken_fixture.initial_state.rcx = HexU64(1);
        let not_taken = result(&run(&not_taken_fixture.to_json().unwrap(), "jit-block"), 0);
        assert_eq!(not_taken.backend, Backend::JitIrBlock);
        assert_eq!(not_taken.status, CompletionStatus::Completed);
        assert_eq!(
            not_taken.snapshot.rip,
            Observation::Available { value: HexU64(5) }
        );
        assert_eq!(
            not_taken.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );

        let branch_before_tail = [
            0x75, 0x0A, // JNZ to code_end
            0x48, 0xB8, 1, 0, 0, 0, 0, 0, 0, 0, // unreachable MOV RAX, 1
        ];
        let branch_before_tail_case = fixture(
            "s313_branch_before_tail",
            &branch_before_tail,
            8,
            ExpectedExit::ReachedCodeEnd,
        );
        let branch_before_tail_result = result(&run(&branch_before_tail_case, "jit-block"), 0);
        assert_eq!(branch_before_tail_result.backend, Backend::JitIrBlock);
        assert_eq!(
            branch_before_tail_result.status,
            CompletionStatus::Completed
        );
        assert_eq!(
            branch_before_tail_result.instructions_executed,
            Observation::Available { value: HexU64(1) }
        );
        assert_eq!(
            branch_before_tail_result.snapshot.rip,
            Observation::Available { value: HexU64(12) }
        );

        let internal_branch = [0x48, 0x03, 0xC1, 0x74, 0xFB]; // JZ back to entry
        let internal_case = fixture(
            "s313_internal_jz",
            &internal_branch,
            8,
            ExpectedExit::ReachedCodeEnd,
        );
        let result = result(&run(&internal_case, "jit-block"), 4);
        assert_eq!(result.backend, Backend::JitIrBlock);
        assert_eq!(result.status, CompletionStatus::Unsupported);
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }
}

#[test]
fn jit_block_child_has_no_fallback_and_preserves_error_exit_classes() {
    let mut unsupported = vec![0x48, 0xB8];
    unsupported.extend_from_slice(&1_u64.to_le_bytes());
    unsupported.extend_from_slice(&[0x48, 0x89, 0xC8]); // decoded MOV r64,r64 outside the native profile
    let unsupported = fixture(
        "s313_unsupported",
        &unsupported,
        8,
        ExpectedExit::ReachedCodeEnd,
    );
    let unsupported_result = result(&run(&unsupported, "jit-block"), 4);
    assert_eq!(unsupported_result.backend, Backend::JitIrBlock);
    assert_eq!(unsupported_result.status, CompletionStatus::Unsupported);
    assert!(matches!(
        unsupported_result.exit_reason,
        Observation::Unavailable { .. }
    ));

    let overlapping = [0x48, 0xB8, 0x75, 0x06, 0, 0, 0, 0, 0, 0];
    let mut invalid_entry = Fixture::from_json(&fixture(
        "s313_invalid_entry_boundary",
        &overlapping,
        8,
        ExpectedExit::ReachedCodeEnd,
    ))
    .unwrap()
    .fixture;
    invalid_entry.initial_state.rip = HexU64(2);
    let invalid_entry_result = result(&run(&invalid_entry.to_json().unwrap(), "jit-block"), 4);
    assert_eq!(invalid_entry_result.backend, Backend::JitIrBlock);
    assert_eq!(invalid_entry_result.status, CompletionStatus::Unsupported);
    assert!(matches!(
        invalid_entry_result.exit_reason,
        Observation::Unavailable { .. }
    ));

    let v2 = run(V2_FIXTURE, "jit-block");
    assert_eq!(v2.status.code(), Some(4));
    let v2_result = od_testkit::v2::RunResult::from_json(&v2.stdout).unwrap();
    assert_eq!(v2_result.backend, Backend::JitIrBlock);
    assert!(matches!(
        v2_result.outcome,
        od_testkit::v2::Outcome::Unsupported { .. }
    ));

    let invalid = run(b"{not json", "jit-block");
    assert_eq!(invalid.status.code(), Some(3));
    assert!(invalid.stdout.is_empty());

    let direct = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .args(["--hex", "90", "--engine", "jit-block"])
        .output()
        .unwrap();
    assert_eq!(direct.status.code(), Some(3));
    assert!(direct.stdout.is_empty());
}

#[test]
fn jit_block_child_reports_emitted_code_limit_as_harness_failure() {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return;
    }
    let mut code = Vec::new();
    for _ in 0..32 {
        code.extend_from_slice(&[0x48, 0x03, 0xC1]); // repeated ADD RAX, RCX
    }
    let oversized = fixture("s313_too_large", &code, 100, ExpectedExit::ReachedCodeEnd);
    let output = run(&oversized, "jit-block");
    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("EmittedCodeTooLarge"));
}
