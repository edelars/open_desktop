use od_isa::memory::{AccessContext, AccessKind, AccessOrigin, InstructionMode, PrivilegeLevel};
use od_isa::{CpuActivity, INTEGER_FLAGS, Register};
use od_machine::{
    AccessWidth, GuestPhysicalAddress, PagedProbeExit, STAGE4_FLAT_PROBE_IMAGE_BYTES,
    STAGE4_FLAT_PROBE_IMAGE_GPA, Stage4FlatProbeLayoutError, Stage4FlatProbeLoadError,
    TrackedAccessError, load_stage4_stack_probe,
};

const PAYLOAD: u64 = 0x8877_6655_4433_2211;
const CODE: [u8; 4] = [0x51, 0x58, 0xee, 0xf4];

fn image(payload: u64) -> Vec<u8> {
    let mut bytes = vec![0; STAGE4_FLAT_PROBE_IMAGE_BYTES];
    for (gpa, pte) in [
        (0x1000, 0x2003),
        (0x2000, 0x3003),
        (0x3010, 0x4003),
        (0x3020, 0x5003),
        (0x3028, 0x6003),
        (0x4000, 0x7001),
        (0x5000, 0x9003 | (1 << 63)),
        (0x6000, 0xa003 | (1 << 63)),
    ] {
        put(
            &mut bytes,
            (gpa - STAGE4_FLAT_PROBE_IMAGE_GPA) as usize,
            pte,
        );
    }
    bytes[0x6000..0x6004].copy_from_slice(&CODE);
    put(&mut bytes, 0x8000, payload);
    bytes
}

fn put(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn qword(machine: &mut od_machine::LoadedStage4FlatProbe, gpa: u64) -> u64 {
    machine
        .bus_mut()
        .read_physical(GuestPhysicalAddress::new(gpa), AccessWidth::Qword)
        .unwrap()
}

fn set_qword(machine: &mut od_machine::LoadedStage4FlatProbe, gpa: u64, value: u64) {
    machine
        .bus_mut()
        .write_physical(
            GuestPhysicalAddress::new(gpa),
            AccessWidth::Qword,
            value,
            |_| {},
        )
        .unwrap();
}

fn assert_page_fault(
    report: &od_machine::PagedProbeQuantumReport,
    saved_rip: u64,
    access: AccessKind,
    error_code: u32,
    linear_address: u64,
) {
    let record = report.page_fault.as_ref().expect("tracked page fault");
    assert_eq!(record.saved_rip(), saved_rip);
    assert_eq!(record.access(), access);
    assert_eq!(record.cr2_value(), linear_address);
    assert_eq!(record.fault().error_code, Some(error_code));
    assert_eq!(record.fault().linear_address, Some(linear_address));
    assert_eq!(
        record.context(),
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        }
    );
}

#[test]
fn stack_probe_executes_across_bounded_calls_and_halted_repeat_is_inert() {
    let mut machine = load_stage4_stack_probe(&image(PAYLOAD)).unwrap();
    assert_eq!(machine.state().gpr[Register::Rax as usize], 0);
    assert_eq!(machine.state().gpr[Register::Rcx as usize], PAYLOAD);
    assert_eq!(machine.state().gpr[Register::Rsp as usize], 0x00a0_1000);
    assert_eq!(machine.rflags_defined(), INTEGER_FLAGS);

    let lower_canary = 0xcafe_babe_0123_4567;
    let upper_canary = 0x7654_3210_dead_beef;
    set_qword(&mut machine, 0xaff0, lower_canary);
    set_qword(&mut machine, 0xb000, upper_canary);
    let empty_uart = machine.uart();
    assert!(!empty_uart.transfer_holding_to_shift());

    let mut committed = Vec::new();
    let first = machine.run_stack(
        1,
        || false,
        |range| committed.push((range.start().get(), range.len())),
    );
    assert_eq!(first.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(first.instructions_executed, 1);
    assert_eq!(machine.state().gpr[Register::Rsp as usize], 0x00a0_0ff8);
    assert_eq!(qword(&mut machine, 0xaff8), PAYLOAD);
    assert_eq!(qword(&mut machine, 0xaff0), lower_canary);
    assert_eq!(qword(&mut machine, 0xb000), upper_canary);
    assert_eq!(
        committed,
        vec![
            (0x1000, 8),
            (0x2000, 8),
            (0x3010, 8),
            (0x4000, 8),
            (0x3028, 8),
            (0x6000, 8),
            (0x6000, 8),
            (0xaff8, 8),
        ]
    );
    for gpa in [0x1000, 0x2000, 0x3010, 0x4000, 0x3028] {
        assert_ne!(qword(&mut machine, gpa) & (1 << 5), 0, "A bit at {gpa:#x}");
    }
    assert_eq!(
        qword(&mut machine, 0x6000) & ((1 << 5) | (1 << 6)),
        (1 << 5) | (1 << 6)
    );

    let second = machine.run_stack(1, || false, |_| {});
    assert_eq!(second.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(second.instructions_executed, 1);
    assert_eq!(machine.state().gpr[Register::Rax as usize], PAYLOAD);
    assert_eq!(machine.state().gpr[Register::Rsp as usize], 0x00a0_1000);

    let third = machine.run_stack(1, || false, |_| {});
    assert_eq!(third.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(third.instructions_executed, 1);
    let uart = machine.uart();
    assert!(uart.transfer_holding_to_shift());
    assert_eq!(uart.complete_transmission(), Some(PAYLOAD as u8));

    let fourth = machine.run_stack(1, || false, |_| {});
    assert_eq!(fourth.exit, PagedProbeExit::Halted);
    assert_eq!(fourth.instructions_executed, 1);
    assert_eq!(machine.state().rip, 0x0040_0004);
    assert_eq!(machine.state().activity, Some(CpuActivity::Halted));
    assert_eq!(machine.state().gpr[Register::Rax as usize], PAYLOAD);
    assert_eq!(machine.state().gpr[Register::Rsp as usize], 0x00a0_1000);
    assert_eq!(qword(&mut machine, 0xaff8), PAYLOAD);
    assert_eq!(qword(&mut machine, 0xaff0), lower_canary);
    assert_eq!(qword(&mut machine, 0xb000), upper_canary);

    let state = machine.state().clone();
    let mut callbacks = 0;
    let repeated = machine.run_stack(4, || false, |_| callbacks += 1);
    assert_eq!(repeated.exit, PagedProbeExit::Halted);
    assert_eq!(repeated.instructions_executed, 0);
    assert_eq!(machine.state(), &state);
    assert_eq!(callbacks, 0);
    assert!(!uart.transfer_holding_to_shift());
}

#[test]
fn budget_zero_and_each_cli_budget_boundary_are_respected_by_quantum() {
    for budget in 0..=4 {
        let mut machine = load_stage4_stack_probe(&image(PAYLOAD)).unwrap();
        let report = machine.run_stack(budget, || false, |_| {});
        assert_eq!(report.instructions_executed, budget);
        if budget == 4 {
            assert_eq!(report.exit, PagedProbeExit::Halted);
        } else {
            assert_eq!(report.exit, PagedProbeExit::BudgetExhausted);
        }
        assert_eq!(
            machine.state().gpr[Register::Rax as usize],
            if budget >= 2 { PAYLOAD } else { 0 }
        );
        assert_eq!(
            machine.state().gpr[Register::Rsp as usize],
            if budget == 1 {
                0x00a0_0ff8
            } else {
                0x00a0_1000
            }
        );
        assert_eq!(
            qword(&mut machine, 0xaff8),
            if budget >= 1 { PAYLOAD } else { 0 }
        );
        let uart = machine.uart();
        assert_eq!(uart.transfer_holding_to_shift(), budget >= 3);
        if budget >= 3 {
            assert_eq!(uart.complete_transmission(), Some(PAYLOAD as u8));
        }
    }
}

#[test]
fn rejects_corrupt_instruction_page_table_and_image_size() {
    let mut wrong_code = image(PAYLOAD);
    wrong_code[0x6000] ^= 1;
    assert!(matches!(
        load_stage4_stack_probe(&wrong_code),
        Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::CodeBytes { .. }
        ))
    ));

    let mut wrong_table = image(PAYLOAD);
    wrong_table[0x5000] ^= 1;
    assert!(matches!(
        load_stage4_stack_probe(&wrong_table),
        Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::PageTableEntry { .. }
        ))
    ));
    assert!(matches!(
        load_stage4_stack_probe(&image(PAYLOAD)[..STAGE4_FLAT_PROBE_IMAGE_BYTES - 1]),
        Err(Stage4FlatProbeLoadError::ImageSize { .. })
    ));
}

#[test]
fn stack_faults_preserve_instruction_register_commit_boundaries() {
    let mut nonpresent_push = load_stage4_stack_probe(&image(PAYLOAD)).unwrap();
    set_qword(&mut nonpresent_push, 0x6000, 0);
    let before = nonpresent_push.state().clone();
    let mut committed = Vec::new();
    let push_fault = nonpresent_push.run_stack(
        1,
        || false,
        |range| committed.push((range.start().get(), range.len())),
    );
    assert_eq!(
        push_fault.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(2),
            linear_address: Some(0x00a0_0ff8),
        })
    );
    assert_eq!(push_fault.instructions_executed, 0);
    assert_eq!(nonpresent_push.state(), &before);
    assert_eq!(push_fault.rflags_defined, INTEGER_FLAGS);
    assert_page_fault(&push_fault, 0x0040_0000, AccessKind::Write, 2, 0x00a0_0ff8);
    assert_eq!(
        committed,
        vec![
            (0x1000, 8),
            (0x2000, 8),
            (0x3010, 8),
            (0x4000, 8),
            (0x3028, 8)
        ]
    );
    assert_eq!(qword(&mut nonpresent_push, 0xaff8), 0);

    let mut readonly_push = load_stage4_stack_probe(&image(PAYLOAD)).unwrap();
    set_qword(&mut readonly_push, 0x6000, 0xa001 | (1 << 63));
    let before = readonly_push.state().clone();
    let mut committed = Vec::new();
    let readonly_fault = readonly_push.run_stack(
        1,
        || false,
        |range| committed.push((range.start().get(), range.len())),
    );
    assert_eq!(
        readonly_fault.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(3),
            linear_address: Some(0x00a0_0ff8),
        })
    );
    assert_eq!(readonly_push.state(), &before);
    assert_eq!(readonly_fault.instructions_executed, 0);
    assert_eq!(readonly_fault.rflags_defined, INTEGER_FLAGS);
    assert_page_fault(
        &readonly_fault,
        0x0040_0000,
        AccessKind::Write,
        3,
        0x00a0_0ff8,
    );
    assert_eq!(
        committed,
        vec![
            (0x1000, 8),
            (0x2000, 8),
            (0x3010, 8),
            (0x4000, 8),
            (0x3028, 8),
            (0x6000, 8)
        ]
    );
    assert_ne!(qword(&mut readonly_push, 0x6000) & (1 << 5), 0);
    assert_eq!(qword(&mut readonly_push, 0x6000) & (1 << 6), 0);
    assert_eq!(qword(&mut readonly_push, 0xaff8), 0);

    let mut nonpresent_pop = load_stage4_stack_probe(&image(PAYLOAD)).unwrap();
    assert_eq!(
        nonpresent_pop.run_stack(1, || false, |_| {}).exit,
        PagedProbeExit::BudgetExhausted
    );
    set_qword(&mut nonpresent_pop, 0x6000, 0);
    let before = nonpresent_pop.state().clone();
    let pop_fault = nonpresent_pop.run_stack(1, || false, |_| {});
    assert_eq!(
        pop_fault.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(0x00a0_0ff8),
        })
    );
    assert_eq!(pop_fault.instructions_executed, 0);
    assert_eq!(nonpresent_pop.state(), &before);
    assert_eq!(pop_fault.rflags_defined, INTEGER_FLAGS);
    assert_page_fault(&pop_fault, 0x0040_0001, AccessKind::Read, 0, 0x00a0_0ff8);
    assert_eq!(qword(&mut nonpresent_pop, 0xaff8), PAYLOAD);
}

#[test]
fn stack_quantum_stop_resume_and_negative_boundaries_are_explicit() {
    let mut stopped = load_stage4_stack_probe(&image(PAYLOAD)).unwrap();
    let before = stopped.state().clone();
    let mut callbacks = 0;
    let stopped_report = stopped.run_stack(4, || true, |_| callbacks += 1);
    assert_eq!(stopped_report.exit, PagedProbeExit::Stopped);
    assert_eq!(stopped_report.instructions_executed, 0);
    assert_eq!(stopped.state(), &before);
    assert_eq!(callbacks, 0);

    let resumed = stopped.run_stack(1, || false, |_| {});
    assert_eq!(resumed.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(resumed.instructions_executed, 1);

    for retired_before_stop in [1, 2] {
        let mut machine = load_stage4_stack_probe(&image(PAYLOAD)).unwrap();
        let mut boundary_calls = 0;
        let mut committed = Vec::new();
        let stopped = machine.run_stack(
            4,
            || {
                let stop_now = boundary_calls == retired_before_stop;
                boundary_calls += 1;
                stop_now
            },
            |range| committed.push((range.start().get(), range.len())),
        );
        assert_eq!(stopped.exit, PagedProbeExit::Stopped);
        assert_eq!(stopped.instructions_executed, retired_before_stop);
        let stopped_state = machine.state().clone();
        let stopped_stack = qword(&mut machine, 0xaff8);
        let committed_before_resume = committed.clone();
        assert_eq!(stopped_stack, PAYLOAD);

        let resumed = machine.run_stack(
            4,
            || false,
            |range| committed.push((range.start().get(), range.len())),
        );
        assert_eq!(resumed.exit, PagedProbeExit::Halted);
        assert_eq!(resumed.instructions_executed, 4 - retired_before_stop);
        assert!(committed.starts_with(&committed_before_resume));
        assert_eq!(
            committed.iter().filter(|&&(gpa, _)| gpa == 0xaff8).count(),
            1
        );
        assert_eq!(machine.state().activity, Some(CpuActivity::Halted));
        assert_eq!(machine.state().gpr[Register::Rax as usize], PAYLOAD);
        assert_eq!(machine.state().gpr[Register::Rsp as usize], 0x00a0_1000);
        if retired_before_stop == 1 {
            assert_eq!(stopped_state.gpr[Register::Rsp as usize], 0x00a0_0ff8);
            assert_eq!(stopped_state.rip, 0x0040_0001);
        } else {
            assert_eq!(stopped_state.gpr[Register::Rsp as usize], 0x00a0_1000);
            assert_eq!(stopped_state.gpr[Register::Rax as usize], PAYLOAD);
            assert_eq!(stopped_state.rip, 0x0040_0002);
        }
    }

    for code in [0xf4_ee_58_50u64, 0xf4_ee_51_48u64] {
        let mut machine = load_stage4_stack_probe(&image(PAYLOAD)).unwrap();
        set_qword(&mut machine, 0x7000, code);
        let before = machine.state().clone();
        let report = machine.run_stack(1, || false, |_| {});
        assert!(matches!(report.exit, PagedProbeExit::Unsupported { .. }));
        assert_eq!(report.instructions_executed, 0);
        assert_eq!(machine.state(), &before);
    }
}

#[test]
fn pop_read_sets_only_accessed_and_preserves_stack_operand() {
    let mut machine = load_stage4_stack_probe(&image(PAYLOAD)).unwrap();
    assert_eq!(
        machine.run_stack(1, || false, |_| {}).exit,
        PagedProbeExit::BudgetExhausted
    );
    set_qword(&mut machine, 0x6000, 0xa003 | (1 << 63));
    let mut committed = Vec::new();
    let report = machine.run_stack(
        1,
        || false,
        |range| committed.push((range.start().get(), range.len())),
    );
    assert_eq!(report.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(report.instructions_executed, 1);
    assert_eq!(machine.state().gpr[Register::Rax as usize], PAYLOAD);
    assert_eq!(machine.state().gpr[Register::Rsp as usize], 0x00a0_1000);
    assert_eq!(qword(&mut machine, 0xaff8), PAYLOAD);
    assert_eq!(qword(&mut machine, 0x6000) & ((1 << 5) | (1 << 6)), 1 << 5);
    assert_eq!(committed, vec![(0x6000, 8)]);
}

#[test]
fn stack_address_faults_cross_page_refusal_and_backend_failure_are_bounded() {
    let mut noncanonical = load_stage4_stack_probe(&image(PAYLOAD)).unwrap();
    noncanonical.state_mut().gpr[Register::Rsp as usize] = 0x0000_8000_0000_0008;
    let before = noncanonical.state().clone();
    let fault = noncanonical.run_stack(1, || false, |_| {});
    assert_eq!(
        fault.exit,
        PagedProbeExit::Fault(od_isa::ArchitecturalFault {
            vector: 12,
            error_code: Some(0),
            linear_address: None,
        })
    );
    assert_eq!(fault.instructions_executed, 0);
    assert_eq!(noncanonical.state(), &before);
    assert!(fault.page_fault.is_none());

    let mut crossing = load_stage4_stack_probe(&image(PAYLOAD)).unwrap();
    crossing.state_mut().gpr[Register::Rsp as usize] = 0x00a0_1004;
    let before = crossing.state().clone();
    let crossing_report = crossing.run_stack(1, || false, |_| {});
    assert!(matches!(
        crossing_report.exit,
        PagedProbeExit::Backend(od_isa::memory::MemoryError::Backend(_))
    ));
    assert!(matches!(
        crossing_report.memory_error,
        Some(TrackedAccessError::CrossesPage { address, width })
            if address.get() == 0x00a0_0ffc && width == 8
    ));
    assert_eq!(crossing.state(), &before);

    let mut backend = load_stage4_stack_probe(&image(PAYLOAD)).unwrap();
    set_qword(&mut backend, 0x6000, 0x20_003);
    let before = backend.state().clone();
    let backend_report = backend.run_stack(1, || false, |_| {});
    assert!(matches!(
        backend_report.exit,
        PagedProbeExit::Backend(od_isa::memory::MemoryError::Backend(_))
    ));
    assert!(matches!(
        backend_report.memory_error,
        Some(TrackedAccessError::OperandPhysical { .. })
    ));
    assert_eq!(backend.state(), &before);
}
