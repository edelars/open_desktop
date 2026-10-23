//! Cross-component CPU/memory checks. Byte programs are hand-authored, not encoder output.
use crate::HexU64;
use crate::case::MemoryPermissions;
use crate::memory::{FlatMemory, encode_bytes};
use crate::v2::{Fixture, decode_bytes};
use od_isa::execute::{REP_ITERATION_WORK_LIMIT, RunConfig, RunExit, StepOutcome, run, step};
use od_isa::memory::{AccessContext, AccessKind, ArchitecturalFault, GuestMemory, MemoryError};
use od_isa::{LongModeState, Register};

fn memory(code: &[u8], base: u64) -> FlatMemory {
    let source = crate::case::Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/s0_09_mov_imm64.case.json"
    ))
    .unwrap();
    let mut input = Fixture::convert_v1(&source).unwrap();
    input.memory[0].guest_address = HexU64(base);
    input.memory[0].length = code.len() as u32;
    input.memory[0].initial_bytes_hex = encode_bytes(code);
    input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
    input.memory[0].permissions = MemoryPermissions {
        read: false,
        write: false,
        execute: true,
    };
    FlatMemory::from_fixture(&input).unwrap()
}

fn config(budget: u64, stop: Option<u64>) -> RunConfig {
    RunConfig {
        instruction_budget: budget,
        stop_rip: stop,
        context: AccessContext::default(),
    }
}

fn execute_data_transfer_fixture(raw: &[u8], changes: &[(usize, u64)], retired: u64) -> FlatMemory {
    let fixture = Fixture::from_json(raw).unwrap().fixture;
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let mut state = fixture.initial_state.to_cpu().unwrap();
    let mut expected = state.gpr;
    for &(register, value) in changes {
        expected[register] = value;
    }
    let result = run(
        &mut state,
        &mut memory,
        config(
            fixture.instruction_budget,
            fixture.stop_rip.map(HexU64::get),
        ),
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress, "{}", fixture.case_id);
    assert_eq!(result.instructions_executed, retired, "{}", fixture.case_id);
    assert_eq!(state.gpr, expected, "{}", fixture.case_id);
    assert_eq!(state.rip, fixture.stop_rip.unwrap().get());
    assert_eq!(
        state.rflags, 0xcd7,
        "all modeled input flags must be preserved"
    );
    for region in memory.snapshot() {
        if region.region_id == "code" || region.region_id == "stack" {
            let initial = fixture
                .memory
                .iter()
                .find(|r| r.region_id == region.region_id)
                .unwrap();
            assert_eq!(region.bytes_hex, initial.initial_bytes_hex);
        }
    }
    memory
}

#[test]
fn c6_c7_and_moffs_have_hand_calculated_registers_and_actual_memory() {
    // Expected values/bytes were written independently of the decoder/executor.
    let memory = execute_data_transfer_fixture(
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_10_immediate_moffs.case.json"),
        &[
            (0, 0xffff_ffff_8000_0001),
            (1, 0xaabb_ccdd_eeff_8000),
            (8, 0x8000_0000),
            (9, 0xffff_ffff_8000_0001),
        ],
        34,
    );
    let values: [&[u8]; 4] = [
        &[0x80],
        &[1, 0x80],
        &[1, 0, 0, 0x80],
        &[1, 0, 0, 0x80, 0xff, 0xff, 0xff, 0xff],
    ];
    let mut data = vec![0x5a; 80];
    for (slot, bytes) in values.iter().enumerate() {
        data[slot * 16..slot * 16 + bytes.len()].copy_from_slice(bytes);
    }
    data[64..68].copy_from_slice(&[0x78, 0x56, 0x34, 0x12]);
    assert_region(&memory, "data", &data);
    let mut output = vec![0xa5; 128];
    for slot in 0..8 {
        let bytes = values[slot % 4];
        output[slot * 16..slot * 16 + bytes.len()].copy_from_slice(bytes);
    }
    assert_region(&memory, "moffs_output", &output);
    assert_words(
        &memory,
        &[
            0x1122_3344_5566_7f88,
            0x1122_3344_5566_7f80,
            0x1122_3344_5566_8001,
            0x8000_0001,
            0xffff_ffff_8000_0001,
            0xffff_ffff_8000_0080,
            0xffff_ffff_8000_8001,
            0x8000_0001,
            0xffff_ffff_8000_0001,
        ],
    );
}

fn assert_region(memory: &FlatMemory, id: &str, expected: &[u8]) {
    let region = memory
        .snapshot()
        .into_iter()
        .find(|r| r.region_id == id)
        .unwrap();
    assert_eq!(region.bytes_hex, encode_bytes(expected), "{id}");
}

fn assert_words(memory: &FlatMemory, expected: &[u64]) {
    let bytes: Vec<u8> = expected
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    assert_region(memory, "results", &bytes);
}

const EXTENSION_CHANGES: &[(usize, u64)] = &[
    (0, 0x1122_3344_5566_ff80),
    (1, 0xffff_ffff),
    (2, 0xffff_8001),
    (6, 0xffff_ffff),
    (7, 0xf8),
    (8, 0x80),
    (9, 0x8001),
    (10, 0x8000),
    (11, 0xffff_ffff_ffff_ff80),
    (12, 0xffff_ffff_ffff_8000),
    (13, 0xffff_ffff_8000_0000),
    (14, 0x7788_99aa_bbcc_8000),
    (15, 0x80),
];
const EXTENSION_RESULTS: &[u64] = &[
    0x77,
    0xff,
    0x1122_3344_5566_0080,
    0x80,
    0x8001,
    0x8000,
    0x1122_3344_5566_ff80,
    0xffff_ffff,
    0xffff_ffff_ffff_ff80,
    0xffff_8001,
    0xffff_ffff_ffff_8000,
    0xffff_ffff_8000_0000,
    0x8000_0000,
    0x7788_99aa_bbcc_8000,
    0xffff_ffff,
    0xf8,
    0x80,
];

#[test]
fn mov_extensions_cover_high_bytes_rex_aliases_signedness_and_destination_widths() {
    let memory = execute_data_transfer_fixture(
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_10_extend.case.json"),
        EXTENSION_CHANGES,
        34,
    );
    let data = memory
        .snapshot()
        .into_iter()
        .find(|r| r.region_id == "data")
        .unwrap();
    assert_eq!(data.bytes_hex, "80ff0180008000000080ffff");
    assert_words(&memory, EXTENSION_RESULTS);
}

#[test]
fn addr32_transfers_truncate_start_address_and_keep_each_result_observable() {
    let memory = execute_data_transfer_fixture(
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_10_addr32_transfers.case.json"),
        EXTENSION_CHANGES,
        38,
    );
    assert_words(&memory, EXTENSION_RESULTS);
    let mut data = vec![0x5a; 96];
    data[..12].copy_from_slice(&[0x80, 0xff, 1, 0x80, 0, 0x80, 0, 0, 0, 0x80, 0xff, 0xff]);
    data[32] = 0x80;
    data[48..50].copy_from_slice(&[1, 0x80]);
    data[64..68].copy_from_slice(&[1, 0, 0, 0x80]);
    data[80..88].copy_from_slice(&[1, 0, 0, 0x80, 0xff, 0xff, 0xff, 0xff]);
    assert_region(&memory, "data", &data);
}

#[test]
fn lea_produces_unmapped_noncanonical_offsets_without_reading_or_faulting() {
    execute_data_transfer_fixture(
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_10_lea_no_read.case.json"),
        &[
            (0, 0x0000_8000_0000_0000),
            (2, 0xffff_ffff),
            (6, 0x0123_4567_89ab_0012),
            (7, 0x13),
            (8, 0x0000_8000_0000_0005),
            (9, 0xffff_ffff),
            (10, 0x101020),
            (11, 0xaabb_ccdd_eeff_ffff),
            (12, 0x0000_8000_0000_0000),
            (13, 0),
            (14, 0x7788_99aa_bbcc_0023),
            (15, 0x0000_8000_0000_0000),
        ],
        12,
    );
}

#[test]
fn register_xchg_aliases_use_old_values_and_nop_is_not_eax_zero_extension() {
    execute_data_transfer_fixture(
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_10_xchg_aliases.case.json"),
        &[
            (0, 0x89ab_cdef),
            (1, 0xaabb_ccdd_eeff_ba98),
            (2, 0x5566_0011),
            (4, 0x300077),
            (8, 0x7654_3210),
            (9, 0x0123_4567_89ab_cdef),
            (10, 0x1122_3344_5566_7788),
            (11, 0x89ab_cdef),
            (12, 0xfedc_88f8),
            (13, 0xfedc_ba98_7654_3210),
        ],
        18,
    );
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ArithmeticExpected {
    gpr: [HexU64; 16],
    rflags: HexU64,
    #[serde(default)]
    rflags_defined: Option<HexU64>,
    memory: std::collections::BTreeMap<String, String>,
}

fn arithmetic_fixture_directory() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2")
}

#[test]
fn arithmetic_fixtures_match_independent_literal_answers_for_every_observable_output() {
    assert_literal_fixture_answers("s2_11_expected.json", 313);
}

#[test]
fn logic_fixtures_match_defined_flags_and_independent_literal_answers() {
    assert_literal_fixture_answers("s2_12_expected.json", 197);
}

#[test]
fn shift_rotate_fixtures_match_literal_boundary_answers() {
    assert_literal_fixture_answers("s2_13_expected.json", 144);
}

#[test]
fn sign_extension_fixtures_match_literal_full_state_answers() {
    assert_literal_fixture_answers("s2_15_sign_extension_expected.json", 6);
}

#[test]
fn bswap_fixtures_match_literal_full_state_answers() {
    assert_literal_fixture_answers("s2_15_bswap_expected.json", 7);
}

#[test]
fn bit_operation_fixtures_match_literal_full_state_and_definedness_answers() {
    assert_literal_fixture_answers("s2_15_bitops_expected.json", 12);
}

#[test]
fn bit_immediate_fixtures_match_literal_full_state_and_definedness_answers() {
    assert_literal_fixture_answers("s2_15_bitops_immediate_expected.json", 48);
}

#[test]
fn bit_immediate_memory_fixtures_match_literal_full_state_and_definedness_answers() {
    assert_literal_fixture_answers("s2_15_bitops_immediate_memory_expected.json", 48);
}

#[test]
fn bit_register_memory_fixtures_match_literal_full_state_and_definedness_answers() {
    assert_literal_fixture_answers("s2_15_bitops_register_memory_expected.json", 72);
}

#[test]
fn bit_scan_fixtures_match_literal_full_state_and_definedness_answers() {
    assert_literal_fixture_answers("s2_15_scans_expected.json", 23);
}

#[test]
fn direct_control_flow_fixtures_match_literal_full_state_and_definedness_answers() {
    assert_literal_fixture_answers("s2_16a_control_flow_expected.json", 68);
}

#[test]
fn call_ret_fixtures_match_literal_state_nested_targets_faults_and_budget() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17c_call_ret_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 6);
    for (case_id, answer) in answers {
        let raw = std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let input = Fixture::from_json(&raw).unwrap().fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let mut state = input.initial_state.to_cpu().unwrap();
        let before = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            result.instructions_executed, answer.instructions_executed,
            "{case_id}"
        );
        match (answer.exit.as_str(), answer.exception.as_ref()) {
            ("stop_address", None) => assert_eq!(result.exit, RunExit::StopAddress, "{case_id}"),
            ("instruction_budget_exhausted", None) => assert_eq!(
                result.exit,
                RunExit::InstructionBudgetExhausted,
                "{case_id}"
            ),
            ("exception", Some(expected)) => assert_eq!(
                result.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: expected.vector,
                    error_code: Some(expected.error_code),
                    linear_address: expected.linear_address.map(HexU64::get),
                }),
                "{case_id}"
            ),
            other => panic!("invalid literal answer for {case_id}: {}", other.0),
        }
        if answer.exception.is_some() {
            assert_eq!(state, before, "{case_id}: no architectural commit");
        }
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn call_and_ret_stack_faults_leave_state_and_memory_uncommitted() {
    let directory = arithmetic_fixture_directory();
    let input = Fixture::from_json(
        &std::fs::read(directory.join("s2_17c_call_forward.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let stack_address = 0x3000f8;
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(6),
        linear_address: Some(stack_address),
    };
    for (rip, access, error) in [
        (
            input.initial_state.rip.get(),
            AccessKind::Write,
            MemoryError::Architectural(fault.clone()),
        ),
        (
            input.initial_state.rip.get() + 10,
            AccessKind::Read,
            MemoryError::Backend("injected RET stack read failure".into()),
        ),
    ] {
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        memory.inject_once(stack_address, access, error.clone());
        let mut state = input.initial_state.to_cpu().unwrap();
        state.rip = rip;
        if access == AccessKind::Read {
            state.gpr[4] = 0x3000f8;
        }
        let before = state.clone();
        let before_memory = memory.snapshot();
        let result = run(&mut state, &mut memory, config(1, None), || false);
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{access:?}");
        assert_eq!(memory.snapshot(), before_memory, "{access:?}");
        match error {
            MemoryError::Architectural(fault) => assert_eq!(result.exit, RunExit::Fault(fault)),
            error => assert_eq!(result.exit, RunExit::MemoryError(error)),
        }
    }
}

#[test]
fn indirect_call_fixtures_match_literal_targets_faults_and_budget() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17d_indirect_call_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 5);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let mut state = input.initial_state.to_cpu().unwrap();
        let before = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            result.instructions_executed, answer.instructions_executed,
            "{case_id}"
        );
        match (answer.exit.as_str(), answer.exception.as_ref()) {
            ("stop_address", None) => assert_eq!(result.exit, RunExit::StopAddress, "{case_id}"),
            ("instruction_budget_exhausted", None) => assert_eq!(
                result.exit,
                RunExit::InstructionBudgetExhausted,
                "{case_id}"
            ),
            ("exception", Some(expected)) => assert_eq!(
                result.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: expected.vector,
                    error_code: Some(expected.error_code),
                    linear_address: expected.linear_address.map(HexU64::get),
                }),
                "{case_id}"
            ),
            other => panic!("invalid literal answer for {case_id}: {}", other.0),
        }
        if answer.exception.is_some() {
            assert_eq!(state, before, "{case_id}: no architectural commit");
        }
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn indirect_memory_call_fixtures_match_literal_full_state_and_fault_order() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17e_indirect_call_memory_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 9);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        if case_id == "s2_17e_call_mem_read_fault" {
            memory.inject_once(
                0x0020_0000,
                AccessKind::Read,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    linear_address: Some(0x0020_0000),
                }),
            );
        }
        let mut state = input.initial_state.to_cpu().unwrap();
        let initial = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            result.instructions_executed, answer.instructions_executed,
            "{case_id}"
        );
        match (answer.exit.as_str(), answer.exception.as_ref()) {
            ("stop_address", None) => assert_eq!(result.exit, RunExit::StopAddress, "{case_id}"),
            ("instruction_budget_exhausted", None) => assert_eq!(
                result.exit,
                RunExit::InstructionBudgetExhausted,
                "{case_id}"
            ),
            ("exception", Some(expected)) => assert_eq!(
                result.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: expected.vector,
                    error_code: Some(expected.error_code),
                    linear_address: expected.linear_address.map(HexU64::get),
                }),
                "{case_id}"
            ),
            other => panic!("invalid literal answer for {case_id}: {}", other.0),
        }
        if let Some(expected) = &answer.exception {
            assert_eq!(expected.fault_rip.get(), state.rip, "{case_id}");
            assert_eq!(state, initial, "{case_id}: no CPU commit");
        }
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn ret_imm16_fixtures_match_literal_adjustment_nested_cleanup_and_faults() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17f_ret_imm16_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 9);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        if case_id == "s2_17f_ret_imm_read_fault" {
            memory.inject_once(
                0x0030_0100,
                AccessKind::Read,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    linear_address: Some(0x0030_0100),
                }),
            );
        }
        if case_id == "s2_17f_ret_imm_truncated" {
            memory.inject_once(
                0x0010_0002,
                AccessKind::Fetch,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    linear_address: Some(0x0010_0002),
                }),
            );
        }
        let mut state = input.initial_state.to_cpu().unwrap();
        let initial = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            result.instructions_executed, answer.instructions_executed,
            "{case_id}"
        );
        match (answer.exit.as_str(), answer.exception.as_ref()) {
            ("stop_address", None) => assert_eq!(result.exit, RunExit::StopAddress, "{case_id}"),
            ("instruction_budget_exhausted", None) => assert_eq!(
                result.exit,
                RunExit::InstructionBudgetExhausted,
                "{case_id}"
            ),
            ("exception", Some(expected)) => assert_eq!(
                result.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: expected.vector,
                    error_code: Some(expected.error_code),
                    linear_address: expected.linear_address.map(HexU64::get),
                }),
                "{case_id}"
            ),
            other => panic!("invalid literal answer for {case_id}: {}", other.0),
        }
        if let Some(expected) = &answer.exception {
            assert_eq!(expected.fault_rip.get(), state.rip, "{case_id}");
            assert_eq!(state, initial, "{case_id}: no CPU commit");
        }
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct IndirectJumpExpected {
    gpr: [HexU64; 16],
    rip: HexU64,
    rflags: HexU64,
    rflags_defined: HexU64,
    memory: std::collections::BTreeMap<String, String>,
    exit: String,
    exception: Option<IndirectJumpException>,
    instructions_executed: u64,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct IndirectJumpException {
    vector: u8,
    error_code: u32,
    #[serde(default)]
    linear_address: Option<HexU64>,
    fault_rip: HexU64,
}

#[test]
fn indirect_jmp_fixtures_reach_new_decode_address_and_preserve_noncanonical_fault_state() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_16d_indirect_jmp_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 3);
    for (case_id, answer) in answers {
        let raw = std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let input = Fixture::from_json(&raw).unwrap().fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let mut state = input.initial_state.to_cpu().unwrap();
        let actual = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(
            state.rflags & input.rflags_mask.get(),
            answer.rflags.get() & input.rflags_mask.get(),
            "{case_id}"
        );
        assert_eq!(
            actual.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            actual.instructions_executed, answer.instructions_executed,
            "{case_id}"
        );
        match (answer.exit.as_str(), answer.exception.as_ref()) {
            ("stop_address", None) => assert_eq!(actual.exit, RunExit::StopAddress, "{case_id}"),
            ("exception", Some(expected)) => assert_eq!(
                actual.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: expected.vector,
                    error_code: Some(expected.error_code),
                    linear_address: expected.linear_address.map(HexU64::get),
                }),
                "{case_id}"
            ),
            other => panic!("invalid literal answer for {case_id}: {}", other.0),
        }
        if let Some(expected) = &answer.exception {
            assert_eq!(expected.fault_rip.get(), state.rip, "{case_id}");
            assert_eq!(
                state,
                input.initial_state.to_cpu().unwrap(),
                "{case_id}: no commit"
            );
        }
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn indirect_memory_jmp_fixtures_read_qword_before_fault_or_target_decode() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_16e_indirect_jmp_memory_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 7);
    for (case_id, answer) in answers {
        let raw = std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let input = Fixture::from_json(&raw).unwrap().fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        if case_id == "s2_16e_jmp_mem_read_fault" {
            memory.inject_once(
                0x0020_0000,
                AccessKind::Read,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    linear_address: Some(0x0020_0000),
                }),
            );
        }
        let mut state = input.initial_state.to_cpu().unwrap();
        let initial = state.clone();
        let actual = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(
            state.rflags & input.rflags_mask.get(),
            answer.rflags.get() & input.rflags_mask.get(),
            "{case_id}"
        );
        assert_eq!(
            actual.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            actual.instructions_executed, answer.instructions_executed,
            "{case_id}"
        );
        match (answer.exit.as_str(), answer.exception.as_ref()) {
            ("stop_address", None) => assert_eq!(actual.exit, RunExit::StopAddress, "{case_id}"),
            ("exception", Some(expected)) => assert_eq!(
                actual.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: expected.vector,
                    error_code: Some(expected.error_code),
                    linear_address: expected.linear_address.map(HexU64::get),
                }),
                "{case_id}"
            ),
            other => panic!("invalid literal answer for {case_id}: {}", other.0),
        }
        if answer.exception.is_some() {
            assert_eq!(
                state, initial,
                "{case_id}: source/target fault must not commit"
            );
        }
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn setcc_fixtures_match_literal_full_state_and_definedness_answers() {
    assert_literal_fixture_answers("s2_16b_setcc_expected.json", 79);
}

#[test]
fn cmovcc_fixtures_match_literal_full_state_and_definedness_answers() {
    assert_literal_fixture_answers("s2_16c_cmovcc_expected.json", 70);
}

#[test]
fn push_pop_fixtures_match_literal_full_state_and_definedness_answers() {
    assert_literal_fixture_answers("s2_17a_push_pop_expected.json", 8);
}

#[test]
fn push_immediate_fixtures_match_literal_signed_boundary_answers() {
    assert_literal_fixture_answers("s2_17b_push_immediate_expected.json", 8);
}

#[test]
fn push_pop16_fixtures_match_literal_state_alias_and_fault_answers() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17g_push_pop16_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 10);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        match case_id.as_str() {
            "s2_17g_push_fault_bottom" => memory.inject_once(
                0x002f_fffe,
                AccessKind::Write,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(6),
                    linear_address: Some(0x002f_fffe),
                }),
            ),
            "s2_17g_pop_fault_top" => memory.inject_once(
                0x0030_0100,
                AccessKind::Read,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    linear_address: Some(0x0030_0100),
                }),
            ),
            "s2_17g_truncated_fetch" => memory.inject_once(
                0x0010_0001,
                AccessKind::Fetch,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    linear_address: Some(0x0010_0001),
                }),
            ),
            _ => {}
        }
        let mut state = input.initial_state.to_cpu().unwrap();
        let before = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            result.instructions_executed, answer.instructions_executed,
            "{case_id}"
        );
        match (answer.exit.as_str(), answer.exception.as_ref()) {
            ("stop_address", None) => assert_eq!(result.exit, RunExit::StopAddress, "{case_id}"),
            ("exception", Some(expected)) => {
                assert_eq!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: expected.vector,
                        error_code: Some(expected.error_code),
                        linear_address: expected.linear_address.map(HexU64::get),
                    }),
                    "{case_id}"
                );
                assert_eq!(state, before, "{case_id}: no CPU commit");
                assert_eq!(state.rip, expected.fault_rip.get(), "{case_id}");
            }
            other => panic!("invalid literal answer for {case_id}: {}", other.0),
        }
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn register_stack_rexw_fixtures_match_literal_width_and_prefix_order_answers() {
    assert_literal_fixture_answers("s2_17n_register_stack_rexw_expected.json", 6);
}

#[test]
fn popfq_fixtures_match_literal_stack_flags_and_definedness_answers() {
    assert_literal_fixture_answers("s2_17o_popfq_expected.json", 2);
}

#[test]
fn popfw_fixtures_match_literal_word_stack_flags_and_answers() {
    assert_literal_fixture_answers("s2_17r_popfw_expected.json", 1);
}

#[test]
fn popfw_preserves_unmasked_rflags_bits_locally() {
    let directory = arithmetic_fixture_directory();
    let raw =
        std::fs::read(directory.join("s2_17r_popfw_updates_allowed_flags.case.json")).unwrap();
    let fixture = Fixture::from_json(&raw).unwrap().fixture;
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let mut state = fixture.initial_state.to_cpu().unwrap();
    // The v2 fixture mask does not compare IF, reserved bit 3, ID, or upper
    // RFLAGS bits. Keep IOPL=0 for the admitted profile and assert those bits
    // directly here. The stack source itself carries IOPL=3, which POPFW must
    // leave privileged and unchanged.
    let initial_flags = 2 | (1 << 3) | (1 << 9) | (1 << 21) | (1 << 63);
    state.rflags = initial_flags;
    let result = run(
        &mut state,
        &mut memory,
        config(
            fixture.instruction_budget,
            fixture.stop_rip.map(HexU64::get),
        ),
        || false,
    );

    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    let modeled = 0x0cd5;
    assert_eq!(state.rflags, (initial_flags & !modeled) | modeled);
    assert_eq!(
        state.gpr[Register::Rsp as usize],
        fixture.initial_state.gpr[4].get() + 2
    );
    assert_eq!(state.rip, fixture.stop_rip.unwrap().get());
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn popfq_out_of_profile_fixture_inputs_return_unsupported_without_commit() {
    let directory = arithmetic_fixture_directory();
    let raw = std::fs::read(directory.join("s2_17o_loads_status_flags_and_df.case.json")).unwrap();
    let base = Fixture::from_json(&raw).unwrap().fixture;

    for event_flag in [1_u64 << 8, 1_u64 << 18] {
        let mut fixture = base.clone();
        let stack = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "stack")
            .unwrap();
        let mut bytes = decode_bytes(&stack.initial_bytes_hex).unwrap();
        let offset = (fixture.initial_state.gpr[4].get() - stack.guest_address.get()) as usize;
        bytes[offset..offset + 8].copy_from_slice(&event_flag.to_le_bytes());
        stack.initial_bytes_hex = encode_bytes(&bytes);

        // Round-trip through the public fixture parser to exercise the same input contract.
        let raw = serde_json::to_vec(&fixture).unwrap();
        let fixture = Fixture::from_json(&raw).unwrap().fixture;
        let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
        let memory_before = memory.snapshot();
        let mut state = fixture.initial_state.to_cpu().unwrap();
        let state_before = state.clone();
        let actual = run(&mut state, &mut memory, config(1, None), || false);

        assert!(matches!(actual.exit, RunExit::Unsupported { opcode: 0x9d }));
        assert_eq!(state, state_before);
        assert_eq!(memory.snapshot(), memory_before);
    }
}

fn assert_popfw_unsupported_without_commit(stack_flags: u16, initial_flags: u64) {
    let directory = arithmetic_fixture_directory();
    let raw =
        std::fs::read(directory.join("s2_17r_popfw_updates_allowed_flags.case.json")).unwrap();
    let mut fixture = Fixture::from_json(&raw).unwrap().fixture;
    let stack = fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "stack")
        .unwrap();
    let mut bytes = decode_bytes(&stack.initial_bytes_hex).unwrap();
    let offset = (fixture.initial_state.gpr[4].get() - stack.guest_address.get()) as usize;
    bytes[offset..offset + 2].copy_from_slice(&stack_flags.to_le_bytes());
    stack.initial_bytes_hex = encode_bytes(&bytes);

    let raw = serde_json::to_vec(&fixture).unwrap();
    let fixture = Fixture::from_json(&raw).unwrap().fixture;
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let mut state = fixture.initial_state.to_cpu().unwrap();
    state.rflags = initial_flags;
    let state_before = state.clone();
    let actual = run(&mut state, &mut memory, config(1, None), || false);

    assert!(matches!(actual.exit, RunExit::Unsupported { opcode: 0x9d }));
    assert_eq!(state, state_before);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn popfw_tf_source_returns_unsupported_without_cpu_or_memory_commit() {
    assert_popfw_unsupported_without_commit(1 << 8, 2);
}

#[test]
fn popfw_initial_tf_returns_unsupported_without_cpu_or_memory_commit() {
    assert_popfw_unsupported_without_commit(0, 2 | (1 << 8));
}

#[test]
fn popfw_nt_source_returns_unsupported_without_cpu_or_memory_commit() {
    assert_popfw_unsupported_without_commit(1 << 14, 2);
}

#[test]
fn popfw_initial_nt_returns_unsupported_without_cpu_or_memory_commit() {
    assert_popfw_unsupported_without_commit(0, 2 | (1 << 14));
}

#[test]
fn popfw_nonzero_current_iopl_is_unsupported_before_stack_access() {
    let directory = arithmetic_fixture_directory();
    let raw =
        std::fs::read(directory.join("s2_17r_popfw_updates_allowed_flags.case.json")).unwrap();
    let fixture = Fixture::from_json(&raw).unwrap().fixture;
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let mut state = fixture.initial_state.to_cpu().unwrap();
    state.rflags = 2 | (3 << 12);
    let state_before = state.clone();
    let stack_address = state.gpr[Register::Rsp as usize];
    memory.inject_once(
        stack_address,
        AccessKind::Read,
        MemoryError::Unmapped {
            address: stack_address,
            access: AccessKind::Read,
        },
    );
    let result = run(&mut state, &mut memory, config(1, None), || false);

    assert!(matches!(result.exit, RunExit::Unsupported { opcode: 0x9d }));
    assert_eq!(state, state_before);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn pushfq_fixture_matches_literal_masked_rflags_image() {
    assert_literal_fixture_answers("s2_17p_pushfq_expected.json", 1);
}

#[test]
fn pushf_rexw_overrides_66_fixture_matches_qword_round_trip() {
    assert_literal_fixture_answers("s2_17s_pushf_rexw_expected.json", 1);
}

#[test]
fn pushf_rexw_round_trip_preserves_unmasked_rflags_and_full_qword_stack_image() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_17s_pushf_rexw_overrides_66_qword_roundtrip.case.json"
    ))
    .unwrap()
    .fixture;
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let mut state = fixture.initial_state.to_cpu().unwrap();
    // Exercise an allowed POPFQ source state: TF/NT/AC, RF, and IOPL are clear.
    // IF and ID remain outside the fixture mask and must round-trip unchanged.
    state.rflags |= (1 << 9) | (1 << 21);
    let original_flags = state.rflags;
    let original_rsp = state.gpr[Register::Rsp as usize];

    let result = run(
        &mut state,
        &mut memory,
        config(2, fixture.stop_rip.map(HexU64::get)),
        || false,
    );

    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.rip, fixture.stop_rip.unwrap().get());
    assert_eq!(state.gpr[Register::Rsp as usize], original_rsp);
    assert_eq!(state.rflags, original_flags);
    let stack_spec = fixture
        .memory
        .iter()
        .find(|region| region.region_id == "stack")
        .unwrap();
    let stack = memory
        .snapshot()
        .into_iter()
        .find(|region| region.region_id == "stack")
        .unwrap();
    let offset = (original_rsp - 8 - stack_spec.guest_address.get()) as usize;
    let expected_image = (original_flags & 0x00fc_ffff)
        .to_le_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let start = offset * 2;
    assert_eq!(&stack.bytes_hex[start..start + 16], expected_image);
    assert_eq!(&stack.bytes_hex[start - 2..start], "a5");
    assert_eq!(&stack.bytes_hex[start + 16..start + 18], "a5");
}

#[test]
fn pushfw_fixture_matches_literal_low_flags_word() {
    assert_literal_fixture_answers("s2_17q_pushfw_expected.json", 1);
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MovsbExpected {
    gpr: [HexU64; 16],
    rflags: HexU64,
    rflags_defined: HexU64,
    rip: HexU64,
    memory: std::collections::BTreeMap<String, String>,
    fault: Option<MovsbFault>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MovsbFault {
    access: String,
    address: HexU64,
}

struct MovsbAccessLog {
    inner: FlatMemory,
    accesses: Vec<(AccessKind, u64, usize)>,
    mfences: Vec<AccessContext>,
    mfence_error: Option<MemoryError>,
}

impl GuestMemory for MovsbAccessLog {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        self.accesses.push((AccessKind::Fetch, address, 1));
        self.inner.fetch(address, context)
    }

    fn read(
        &mut self,
        address: u64,
        context: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        self.accesses
            .push((AccessKind::Read, address, destination.len()));
        self.inner.read(address, context, destination)
    }

    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        source: &[u8],
    ) -> Result<(), MemoryError> {
        self.accesses
            .push((AccessKind::Write, address, source.len()));
        self.inner.write(address, context, source)
    }

    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: od_isa::OperandSize,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        self.inner
            .atomic_exchange_integer(address, context, size, replacement)
    }

    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: od_isa::OperandSize,
        addend: u64,
    ) -> Result<u64, MemoryError> {
        self.inner
            .atomic_fetch_add_integer(address, context, size, addend)
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: od_isa::OperandSize,
        expected: u64,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        self.inner
            .atomic_compare_exchange_integer(address, context, size, expected, replacement)
    }

    fn mfence(&mut self, context: AccessContext) -> Result<(), MemoryError> {
        self.mfences.push(context);
        if let Some(error) = &self.mfence_error {
            return Err(error.clone());
        }
        self.inner.mfence(context)
    }
}

#[test]
fn mfence_uses_one_backend_completion_call_and_failure_does_not_retire_it() {
    let directory = arithmetic_fixture_directory();
    let input = Fixture::from_json(
        &std::fs::read(directory.join("s2_21g_mfence_store_fence_load.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let context = AccessContext::default();

    // Begin directly at MFENCE to prove it fetches only its encoding and
    // performs no operand or data-memory access of its own.
    let mut state = input.initial_state.to_cpu().unwrap();
    state.rip += 3;
    let before = state.clone();
    let mut memory = MovsbAccessLog {
        inner: FlatMemory::from_fixture(&input).unwrap(),
        accesses: Vec::new(),
        mfences: Vec::new(),
        mfence_error: None,
    };
    let result = run(&mut state, &mut memory, config(1, None), || false);
    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, before.rip + 3);
    assert_eq!(state.gpr, before.gpr);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(memory.mfences, [context]);
    assert_eq!(
        memory.accesses,
        [
            (AccessKind::Fetch, before.rip, 1),
            (AccessKind::Fetch, before.rip + 1, 1),
            (AccessKind::Fetch, before.rip + 2, 1),
        ]
    );

    // If the fence backend rejects completion, it does not retire or roll
    // back the store that completed in the preceding instruction.
    let mut state = input.initial_state.to_cpu().unwrap();
    let mut memory = MovsbAccessLog {
        inner: FlatMemory::from_fixture(&input).unwrap(),
        accesses: Vec::new(),
        mfences: Vec::new(),
        mfence_error: Some(MemoryError::Backend("fence unsupported".into())),
    };
    let result = run(&mut state, &mut memory, config(3, None), || false);
    assert_eq!(
        result.exit,
        RunExit::MemoryError(MemoryError::Backend("fence unsupported".into()))
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, input.initial_state.rip.get() + 3);
    assert_eq!(state.gpr, input.initial_state.gpr.map(HexU64::get));
    assert_eq!(state.rflags, input.initial_state.rflags.get());
    assert_eq!(memory.mfences, [context]);
    assert_eq!(memory.inner.snapshot()[1].bytes_hex, "8877665544332211");
    assert_eq!(
        memory.accesses.last(),
        Some(&(AccessKind::Fetch, input.initial_state.rip.get() + 5, 1))
    );
}

#[test]
fn movsb_fixtures_match_literal_state_and_access_order() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, MovsbExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_18a_movsb_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 5);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let original_state = input.initial_state.to_cpu().unwrap();
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&input).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        if let Some(fault) = &answer.fault {
            let access = match fault.access.as_str() {
                "read" => AccessKind::Read,
                "write" => AccessKind::Write,
                _ => panic!("unknown MOVSB injected access kind: {}", fault.access),
            };
            memory.inner.inject_once(
                fault.address.get(),
                access,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(if access == AccessKind::Write { 6 } else { 4 }),
                    linear_address: Some(fault.address.get()),
                }),
            );
        }
        let initial_memory = memory.inner.snapshot();
        let mut state = original_state.clone();
        let actual = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        let source = original_state.gpr[Register::Rsi as usize];
        let destination = original_state.gpr[Register::Rdi as usize];
        match answer.fault.as_ref() {
            Some(fault) => {
                assert_eq!(
                    actual.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: 14,
                        error_code: Some(if fault.access == "write" { 6 } else { 4 }),
                        linear_address: Some(fault.address.get()),
                    }),
                    "{case_id}"
                );
                assert_eq!(actual.instructions_executed, 0, "{case_id}");
                // This checks this executor's no-commit policy. Intel SDM 093
                // does not establish the destination-#PF restart snapshot;
                // see ISSUE-016 before treating it as a hardware guarantee.
                assert_eq!(state, original_state, "{case_id}");
                assert_eq!(memory.inner.snapshot(), initial_memory, "{case_id}");
                let expected_accesses = if fault.access == "read" {
                    vec![
                        (AccessKind::Fetch, 0x0010_0000, 1),
                        (AccessKind::Read, source, 1),
                    ]
                } else {
                    vec![
                        (AccessKind::Fetch, 0x0010_0000, 1),
                        (AccessKind::Read, source, 1),
                        (AccessKind::Write, destination, 1),
                    ]
                };
                assert_eq!(memory.accesses, expected_accesses, "{case_id}");
            }
            None => {
                assert_eq!(actual.exit, RunExit::StopAddress, "{case_id}");
                assert_eq!(actual.instructions_executed, 1, "{case_id}");
                assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
                assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
                assert_eq!(state.rip, answer.rip.get(), "{case_id}");
                assert_eq!(
                    actual.rflags_defined,
                    answer.rflags_defined.get(),
                    "{case_id}"
                );
                for region in memory.inner.snapshot() {
                    assert_eq!(
                        region.bytes_hex.as_str(),
                        answer.memory.get(&region.region_id).unwrap().as_str(),
                        "{case_id}: {}",
                        region.region_id
                    );
                }
                assert_eq!(
                    memory.accesses,
                    vec![
                        (AccessKind::Fetch, 0x0010_0000, 1),
                        (AccessKind::Read, source, 1),
                        (AccessKind::Write, destination, 1),
                    ],
                    "{case_id}"
                );
            }
        }
    }
}

#[test]
fn movsw_fixtures_match_literal_state_and_access_order() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, MovsbExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_18b_movsw_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 5);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let original_state = input.initial_state.to_cpu().unwrap();
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&input).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        if let Some(fault) = &answer.fault {
            let access = match fault.access.as_str() {
                "read" => AccessKind::Read,
                "write" => AccessKind::Write,
                _ => panic!("unknown MOVSW injected access kind: {}", fault.access),
            };
            memory.inner.inject_once(
                fault.address.get(),
                access,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(if access == AccessKind::Write { 6 } else { 4 }),
                    linear_address: Some(fault.address.get()),
                }),
            );
        }
        let initial_memory = memory.inner.snapshot();
        let mut state = original_state.clone();
        let actual = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        let source = original_state.gpr[Register::Rsi as usize];
        let destination = original_state.gpr[Register::Rdi as usize];
        match answer.fault.as_ref() {
            Some(fault) => {
                assert_eq!(
                    actual.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: 14,
                        error_code: Some(if fault.access == "write" { 6 } else { 4 }),
                        linear_address: Some(fault.address.get()),
                    }),
                    "{case_id}"
                );
                assert_eq!(actual.instructions_executed, 0, "{case_id}");
                // This checks the executor's local no-commit policy only. The
                // hardware restart snapshot after MOVSW destination #PF is
                // not specified by Intel SDM 093; see ISSUE-016.
                assert_eq!(state, original_state, "{case_id}");
                assert_eq!(memory.inner.snapshot(), initial_memory, "{case_id}");
                let expected_accesses = if fault.access == "read" {
                    vec![
                        (AccessKind::Fetch, 0x0010_0000, 1),
                        (AccessKind::Fetch, 0x0010_0001, 1),
                        (AccessKind::Read, source, 2),
                    ]
                } else {
                    vec![
                        (AccessKind::Fetch, 0x0010_0000, 1),
                        (AccessKind::Fetch, 0x0010_0001, 1),
                        (AccessKind::Read, source, 2),
                        (AccessKind::Write, destination, 2),
                    ]
                };
                assert_eq!(memory.accesses, expected_accesses, "{case_id}");
            }
            None => {
                assert_eq!(actual.exit, RunExit::StopAddress, "{case_id}");
                assert_eq!(actual.instructions_executed, 1, "{case_id}");
                assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
                assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
                assert_eq!(state.rip, answer.rip.get(), "{case_id}");
                assert_eq!(
                    actual.rflags_defined,
                    answer.rflags_defined.get(),
                    "{case_id}"
                );
                for region in memory.inner.snapshot() {
                    assert_eq!(
                        region.bytes_hex.as_str(),
                        answer.memory.get(&region.region_id).unwrap().as_str(),
                        "{case_id}: {}",
                        region.region_id
                    );
                }
                assert_eq!(
                    memory.accesses,
                    vec![
                        (AccessKind::Fetch, 0x0010_0000, 1),
                        (AccessKind::Fetch, 0x0010_0001, 1),
                        (AccessKind::Read, source, 2),
                        (AccessKind::Write, destination, 2),
                    ],
                    "{case_id}"
                );
            }
        }
    }
}

#[test]
fn movsd_fixtures_match_literal_state_and_access_order() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, MovsbExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_18c_movsd_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 5);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let original_state = input.initial_state.to_cpu().unwrap();
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&input).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        if let Some(fault) = &answer.fault {
            let access = match fault.access.as_str() {
                "read" => AccessKind::Read,
                "write" => AccessKind::Write,
                _ => panic!("unknown MOVSD injected access kind: {}", fault.access),
            };
            memory.inner.inject_once(
                fault.address.get(),
                access,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(if access == AccessKind::Write { 6 } else { 4 }),
                    linear_address: Some(fault.address.get()),
                }),
            );
        }
        let initial_memory = memory.inner.snapshot();
        let mut state = original_state.clone();
        let actual = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        let source = original_state.gpr[Register::Rsi as usize];
        let destination = original_state.gpr[Register::Rdi as usize];
        match answer.fault.as_ref() {
            Some(fault) => {
                assert_eq!(
                    actual.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: 14,
                        error_code: Some(if fault.access == "write" { 6 } else { 4 }),
                        linear_address: Some(fault.address.get()),
                    }),
                    "{case_id}"
                );
                assert_eq!(actual.instructions_executed, 0, "{case_id}");
                // This checks the executor's local no-commit policy only. Intel
                // SDM 093 does not specify the MOVSD destination-#PF restart
                // snapshot; see ISSUE-016.
                assert_eq!(state, original_state, "{case_id}");
                assert_eq!(memory.inner.snapshot(), initial_memory, "{case_id}");
                let expected_accesses = if fault.access == "read" {
                    vec![
                        (AccessKind::Fetch, 0x0010_0000, 1),
                        (AccessKind::Read, source, 4),
                    ]
                } else {
                    vec![
                        (AccessKind::Fetch, 0x0010_0000, 1),
                        (AccessKind::Read, source, 4),
                        (AccessKind::Write, destination, 4),
                    ]
                };
                assert_eq!(memory.accesses, expected_accesses, "{case_id}");
            }
            None => {
                assert_eq!(actual.exit, RunExit::StopAddress, "{case_id}");
                assert_eq!(actual.instructions_executed, 1, "{case_id}");
                assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
                assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
                assert_eq!(state.rip, answer.rip.get(), "{case_id}");
                assert_eq!(
                    actual.rflags_defined,
                    answer.rflags_defined.get(),
                    "{case_id}"
                );
                for region in memory.inner.snapshot() {
                    assert_eq!(
                        region.bytes_hex.as_str(),
                        answer.memory.get(&region.region_id).unwrap().as_str(),
                        "{case_id}: {}",
                        region.region_id
                    );
                }
                assert_eq!(
                    memory.accesses,
                    vec![
                        (AccessKind::Fetch, 0x0010_0000, 1),
                        (AccessKind::Read, source, 4),
                        (AccessKind::Write, destination, 4),
                    ],
                    "{case_id}"
                );
            }
        }
    }
}

#[test]
fn movsq_fixtures_match_literal_state_and_exact_qword_accesses() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, MovsbExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_18d_movsq_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 7);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let original_state = input.initial_state.to_cpu().unwrap();
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&input).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        if let Some(fault) = &answer.fault {
            let access = match fault.access.as_str() {
                "read" => AccessKind::Read,
                "write" => AccessKind::Write,
                _ => panic!("unknown MOVSQ injected access kind: {}", fault.access),
            };
            memory.inner.inject_once(
                fault.address.get(),
                access,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(if access == AccessKind::Write { 6 } else { 4 }),
                    linear_address: Some(fault.address.get()),
                }),
            );
        }
        let initial_memory = memory.inner.snapshot();
        let mut state = original_state.clone();
        let actual = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        let source = original_state.gpr[Register::Rsi as usize];
        let destination = original_state.gpr[Register::Rdi as usize];
        let mut expected_accesses: Vec<_> = (0..input.memory[0].length)
            .map(|offset| (AccessKind::Fetch, 0x0010_0000 + u64::from(offset), 1))
            .collect();
        expected_accesses.push((AccessKind::Read, source, 8));
        if answer
            .fault
            .as_ref()
            .is_none_or(|fault| fault.access == "write")
        {
            expected_accesses.push((AccessKind::Write, destination, 8));
        }
        assert_eq!(memory.accesses, expected_accesses, "{case_id}");

        if let Some(fault) = &answer.fault {
            assert_eq!(
                actual.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(if fault.access == "write" { 6 } else { 4 }),
                    linear_address: Some(fault.address.get()),
                }),
                "{case_id}"
            );
            assert_eq!(actual.instructions_executed, 0, "{case_id}");
            // Local bounded-memory policy only. SDM 093 leaves the hardware
            // MOVSQ destination-#PF restart snapshot unspecified; see ISSUE-016.
            assert_eq!(state, original_state, "{case_id}");
            assert_eq!(memory.inner.snapshot(), initial_memory, "{case_id}");
        } else {
            assert_eq!(actual.exit, RunExit::StopAddress, "{case_id}");
            assert_eq!(actual.instructions_executed, 1, "{case_id}");
            assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
            assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
            assert_eq!(state.rip, answer.rip.get(), "{case_id}");
            assert_eq!(
                actual.rflags_defined,
                answer.rflags_defined.get(),
                "{case_id}"
            );
            for region in memory.inner.snapshot() {
                assert_eq!(
                    region.bytes_hex,
                    answer.memory.get(&region.region_id).unwrap().as_str(),
                    "{case_id}: {}",
                    region.region_id
                );
            }
        }
    }
}

#[test]
fn stosb_fixtures_match_literal_state_and_write_only_accesses() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, MovsbExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_18e_stosb_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 4);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let original_state = input.initial_state.to_cpu().unwrap();
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&input).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        if let Some(fault) = &answer.fault {
            assert_eq!(fault.access, "write", "{case_id}");
            memory.inner.inject_once(
                fault.address.get(),
                AccessKind::Write,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(6),
                    linear_address: Some(fault.address.get()),
                }),
            );
        }
        let initial_memory = memory.inner.snapshot();
        let mut state = original_state.clone();
        let actual = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        let destination = original_state.gpr[Register::Rdi as usize];
        let code_len = input.memory[0].length;
        let expected_accesses = (0..code_len)
            .map(|offset| (AccessKind::Fetch, 0x0010_0000 + u64::from(offset), 1))
            .chain(std::iter::once((AccessKind::Write, destination, 1)))
            .collect::<Vec<_>>();
        assert_eq!(memory.accesses, expected_accesses, "{case_id}");

        if let Some(fault) = &answer.fault {
            assert_eq!(
                actual.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(6),
                    linear_address: Some(fault.address.get()),
                }),
                "{case_id}"
            );
            assert_eq!(actual.instructions_executed, 0, "{case_id}");
            // This is the local injected-backend no-commit policy. The
            // hardware restart snapshot for STOSB destination #PF is not
            // established by these tests (ISSUE-016 tracks string faults).
            assert_eq!(state, original_state, "{case_id}");
            assert_eq!(memory.inner.snapshot(), initial_memory, "{case_id}");
        } else {
            assert_eq!(actual.exit, RunExit::StopAddress, "{case_id}");
            assert_eq!(actual.instructions_executed, 1, "{case_id}");
            assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
            assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
            assert_eq!(state.rip, answer.rip.get(), "{case_id}");
            assert_eq!(
                actual.rflags_defined,
                answer.rflags_defined.get(),
                "{case_id}"
            );
            for region in memory.inner.snapshot() {
                assert_eq!(
                    region.bytes_hex,
                    answer.memory.get(&region.region_id).unwrap().as_str(),
                    "{case_id}: {}",
                    region.region_id
                );
            }
        }
    }
}

#[test]
fn stosb_rejects_prefixes_without_committing() {
    let directory = arithmetic_fixture_directory();
    let mut fixture =
        Fixture::from_json(&std::fs::read(directory.join("s2_18e_stosb_df0.case.json")).unwrap())
            .unwrap()
            .fixture;
    for bytes in ["40aa", "66aa", "67aa", "2eaa", "64aa", "f2aa"] {
        fixture.memory[0].length = (bytes.len() / 2) as u32;
        fixture.memory[0].initial_bytes_hex = bytes.into();
        fixture.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        fixture.stop_rip = Some(HexU64(0x0010_0000 + (bytes.len() / 2) as u64));
        let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
        let initial_memory = memory.snapshot();
        let mut state = fixture.initial_state.to_cpu().unwrap();
        let initial_state = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(
                fixture.instruction_budget,
                fixture.stop_rip.map(HexU64::get),
            ),
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::Unsupported { opcode: 0xaa },
            "{bytes}"
        );
        assert_eq!(result.instructions_executed, 0, "{bytes}");
        assert_eq!(state, initial_state, "{bytes}");
        assert_eq!(memory.snapshot(), initial_memory, "{bytes}");
    }
}

#[test]
fn rep_stosb_fixtures_match_literal_progress_and_access_order() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, MovsbExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_19a_rep_stosb_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 5);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let original = input.initial_state.to_cpu().unwrap();
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&input).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        let expected_gprs = answer.gpr.map(HexU64::get);
        if let Some(fault) = &answer.fault {
            assert_eq!(fault.access, "write", "{case_id}");
            memory.inner.inject_once(
                fault.address.get(),
                AccessKind::Write,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(6),
                    linear_address: Some(fault.address.get()),
                }),
            );
        }
        let mut state = original.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        let count = original.gpr[Register::Rcx as usize];
        let initial_rdi = original.gpr[Register::Rdi as usize];
        let expected_addresses = if input.initial_state.rflags.get() & (1 << 10) == 0 {
            (0..count).map(|i| initial_rdi + i).collect::<Vec<_>>()
        } else {
            (0..count).map(|i| initial_rdi - i).collect::<Vec<_>>()
        };
        let mut expected_accesses = (0..2)
            .map(|offset| (AccessKind::Fetch, 0x0010_0000 + offset, 1))
            .collect::<Vec<_>>();
        expected_accesses.extend(
            expected_addresses
                .iter()
                .map(|address| (AccessKind::Write, *address, 1)),
        );
        assert_eq!(memory.accesses, expected_accesses, "{case_id}");
        if answer.fault.is_some() {
            assert_eq!(
                result.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(6),
                    linear_address: answer.fault.as_ref().map(|fault| fault.address.get()),
                }),
                "{case_id}"
            );
            assert_eq!(result.instructions_executed, 0, "{case_id}");
            assert_eq!(state.gpr, expected_gprs, "{case_id}");
            assert_eq!(state.rip, answer.rip.get(), "{case_id}");
            assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
            for region in memory.inner.snapshot() {
                assert_eq!(
                    region.bytes_hex,
                    answer.memory.get(&region.region_id).unwrap().as_str(),
                    "{case_id}: {}",
                    region.region_id
                );
            }
        } else {
            assert_eq!(result.exit, RunExit::StopAddress, "{case_id}");
            assert_eq!(result.instructions_executed, 1, "{case_id}");
            assert_eq!(state.gpr, expected_gprs, "{case_id}");
            assert_eq!(state.rip, answer.rip.get(), "{case_id}");
            assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
            for region in memory.inner.snapshot() {
                assert_eq!(
                    region.bytes_hex,
                    answer.memory.get(&region.region_id).unwrap().as_str(),
                    "{case_id}: {}",
                    region.region_id
                );
            }
        }
    }
}

#[test]
fn rep_stosb_stop_and_iteration_work_limit_preserve_restart_state() {
    let directory = arithmetic_fixture_directory();
    let mut fixture = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_df0.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let mut state = fixture.initial_state.to_cpu().unwrap();
    let start_rip = state.rip;
    let start_rdi = state.gpr[Register::Rdi as usize];
    let mut successful_boundaries = 0;
    let stopped = run(
        &mut state,
        &mut memory,
        config(1, fixture.stop_rip.map(HexU64::get)),
        || {
            successful_boundaries += 1;
            successful_boundaries == 3
        },
    );
    assert_eq!(stopped.exit, RunExit::StopRequested);
    assert_eq!(stopped.instructions_executed, 0);
    assert_eq!(state.rip, start_rip);
    assert_eq!(state.gpr[Register::Rcx as usize], 1);
    assert_eq!(state.gpr[Register::Rdi as usize], start_rdi + 2);

    let resumed = run(
        &mut state,
        &mut memory,
        config(1, fixture.stop_rip.map(HexU64::get)),
        || false,
    );
    assert_eq!(resumed.exit, RunExit::StopAddress);
    assert_eq!(resumed.instructions_executed, 1);
    assert_eq!(state.gpr[Register::Rcx as usize], 0);
    assert_eq!(state.gpr[Register::Rdi as usize], start_rdi + 3);
    assert_eq!(state.rip, fixture.stop_rip.unwrap().get());

    fixture.initial_state.gpr[Register::Rcx as usize] = HexU64(REP_ITERATION_WORK_LIMIT + 1);
    fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0020_0000);
    fixture.memory[1].length = (REP_ITERATION_WORK_LIMIT + 1) as u32;
    fixture.memory[1].initial_bytes_hex = "00".repeat((REP_ITERATION_WORK_LIMIT + 1) as usize);
    fixture.memory[1].comparison_mask_hex = "ff".repeat((REP_ITERATION_WORK_LIMIT + 1) as usize);
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let mut state = fixture.initial_state.to_cpu().unwrap();
    let start_rip = state.rip;
    let start_rdi = state.gpr[Register::Rdi as usize];
    let limited = run(
        &mut state,
        &mut memory,
        config(1, fixture.stop_rip.map(HexU64::get)),
        || false,
    );
    assert_eq!(limited.exit, RunExit::RepIterationBudgetExhausted);
    assert_eq!(limited.instructions_executed, 0);
    assert_eq!(state.rip, start_rip);
    assert_eq!(state.gpr[Register::Rcx as usize], 1);
    assert_eq!(
        state.gpr[Register::Rdi as usize],
        start_rdi + REP_ITERATION_WORK_LIMIT
    );
    assert!(
        memory.snapshot()[1]
            .bytes_hex
            .starts_with(&"88".repeat(REP_ITERATION_WORK_LIMIT as usize))
    );
    assert!(memory.snapshot()[1].bytes_hex.ends_with("00"));
    let resumed = run(
        &mut state,
        &mut memory,
        config(1, fixture.stop_rip.map(HexU64::get)),
        || false,
    );
    assert_eq!(resumed.exit, RunExit::StopAddress);
    assert_eq!(resumed.instructions_executed, 1);
    assert_eq!(state.gpr[Register::Rcx as usize], 0);
    assert_eq!(
        state.gpr[Register::Rdi as usize],
        start_rdi + REP_ITERATION_WORK_LIMIT + 1
    );
}

#[test]
fn rep_stosb_rejects_other_prefix_combinations_without_commit() {
    let directory = arithmetic_fixture_directory();
    let mut fixture = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    for bytes in ["f2aa", "f366aa", "f367aa", "f340aa"] {
        fixture.memory[0].length = (bytes.len() / 2) as u32;
        fixture.memory[0].initial_bytes_hex = bytes.into();
        fixture.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
        let mut state = fixture.initial_state.to_cpu().unwrap();
        let original_state = state.clone();
        let result = run(&mut state, &mut memory, config(1, None), || false);
        assert_eq!(
            result.exit,
            RunExit::Unsupported { opcode: 0xaa },
            "{bytes}"
        );
        assert_eq!(result.instructions_executed, 0, "{bytes}");
        assert_eq!(state, original_state, "{bytes}");
    }
}

#[test]
fn movsd_rejects_non_w_rex_rep_segment_and_address_prefixes_at_execution_boundary() {
    let directory = arithmetic_fixture_directory();
    let mut fixture =
        Fixture::from_json(&std::fs::read(directory.join("s2_18c_movsd_df0.case.json")).unwrap())
            .unwrap()
            .fixture;
    for (bytes, rip) in [
        ("40a5", 0x0010_0002),
        ("67a5", 0x0010_0002),
        ("6748a5", 0x0010_0003),
        ("26a5", 0x0010_0002),
        ("2e48a5", 0x0010_0003),
        ("2ea5", 0x0010_0002),
        ("36a5", 0x0010_0002),
        ("3ea5", 0x0010_0002),
        ("64a5", 0x0010_0002),
        ("65a5", 0x0010_0002),
        ("f2a5", 0x0010_0002),
        ("f3a5", 0x0010_0002),
    ] {
        fixture.memory[0].length = (bytes.len() / 2) as u32;
        fixture.memory[0].initial_bytes_hex = bytes.into();
        fixture.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        fixture.stop_rip = Some(HexU64(rip));
        let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
        let mut state = fixture.initial_state.to_cpu().unwrap();
        let original_state = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(
                fixture.instruction_budget,
                fixture.stop_rip.map(HexU64::get),
            ),
            || false,
        );
        assert!(
            matches!(result.exit, RunExit::Unsupported { .. }),
            "{bytes}"
        );
        assert_eq!(result.instructions_executed, 0, "{bytes}");
        assert_eq!(state, original_state, "{bytes}");
    }
}

#[test]
fn movsw_accepts_ignored_misplaced_rex_after_normalization() {
    let directory = arithmetic_fixture_directory();
    let mut fixture =
        Fixture::from_json(&std::fs::read(directory.join("s2_18b_movsw_df0.case.json")).unwrap())
            .unwrap()
            .fixture;
    let expected: std::collections::BTreeMap<String, MovsbExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_18b_movsw_expected.json")).unwrap(),
    )
    .unwrap();
    let answer = expected.get("s2_18b_movsw_df0").unwrap();
    fixture.memory[0].length = 3;
    fixture.memory[0].initial_bytes_hex = "4866a5".into();
    fixture.memory[0].comparison_mask_hex = "ffffff".into();
    fixture.stop_rip = Some(HexU64(0x0010_0003));
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let mut state = fixture.initial_state.to_cpu().unwrap();
    let result = run(
        &mut state,
        &mut memory,
        config(
            fixture.instruction_budget,
            fixture.stop_rip.map(HexU64::get),
        ),
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr, answer.gpr.map(HexU64::get));
    assert_eq!(state.rflags, answer.rflags.get());
    assert_eq!(state.rip, 0x0010_0003);
    assert_eq!(
        memory
            .snapshot()
            .into_iter()
            .find(|r| r.region_id == "data")
            .unwrap()
            .bytes_hex,
        answer.memory.get("data").unwrap().as_str()
    );
}

#[test]
fn movsb_rejects_prefixes_and_other_string_widths_without_commit() {
    for code in [
        &[0xf3, 0xa4][..],
        &[0x67, 0xa4][..],
        &[0x2e, 0xa4][..],
        &[0x66, 0xa4][..],
        &[0x48, 0xa4][..],
        &[0x66, 0xa4][..],
        &[0x67, 0x66, 0xa5][..],
        &[0x2e, 0x66, 0xa5][..],
        &[0xf3, 0x66, 0xa5][..],
    ] {
        let mut state = LongModeState {
            rip: 0x100000,
            gpr: [0; 16],
            rflags: 2,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
        };
        let before = state.clone();
        let outcome = step(
            &mut state,
            &mut memory(code, 0x100000),
            AccessContext::default(),
        )
        .unwrap();
        assert!(
            matches!(outcome, StepOutcome::Unsupported { .. }),
            "{code:02x?}"
        );
        assert_eq!(state, before, "{code:02x?}");
    }
}

#[test]
fn push_immediate16_fixtures_match_literal_words_boundaries_and_faults() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17h_push_immediate16_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 11);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        match case_id.as_str() {
            "s2_17h_push_fault_bottom" => memory.inject_once(
                0x002f_fffe,
                AccessKind::Write,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(6),
                    linear_address: Some(0x002f_fffe),
                }),
            ),
            "s2_17h_push_truncated_imm8" => memory.inject_once(
                0x0010_0002,
                AccessKind::Fetch,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    linear_address: Some(0x0010_0002),
                }),
            ),
            "s2_17h_push_truncated_imm16" => memory.inject_once(
                0x0010_0003,
                AccessKind::Fetch,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    linear_address: Some(0x0010_0003),
                }),
            ),
            _ => {}
        }
        let mut state = input.initial_state.to_cpu().unwrap();
        let before = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            result.instructions_executed, answer.instructions_executed,
            "{case_id}"
        );
        match (answer.exit.as_str(), answer.exception.as_ref()) {
            ("stop_address", None) => assert_eq!(result.exit, RunExit::StopAddress, "{case_id}"),
            ("exception", Some(expected)) => {
                assert_eq!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: expected.vector,
                        error_code: Some(expected.error_code),
                        linear_address: expected.linear_address.map(HexU64::get),
                    }),
                    "{case_id}"
                );
                assert_eq!(state, before, "{case_id}: no CPU commit");
            }
            other => panic!("invalid literal answer for {case_id}: {}", other.0),
        }
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn push_memory_fixtures_read_sources_before_bounded_stack_writes() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17i_push_memory_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 9);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        if case_id == "s2_17i_push_mem_read_fault" {
            memory.inject_once(
                0x0020_0000,
                AccessKind::Read,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    linear_address: Some(0x0020_0000),
                }),
            );
        }
        let mut state = input.initial_state.to_cpu().unwrap();
        let initial = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            result.instructions_executed, answer.instructions_executed,
            "{case_id}"
        );
        match (answer.exit.as_str(), answer.exception.as_ref()) {
            ("stop_address", None) => assert_eq!(result.exit, RunExit::StopAddress, "{case_id}"),
            ("instruction_budget_exhausted", None) => assert_eq!(
                result.exit,
                RunExit::InstructionBudgetExhausted,
                "{case_id}"
            ),
            ("exception", Some(expected)) => {
                assert_eq!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: expected.vector,
                        error_code: Some(expected.error_code),
                        linear_address: expected.linear_address.map(HexU64::get),
                    }),
                    "{case_id}"
                );
                assert_eq!(state, initial, "{case_id}: no CPU commit");
                assert_eq!(state.rip, expected.fault_rip.get(), "{case_id}");
            }
            other => panic!("invalid literal answer for {case_id}: {}", other.0),
        }
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn word_push_memory_fixtures_read_prestate_source_and_write_two_bounded_bytes() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17k_push_memory16_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 8);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        match case_id.as_str() {
            "s2_17k_push_mem_read_fault" => memory.inject_once(
                0x0020_0000,
                AccessKind::Read,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    linear_address: Some(0x0020_0000),
                }),
            ),
            "s2_17k_push_mem_stack_fault" => memory.inject_once(
                0x002f_ffff,
                AccessKind::Write,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(6),
                    linear_address: Some(0x002f_ffff),
                }),
            ),
            _ => {}
        }
        let mut state = input.initial_state.to_cpu().unwrap();
        let initial = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(
            result.exit,
            if let Some(expected) = &answer.exception {
                RunExit::Fault(ArchitecturalFault {
                    vector: expected.vector,
                    error_code: Some(expected.error_code),
                    linear_address: expected.linear_address.map(HexU64::get),
                })
            } else {
                RunExit::StopAddress
            },
            "{case_id}"
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            result.instructions_executed, answer.instructions_executed,
            "{case_id}"
        );
        match (answer.exit.as_str(), answer.exception.as_ref()) {
            ("stop_address", None) => assert_eq!(result.exit, RunExit::StopAddress, "{case_id}"),
            ("exception", Some(expected)) => {
                assert_eq!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: expected.vector,
                        error_code: Some(expected.error_code),
                        linear_address: expected.linear_address.map(HexU64::get),
                    }),
                    "{case_id}"
                );
                assert_eq!(state, initial, "{case_id}: no CPU commit");
            }
            other => panic!("invalid literal answer for {case_id}: {}", other.0),
        }
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn word_push_memory_bounded_stack_crossing_preserves_cpu_and_ram() {
    let directory = arithmetic_fixture_directory();
    let input = Fixture::from_json(
        &std::fs::read(directory.join("s2_17k_push_mem_base.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let mut memory = FlatMemory::from_fixture(&input).unwrap();
    let before_memory = memory.snapshot();
    let mut state = input.initial_state.to_cpu().unwrap();
    state.gpr[Register::Rsp as usize] = 0x0030_0001;
    let before_state = state.clone();
    let result = run(&mut state, &mut memory, config(1, None), || false);
    assert_eq!(
        result.exit,
        RunExit::MemoryError(MemoryError::Unmapped {
            address: 0x002f_ffff,
            access: AccessKind::Write
        })
    );
    assert_eq!(state, before_state);
    assert_eq!(memory.snapshot(), before_memory);
}

#[test]
fn address32_word_push_keeps_the_full_64_bit_stack_pointer() {
    let directory = arithmetic_fixture_directory();
    let mut input = Fixture::from_json(
        &std::fs::read(directory.join("s2_17k_push_mem_addr32.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let high_stack = 0x0000_0001_0030_0000;
    let high_rsp = high_stack + 0x78;
    input
        .memory
        .iter_mut()
        .find(|region| region.region_id == "stack")
        .unwrap()
        .guest_address = HexU64(high_stack);
    input.initial_state.gpr[Register::Rsp as usize] = HexU64(high_rsp);

    let mut memory = FlatMemory::from_fixture(&input).unwrap();
    let mut state = input.initial_state.to_cpu().unwrap();
    let result = run(&mut state, &mut memory, config(1, None), || false);

    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[Register::Rsp as usize], high_rsp - 2);
    assert_eq!(state.rip, 0x0010_0004);
    let stack = memory
        .snapshot()
        .into_iter()
        .find(|region| region.region_id == "stack")
        .unwrap();
    let bytes = decode_bytes(&stack.bytes_hex).unwrap();
    assert_eq!(&bytes[0x76..0x78], &[0xef, 0xbe]);
    assert!(bytes[..0x76].iter().all(|byte| *byte == 0xa5));
    assert!(bytes[0x78..].iter().all(|byte| *byte == 0xa5));
}

#[test]
fn pop_memory_fixtures_use_postincrement_rsp_and_full_literal_state() {
    let directory = arithmetic_fixture_directory();
    let mut raw_answers: serde_json::Value = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17j_pop_memory_expected.json")).unwrap(),
    )
    .unwrap();
    raw_answers
        .as_object_mut()
        .unwrap()
        .remove("s2_17j_pop_mem_store_pf");
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> =
        serde_json::from_value(raw_answers).unwrap();
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let mut state = input.initial_state.to_cpu().unwrap();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(result.instructions_executed, 1, "{case_id}");
        assert_eq!(result.exit, RunExit::StopAddress, "{case_id}");
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn word_pop_memory_fixtures_use_two_byte_stack_and_postincrement_destination() {
    let directory = arithmetic_fixture_directory();
    let mut raw_answers: serde_json::Value = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17l_pop_memory16_expected.json")).unwrap(),
    )
    .unwrap();
    raw_answers
        .as_object_mut()
        .unwrap()
        .remove("s2_17l_pop_mem_store_pf");
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> =
        serde_json::from_value(raw_answers).unwrap();
    assert_eq!(answers.len(), 6);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let mut state = input.initial_state.to_cpu().unwrap();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(result.instructions_executed, 1, "{case_id}");
        assert_eq!(result.exit, RunExit::StopAddress, "{case_id}");
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn memory_stack_rexw_overrides_66_for_qword_operands() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17m_memory_stack_rexw_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 4);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let mut state = input.initial_state.to_cpu().unwrap();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(result.instructions_executed, 1, "{case_id}");
        assert_eq!(result.exit, RunExit::StopAddress, "{case_id}");
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn pop_memory_store_fault_reports_pf_without_claiming_architectural_cpu_poststate() {
    let directory = arithmetic_fixture_directory();
    for case_id in ["s2_17j_pop_mem_store_pf", "s2_17l_pop_mem_store_pf"] {
        let raw = std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let input = Fixture::from_json(&raw).unwrap().fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let before_memory = memory.snapshot();
        memory.inject_once(
            0x0020_0000,
            AccessKind::Write,
            MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(7),
                linear_address: Some(0x0020_0000),
            }),
        );
        let mut state = input.initial_state.to_cpu().unwrap();
        let result = run(&mut state, &mut memory, config(1, None), || false);
        assert_eq!(
            result.exit,
            RunExit::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(7),
                linear_address: Some(0x0020_0000),
            }),
            "{case_id}"
        );
        assert_eq!(result.instructions_executed, 0, "{case_id}");
        // Intel SDM 093 does not state whether RSP increment is committed
        // when the destination store faults; deliberately make no CPU
        // post-state assertion for either operand width.
        assert_eq!(memory.snapshot(), before_memory, "{case_id}");
    }
}

#[test]
fn pop_memory_bounded_destination_crossing_preserves_ram_and_model_state() {
    let directory = arithmetic_fixture_directory();
    let raw = std::fs::read(directory.join("s2_17j_pop_mem_base.case.json")).unwrap();
    let input = Fixture::from_json(&raw).unwrap().fixture;
    let mut memory = FlatMemory::from_fixture(&input).unwrap();
    let before_memory = memory.snapshot();
    let mut state = input.initial_state.to_cpu().unwrap();
    state.gpr[Register::Rax as usize] = 0x0020_0004;
    let before_state = state.clone();
    let result = run(&mut state, &mut memory, config(1, None), || false);
    assert!(matches!(
        result.exit,
        RunExit::MemoryError(MemoryError::Unmapped {
            address: 0x0020_0008,
            access: AccessKind::Write
        })
    ));
    assert_eq!(state, before_state);
    assert_eq!(memory.snapshot(), before_memory);
}

#[test]
fn word_pop_memory_bounded_destination_crossing_preserves_ram_and_model_state() {
    let directory = arithmetic_fixture_directory();
    let raw = std::fs::read(directory.join("s2_17l_pop_mem_base.case.json")).unwrap();
    let input = Fixture::from_json(&raw).unwrap().fixture;
    let mut memory = FlatMemory::from_fixture(&input).unwrap();
    let before_memory = memory.snapshot();
    let mut state = input.initial_state.to_cpu().unwrap();
    state.gpr[Register::Rax as usize] = 0x0020_0001;
    let before_state = state.clone();
    let result = run(&mut state, &mut memory, config(1, None), || false);
    assert!(matches!(
        result.exit,
        RunExit::MemoryError(MemoryError::Unmapped {
            address: 0x0020_0002,
            access: AccessKind::Write
        })
    ));
    assert_eq!(state, before_state);
    assert_eq!(memory.snapshot(), before_memory);
}

#[test]
fn push_pop_stack_faults_and_bounds_leave_state_and_memory_uncommitted() {
    let directory = arithmetic_fixture_directory();
    for (case, access, address) in [
        ("s2_17a_push_rax", AccessKind::Write, 0x3000f8),
        ("s2_17a_pop_rax", AccessKind::Read, 0x3000f8),
        ("s2_17b_push_imm8_min", AccessKind::Write, 0x3000f8),
    ] {
        let raw = std::fs::read(directory.join(format!("{case}.case.json"))).unwrap();
        let fixture = Fixture::from_json(&raw).unwrap().fixture;
        for injected in [
            MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(if access == AccessKind::Write { 6 } else { 4 }),
                linear_address: Some(address),
            }),
            MemoryError::Backend("injected stack backend failure".into()),
        ] {
            let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
            let before_memory = memory.snapshot();
            memory.inject_once(address, access, injected.clone());
            let mut state = fixture.initial_state.to_cpu().unwrap();
            let before_state = state.clone();
            let result = run(
                &mut state,
                &mut memory,
                config(1, fixture.stop_rip.map(HexU64::get)),
                || false,
            );
            let expected = match injected {
                MemoryError::Architectural(fault) => RunExit::Fault(fault),
                error => RunExit::MemoryError(error),
            };
            assert_eq!(result.exit, expected, "{case}");
            assert_eq!(result.instructions_executed, 0, "{case}");
            assert_eq!(state, before_state, "{case}");
            assert_eq!(memory.snapshot(), before_memory, "{case}");
        }
    }

    for (code, rsp, access, address) in [
        (&[0x50][..], 0x300000, AccessKind::Write, 0x2ffff8),
        (&[0x6a, 0x80][..], 0x300000, AccessKind::Write, 0x2ffff8),
        (&[0x58][..], 0x300100, AccessKind::Read, 0x300100),
    ] {
        let raw = std::fs::read(directory.join("s2_17a_push_rax.case.json")).unwrap();
        let mut fixture = Fixture::from_json(&raw).unwrap().fixture;
        let code_region = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code_region.length = code.len() as u32;
        code_region.initial_bytes_hex = crate::memory::encode_bytes(code);
        code_region.comparison_mask_hex = "ff".repeat(code.len());
        fixture.stop_rip = Some(HexU64(0x100000 + code.len() as u64));
        fixture.initial_state.gpr[4] = HexU64(rsp);
        let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
        let before_memory = memory.snapshot();
        let mut state = fixture.initial_state.to_cpu().unwrap();
        let before_state = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(1, fixture.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::MemoryError(MemoryError::Unmapped { address, access })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before_state);
        assert_eq!(memory.snapshot(), before_memory);
    }
}

#[test]
fn push_pop_obey_instruction_budget_after_one_committed_instruction() {
    let directory = arithmetic_fixture_directory();
    let raw = std::fs::read(directory.join("s2_17a_push_rax.case.json")).unwrap();
    let fixture = Fixture::from_json(&raw).unwrap().fixture;
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let mut state = fixture.initial_state.to_cpu().unwrap();
    let result = run(&mut state, &mut memory, config(1, None), || false);
    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[4], 0x3000f8);
}

#[test]
fn push_pop_noncanonical_stack_ranges_raise_stack_fault_before_memory_access() {
    for (code, rsp) in [
        (&[0x50][..], 0x0000_8000_0000_0008), // PUSH range begins noncanonical.
        (&[0x58][..], 0x0000_8000_0000_0000), // POP range begins noncanonical.
        (&[0x58][..], 0x0000_7fff_ffff_ffff), // POP range crosses the canonical hole.
    ] {
        let mut memory = memory(code, 0x1000);
        let mut state = LongModeState::default();
        state.rip = 0x1000;
        state.gpr[4] = rsp;
        let before_state = state.clone();
        let result = run(&mut state, &mut memory, config(1, None), || false);
        assert_eq!(
            result.exit,
            RunExit::Fault(ArchitecturalFault {
                vector: 12,
                error_code: Some(0),
                linear_address: None,
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before_state);
    }
}

#[test]
fn truncated_push_immediates_fault_during_fetch_without_state_commit() {
    for code in [
        &[0x6a][..],
        &[0x68][..],
        &[0x68, 0][..],
        &[0x68, 0, 0][..],
        &[0x68, 0, 0, 0][..],
    ] {
        let mut memory = memory(code, 0x1000);
        let mut state = LongModeState::default();
        state.rip = 0x1000;
        state.gpr[4] = 0x300100;
        let before_state = state.clone();
        let result = run(&mut state, &mut memory, config(1, None), || false);
        assert_eq!(
            result.exit,
            RunExit::MemoryError(MemoryError::Unmapped {
                address: 0x1000 + code.len() as u64,
                access: AccessKind::Fetch,
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before_state);
    }
}

fn assert_literal_fixture_answers(sidecar: &str, count: usize) {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, ArithmeticExpected> =
        serde_json::from_slice(&std::fs::read(directory.join(sidecar)).unwrap()).unwrap();
    assert_eq!(
        answers.len(),
        count,
        "all selected forms must be materialized"
    );
    for (case_id, answer) in answers {
        assert!(
            case_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        );
        let raw = std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let input = Fixture::from_json(&raw).unwrap().fixture;
        assert_eq!(input.case_id, case_id);
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let mut state = input.initial_state.to_cpu().unwrap();
        let actual = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(actual.exit, RunExit::StopAddress, "{case_id}");
        assert_eq!(
            actual.instructions_executed, input.instruction_budget,
            "{case_id}"
        );
        assert_eq!(
            actual.rflags_defined,
            answer
                .rflags_defined
                .map(HexU64::get)
                .unwrap_or(od_isa::INTEGER_FLAGS),
            "{case_id}"
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(
            state.rflags & input.rflags_mask.get(),
            answer.rflags.get() & input.rflags_mask.get(),
            "{case_id}"
        );
        assert_eq!(state.rip, input.stop_rip.unwrap().get(), "{case_id}");
        for region in memory.snapshot() {
            let before = input
                .memory
                .iter()
                .find(|r| r.region_id == region.region_id)
                .unwrap();
            let expected = answer
                .memory
                .get(&region.region_id)
                .unwrap_or(&before.initial_bytes_hex);
            assert_eq!(
                &region.bytes_hex, expected,
                "{case_id}: {}",
                region.region_id
            );
        }
    }
}

#[test]
fn arithmetic_cross_region_read_and_write_faults_commit_neither_flags_nor_memory() {
    let directory = arithmetic_fixture_directory();
    for op in ["add", "adc", "sub", "sbb", "inc", "dec", "neg"] {
        let suffix = if matches!(op, "inc" | "dec" | "neg") {
            "mem"
        } else {
            "mem_reg"
        };
        let raw =
            std::fs::read(directory.join(format!("s2_11_{op}_64_{suffix}.case.json"))).unwrap();
        let input = Fixture::from_json(&raw).unwrap().fixture;
        for access in [AccessKind::Read, AccessKind::Write] {
            let fault = ArchitecturalFault {
                vector: 14,
                error_code: Some(if access == AccessKind::Write { 6 } else { 4 }),
                linear_address: Some(0x201000),
            };
            for error in [
                MemoryError::Architectural(fault),
                MemoryError::Backend("injected arithmetic backend fault".into()),
            ] {
                let mut memory = FlatMemory::from_fixture(&input).unwrap();
                let before_memory = memory.snapshot();
                memory.inject_once(0x201000, access, error.clone());
                let mut state = input.initial_state.to_cpu().unwrap();
                let before_state = state.clone();
                let actual = run(
                    &mut state,
                    &mut memory,
                    config(1, input.stop_rip.map(HexU64::get)),
                    || false,
                );
                let expected = match error {
                    MemoryError::Architectural(fault) => RunExit::Fault(fault),
                    error => RunExit::MemoryError(error),
                };
                assert_eq!(actual.exit, expected, "{op}: {access:?}");
                assert_eq!(actual.instructions_executed, 0);
                assert_eq!(state, before_state, "{op}: {access:?}");
                assert_eq!(memory.snapshot(), before_memory, "{op}: {access:?}");
            }
        }
    }
}

#[test]
fn logic_faults_preserve_definedness_and_all_state_until_retirement() {
    let directory = arithmetic_fixture_directory();
    for op in ["and", "or", "xor", "test", "not"] {
        let suffix = if op == "not" { "mem" } else { "mem_reg" };
        let raw =
            std::fs::read(directory.join(format!("s2_12_{op}_64_{suffix}.case.json"))).unwrap();
        let input = Fixture::from_json(&raw).unwrap().fixture;
        for access in [AccessKind::Read, AccessKind::Write] {
            if op == "test" && access == AccessKind::Write {
                continue; // TEST has no write; readonly fixtures check that separately.
            }
            let fault = ArchitecturalFault {
                vector: 14,
                error_code: Some(if access == AccessKind::Write { 6 } else { 4 }),
                linear_address: Some(0x201000),
            };
            for error in [
                MemoryError::Architectural(fault),
                MemoryError::Backend("injected logic backend fault".into()),
            ] {
                let mut memory = FlatMemory::from_fixture(&input).unwrap();
                let before_memory = memory.snapshot();
                memory.inject_once(0x201000, access, error.clone());
                let mut state = input.initial_state.to_cpu().unwrap();
                let before_state = state.clone();
                let actual = run(
                    &mut state,
                    &mut memory,
                    config(1, input.stop_rip.map(HexU64::get)),
                    || false,
                );
                let expected = match error {
                    MemoryError::Architectural(fault) => RunExit::Fault(fault),
                    error => RunExit::MemoryError(error),
                };
                assert_eq!(actual.exit, expected, "{op}: {access:?}");
                assert_eq!(actual.instructions_executed, 0);
                assert_eq!(
                    actual.rflags_defined,
                    od_isa::INTEGER_FLAGS,
                    "{op}: {access:?}"
                );
                assert_eq!(state, before_state, "{op}: {access:?}");
                assert_eq!(memory.snapshot(), before_memory, "{op}: {access:?}");
            }
        }
    }
}

#[test]
fn memory_mov_faults_preserve_precise_state_and_never_partially_store() {
    let input = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
    ))
    .unwrap()
    .fixture;
    for kind in [AccessKind::Read, AccessKind::Write] {
        let fault = ArchitecturalFault {
            vector: 14,
            error_code: Some(if kind == AccessKind::Write { 6 } else { 4 }),
            linear_address: Some(0x1000),
        };
        for error in [
            MemoryError::Architectural(fault.clone()),
            MemoryError::Backend("injected S2-08 backend failure".into()),
        ] {
            let mut mem = FlatMemory::from_fixture(&input).unwrap();
            let ram = mem.snapshot();
            mem.inject_once(0x1000, kind, error.clone());
            let mut state = input.initial_state.to_cpu().unwrap();
            let mut expected = state.clone();
            if kind == AccessKind::Write {
                expected.gpr[0] = 12;
                expected.rflags = 0x406;
                expected.rip = 0x4007;
            }
            let actual = run(
                &mut state,
                &mut mem,
                config(32, input.stop_rip.map(HexU64::get)),
                || false,
            );
            assert_eq!(
                actual.exit,
                if let MemoryError::Architectural(fault) = error {
                    RunExit::Fault(fault)
                } else {
                    RunExit::MemoryError(error)
                }
            );
            assert_eq!(
                actual.instructions_executed,
                if kind == AccessKind::Write { 2 } else { 0 }
            );
            assert_eq!(state, expected);
            assert_eq!(mem.snapshot(), ram);
        }
    }
}

#[test]
fn memory_load_aliases_have_independent_expected_intermediate_values() {
    let input = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/mov_width_aliasing.case.json"
    ))
    .unwrap()
    .fixture;
    let mut mem = FlatMemory::from_fixture(&input).unwrap();
    let mut state = input.initial_state.to_cpu().unwrap();
    for (rax, rip) in [
        (0x1122_3344_5566_7711, 0x4002),
        (0x1122_3344_5566_7711, 0x4005),
        (0x1122_3344_5566_2211, 0x4008),
        (0x1122_3344_5566_2211, 0x400b),
        (0x1122_3344_5566_4433, 0x400f),
        (0x1122_3344_5566_4433, 0x4013),
        (0x0000_0000_8877_6655, 0x4016),
        (0x0000_0000_8877_6655, 0x4019),
    ] {
        assert_eq!(
            step(&mut state, &mut mem, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.gpr[0], rax);
        assert_eq!(state.rip, rip);
        assert_eq!(state.rflags, 0x402);
        assert_eq!(
            &state.gpr[1..],
            &input.initial_state.to_cpu().unwrap().gpr[1..]
        );
    }
}

#[test]
fn on_demand_execution_skips_invalid_tail_and_accepts_arbitrary_reached_entry() {
    // JZ skips an unsupported byte. The MOV is the last mapped instruction.
    let code = [0x74, 0x01, 0x90, 0x48, 0xb8, 1, 2, 3, 4, 5, 6, 7, 8];
    let base = 0x1000;
    let mut mem = memory(&code, base);
    let mut state = LongModeState {
        rip: base,
        rflags: 0x42,
        ..LongModeState::default()
    };
    let result = run(
        &mut state,
        &mut mem,
        config(10, Some(base + code.len() as u64)),
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.gpr[0], 0x0807_0605_0403_0201);
    assert_eq!(state.rflags, 0x42);
    state.rip = base + 3;
    let result = run(&mut state, &mut mem, config(1, None), || false);
    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    // Mapping end is not an implicit CPU stop address.
    let result = run(&mut state, &mut mem, config(1, None), || false);
    assert!(matches!(
        result.exit,
        RunExit::MemoryError(MemoryError::Unmapped {
            access: AccessKind::Fetch,
            ..
        })
    ));
    assert_eq!(result.instructions_executed, 0);
}

#[test]
fn s2_22a_fs_base_fixture_executes_alias_load_and_observes_profile_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_22a_fs_base_qword_load_alias.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let result = run(
        &mut state,
        &mut memory,
        config(
            fixture.instruction_budget,
            fixture.stop_rip.map(HexU64::get),
        ),
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[0], 0x8877_6655_4433_2211);
    assert_eq!(state.gpr[1..], before.gpr[1..]);
    assert_eq!(state.rip, 0x0010_0004);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(state.fs_base, 0xffff_8000_0000_0000);
    assert_eq!(memory.snapshot()[1].guest_address.get(), 0x0040_0000);
}

#[test]
fn s2_22b_gs_base_fixture_executes_and_faults_before_commit() {
    let mut fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_22b_gs_base_qword_load_alias.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    assert_eq!(
        run(
            &mut state,
            &mut memory,
            config(
                fixture.instruction_budget,
                fixture.stop_rip.map(HexU64::get)
            ),
            || false,
        )
        .exit,
        RunExit::StopAddress
    );
    assert_eq!(state.gpr[0], 0x8877_6655_4433_2211);
    assert_eq!(state.gs_base, 0xffff_8000_0000_0000);
    assert_eq!(memory.snapshot()[1].guest_address.get(), 0x0040_0000);

    fixture.initial_state.gpr[0] = HexU64(0x0000_7fff_ffff_fff0);
    fixture.initial_state.gs_base = Some(HexU64(0x1000));
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let result = run(&mut state, &mut memory, config(1, None), || false);
    assert!(matches!(
        result.exit,
        RunExit::Fault(ArchitecturalFault { vector: 13, .. })
    ));
    assert_eq!(state, before);

    fixture.initial_state.gpr[0] = HexU64(0x0000_0000_0050_0000);
    fixture.initial_state.gs_base = Some(HexU64(0x1000));
    fixture.memory[1].guest_address = HexU64(0x0000_0000_0060_0000);
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    memory.inject_once(
        0x0000_0000_0050_1000,
        AccessKind::Read,
        MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(0x0000_0000_0050_1000),
        }),
    );
    let result = run(&mut state, &mut memory, config(1, None), || false);
    assert!(
        matches!(
            result.exit,
            RunExit::Fault(ArchitecturalFault { vector: 14, .. })
        ),
        "unexpected GS-base unmapped read exit: {:?}",
        result.exit
    );
    assert_eq!(state, before);
}

#[test]
fn backwards_branches_budget_and_stop_have_precise_boundaries() {
    let code = [0x48, 0x2b, 0xc1, 0x75, 0xfb];
    let mut mem = memory(&code, 0);
    let mut state = LongModeState::default();
    state.gpr[0] = 3;
    state.gpr[1] = 1;
    let result = run(&mut state, &mut mem, config(3, Some(5)), || false);
    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(state.rip, 3);
    assert_eq!(state.gpr[0], 1);
    let result = run(&mut state, &mut mem, config(3, Some(5)), || false);
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(state.gpr[0], 0);
    assert_eq!(state.rip, 5);
    state.rip = 0;
    let before = state.clone();
    let result = run(&mut state, &mut mem, config(0, None), || true);
    assert_eq!(result.exit, RunExit::StopRequested);
    assert_eq!(state, before);
    let result = run(&mut state, &mut mem, config(0, None), || false);
    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(state, before);
}

#[test]
fn failed_fetch_and_unsupported_encoding_never_commit_cpu_or_ram() {
    let code = [0x48, 0xb8, 1, 2, 3, 4, 5, 6, 7, 8];
    for length in 1..code.len() {
        let mut mem = memory(&code[..length], 0);
        let mut state = LongModeState {
            gpr: [u64::MAX; 16],
            ..LongModeState::default()
        };
        let before = state.clone();
        let ram = mem.snapshot();
        assert!(matches!(
            step(&mut state, &mut mem, AccessContext::default()),
            Err(MemoryError::Unmapped { .. })
        ));
        assert_eq!(state, before);
        assert_eq!(mem.snapshot(), ram);
    }
    for code in [
        &[0x0f, 0x05][..],
        &[0x64, 0x48, 0x8b, 0x03][..],
        &[0xf0, 0x48, 0x03, 0xc4][..],
    ] {
        let mut mem = memory(code, 0);
        let mut state = LongModeState::default();
        let before = state.clone();
        assert!(matches!(
            step(&mut state, &mut mem, AccessContext::default()),
            Ok(StepOutcome::Unsupported { .. })
        ));
        assert_eq!(state, before);
    }
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(4),
        linear_address: Some(5),
    };
    let mut mem = memory(&code, 0);
    mem.inject_once(
        5,
        AccessKind::Fetch,
        MemoryError::Architectural(fault.clone()),
    );
    let mut state = LongModeState::default();
    let before = state.clone();
    let result = run(&mut state, &mut mem, config(10, None), || false);
    assert_eq!(result.exit, RunExit::Fault(fault));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    state.rip = 0x0000_8000_0000_0000;
    assert!(matches!(
        step(&mut state, &mut mem, AccessContext::default()),
        Ok(StepOutcome::Fault(ArchitecturalFault {
            vector: 13,
            error_code: Some(0),
            ..
        }))
    ));
}

#[test]
fn old_forms_use_the_same_arithmetic_and_preserve_extra_registers() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86");
    for name in [
        "s0_09_mov_imm64",
        "s0_09_add_overflow",
        "s0_09_cmp_jz",
        "s0_09_sub_jnz_loop",
    ] {
        let old = crate::case::Fixture::from_json(
            &std::fs::read(root.join(format!("{name}.case.json"))).unwrap(),
        )
        .unwrap();
        let converted = Fixture::convert_v1(&old).unwrap();
        let code = old.fixture.program.bytes().unwrap();
        let mut state = converted.initial_state.to_cpu().unwrap();
        let mut expected = od_isa::FragmentState::try_from(&state).unwrap();
        let fragment =
            od_isa::interpret(&code, &mut expected, converted.instruction_budget).unwrap();
        let mut mem = FlatMemory::from_fixture(&converted).unwrap();
        state.gpr[15] = u64::MAX;
        let result = run(
            &mut state,
            &mut mem,
            config(
                converted.instruction_budget,
                converted.stop_rip.map(HexU64::get),
            ),
            || false,
        );
        assert_eq!(result.exit, RunExit::StopAddress);
        assert_eq!(result.instructions_executed, fragment.instructions_executed);
        assert_eq!(&state.gpr[..4], &fragment.state.gpr);
        assert_eq!(state.rip, fragment.state.rip as u64);
        assert_eq!(state.rflags, fragment.state.rflags);
        assert_eq!(state.gpr[15], u64::MAX);
    }
}
