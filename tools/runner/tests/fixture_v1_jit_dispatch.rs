//! S3-15 CLI child tests for the opt-in iterative fixture-v1 dispatcher.

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
            "od-runner-s3-15-dispatch-{}-{}",
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

fn assert_completed(output: &Output, reason: ExitReason, retired: u64, rip: u64) {
    let result = result(output, 0);
    assert_eq!(result.backend, Backend::JitIrDispatch);
    assert_eq!(result.status, CompletionStatus::Completed);
    let expected_exit = match reason {
        ExitReason::ReachedCodeEnd => ExpectedExit::ReachedCodeEnd,
        ExitReason::InstructionBudgetExhausted => ExpectedExit::InstructionBudgetExhausted,
        ExitReason::StopRequested => panic!("dispatcher CLI does not request stop in this suite"),
    };
    assert_eq!(result.expected_exit, expected_exit);
    assert_eq!(result.exit_reason, Observation::Available { value: reason });
    assert_eq!(
        result.instructions_executed,
        Observation::Available {
            value: HexU64(retired)
        }
    );
    assert_eq!(
        result.snapshot.rip,
        Observation::Available { value: HexU64(rip) }
    );
}

#[test]
fn dispatch_cli_runs_sequential_blocks_and_reports_taken_and_fallthrough_rips() {
    let code = [
        0x48, 0x3B, 0xC1, // CMP RAX, RCX: establish ZF from input registers
        0x74, 0x0A, // JZ the second MOV
        0x48, 0xB8, 1, 0, 0, 0, 0, 0, 0, 0, // MOV RAX, 1
        0x48, 0xB8, 2, 0, 0, 0, 0, 0, 0, 0, // MOV RAX, 2
    ];

    let taken = Fixture::from_json(&fixture(
        "s315_dispatch_taken",
        &code,
        2,
        ExpectedExit::InstructionBudgetExhausted,
    ))
    .unwrap()
    .fixture;
    let taken_output = run(&taken.to_json().unwrap(), "jit-dispatch");
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        assert_completed(&taken_output, ExitReason::InstructionBudgetExhausted, 2, 15);
    } else {
        assert_eq!(
            result(&taken_output, 4).status,
            CompletionStatus::Unsupported
        );
    }

    let mut not_taken_fixture = Fixture::from_json(&fixture(
        "s315_dispatch_fallthrough",
        &code,
        2,
        ExpectedExit::InstructionBudgetExhausted,
    ))
    .unwrap()
    .fixture;
    not_taken_fixture.initial_state.rcx = HexU64(1);
    let not_taken_output = run(&not_taken_fixture.to_json().unwrap(), "jit-dispatch");
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        assert_completed(
            &not_taken_output,
            ExitReason::InstructionBudgetExhausted,
            2,
            5,
        );
    } else {
        assert_eq!(
            result(&not_taken_output, 4).status,
            CompletionStatus::Unsupported
        );
    }

    // With sufficient budget the taken edge enters the second block and
    // executes the selected MOV before reaching code_end.
    let full_budget = Fixture::from_json(&fixture(
        "s315_dispatch_sequence",
        &code,
        8,
        ExpectedExit::ReachedCodeEnd,
    ))
    .unwrap()
    .fixture;
    let output = run(&full_budget.to_json().unwrap(), "jit-dispatch");
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        assert_completed(&output, ExitReason::ReachedCodeEnd, 3, code.len() as u64);
        assert_eq!(
            result(&output, 0).snapshot.gpr,
            Observation::Available {
                value: od_testkit::result::GprSnapshot {
                    rax: HexU64(2),
                    rcx: HexU64(0),
                    rdx: HexU64(0),
                    rbx: HexU64(0),
                }
            }
        );
    } else {
        assert_eq!(result(&output, 4).status, CompletionStatus::Unsupported);
    }
}

#[test]
fn dispatch_cli_handles_backward_self_loop_interior_budget_and_code_end_priority() {
    let backward = [
        0x48, 0x3B, 0xC0, // CMP RAX, RAX
        0x74, 0xFB, // JZ back to entry
        0x75, 0x00, // reachable boundary and code-end target
    ];
    let loop_case = fixture(
        "s315_dispatch_backward",
        &backward,
        3,
        ExpectedExit::InstructionBudgetExhausted,
    );

    let self_loop = [0x48, 0x3B, 0xC0, 0x74, 0xFE, 0x75, 0x00];
    let self_case = fixture(
        "s315_dispatch_self_loop",
        &self_loop,
        3,
        ExpectedExit::InstructionBudgetExhausted,
    );

    let mut interior = Vec::new();
    for value in [1_u64, 2, 3] {
        interior.extend_from_slice(&[0x48, 0xB8]);
        interior.extend_from_slice(&value.to_le_bytes());
    }
    let interior_case = fixture(
        "s315_dispatch_interior_budget",
        &interior,
        1,
        ExpectedExit::InstructionBudgetExhausted,
    );
    let code_end_code = [
        0x48, 0x3B, 0xC0, // CMP RAX, RAX
        0x74, 0x0A, // JZ code_end
        0x48, 0xB8, 9, 0, 0, 0, 0, 0, 0, 0,
    ];
    let code_end_case = fixture(
        "s315_dispatch_code_end_at_budget",
        &code_end_code,
        2,
        ExpectedExit::ReachedCodeEnd,
    );

    for (input, rip, retired) in [(loop_case, 3, 3), (self_case, 3, 3), (interior_case, 10, 1)] {
        let output = run(&input, "jit-dispatch");
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            assert_completed(
                &output,
                ExitReason::InstructionBudgetExhausted,
                retired,
                rip,
            );
        } else {
            assert_eq!(result(&output, 4).status, CompletionStatus::Unsupported);
        }
    }

    let output = run(&code_end_case, "jit-dispatch");
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        assert_completed(
            &output,
            ExitReason::ReachedCodeEnd,
            2,
            code_end_code.len() as u64,
        );
    } else {
        assert_eq!(result(&output, 4).status, CompletionStatus::Unsupported);
    }
    // Schema v1 disallows a zero instruction budget; zero-budget ordering is
    // covered directly by the dispatcher API tests.
}

#[test]
fn dispatch_cli_reports_decode_unsupported_targets_and_preserves_legacy_engines() {
    for (case_id, code) in [
        ("s315_no_native_prefix", &[0x90][..]),
        ("s315_decode_error", &[0x48][..]),
        ("s315_target_outside_view", &[0x74, 0x20][..]),
        // The JZ target lands inside the MOV immediate, not on an admitted boundary.
        (
            "s315_mid_instruction_target",
            &[0x74, 0x02, 0x48, 0xB8, 0, 0, 0, 0, 0, 0, 0, 0][..],
        ),
    ] {
        let input = fixture(case_id, code, 8, ExpectedExit::ReachedCodeEnd);
        let output = run(&input, "jit-dispatch");
        let result = result(&output, 4);
        assert_eq!(result.backend, Backend::JitIrDispatch);
        assert_eq!(result.status, CompletionStatus::Unsupported);
        assert!(matches!(
            result.instructions_executed,
            Observation::Unavailable { .. }
        ));
    }

    let overlapping = [0x48, 0xB8, 0x75, 0x06, 0, 0, 0, 0, 0, 0];
    let mut bad_entry = Fixture::from_json(&fixture(
        "s315_invalid_entry_boundary",
        &overlapping,
        8,
        ExpectedExit::ReachedCodeEnd,
    ))
    .unwrap()
    .fixture;
    bad_entry.initial_state.rip = HexU64(2);
    let output = run(&bad_entry.to_json().unwrap(), "jit-dispatch");
    let bad_entry_result = result(&output, 4);
    assert_eq!(bad_entry_result.backend, Backend::JitIrDispatch);
    assert_eq!(bad_entry_result.status, CompletionStatus::Unsupported);

    let legacy = fixture(
        "s315_legacy_compatibility",
        &[0x48, 0xB8, 9, 0, 0, 0, 0, 0, 0, 0],
        8,
        ExpectedExit::ReachedCodeEnd,
    );
    let block = run(&legacy, "jit-block");
    let jit = run(&legacy, "jit");
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        assert_eq!(result(&block, 0).backend, Backend::JitIrBlock);
        assert_eq!(result(&jit, 0).backend, Backend::Jit);
    } else {
        assert_eq!(result(&block, 4).backend, Backend::JitIrBlock);
        assert_eq!(result(&jit, 4).backend, Backend::Jit);
    }

    let v2 = run(V2_FIXTURE, "jit-dispatch");
    assert_eq!(v2.status.code(), Some(4));
    let v2 = od_testkit::v2::RunResult::from_json(&v2.stdout).unwrap();
    assert_eq!(v2.backend, Backend::JitIrDispatch);
    assert!(matches!(
        v2.outcome,
        od_testkit::v2::Outcome::Unsupported { .. }
    ));

    let direct = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .args(["--hex", "90", "--engine", "jit-dispatch"])
        .output()
        .unwrap();
    assert_eq!(direct.status.code(), Some(3));
    assert!(direct.stdout.is_empty());
}
