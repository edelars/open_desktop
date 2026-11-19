use super::*;
use crate::{
    AccessWidth, GuestPhysicalAddress, MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec,
    RamLimits,
};
use od_isa::interrupt::{Cpl0Int3Phase, Cpl0PageFaultDeliveryPhase};
use od_isa::memory::{AccessKind, InstructionMode, PrivilegeLevel};
use od_isa::state::{Gdtr, Idtr};
use od_isa::{
    CodeSegmentDescriptorCache, CpuActivity, Register, StackSegmentDescriptorCache, StiShadow,
};

const ROOT: u64 = 0x1000;
const CODE: u64 = 0x0040_0000;
const DATA: u64 = 0x0080_0000;
const CODE_PHYSICAL: u64 = 0x7000;
const DATA_PHYSICAL: u64 = 0x8000;
const GUARD: u64 = DATA + 0x1000;
const STACK: u64 = DATA + 0xf40;
const BREAKPOINT: u64 = CODE + 0x100;
const PAGE_FAULT: u64 = CODE + 0x140;
const EFER: u64 = 0xd00;
const DF: u64 = 1 << 10;
const RF: u64 = 1 << 16;

fn write(bus: &mut MachineBus, address: u64, width: AccessWidth, value: u64) {
    bus.write_physical(GuestPhysicalAddress::new(address), width, value, |_| {})
        .unwrap();
}

fn read_frame(bus: &mut MachineBus, address: u64, words: usize) -> Vec<u64> {
    (0..words)
        .map(|n| {
            bus.read_physical(
                GuestPhysicalAddress::new(DATA_PHYSICAL + address - DATA + n as u64 * 8),
                AccessWidth::Qword,
            )
            .unwrap()
        })
        .collect()
}

fn set_gate(bus: &mut MachineBus, vector: u64, target: u64) {
    let address = DATA_PHYSICAL + 16 * vector;
    let low = (target & 0xffff) | 0x0000_8e00_0008_0000 | ((target & 0xffff0000) << 32);
    write(bus, address, AccessWidth::Qword, low);
    write(bus, address + 8, AccessWidth::Qword, target >> 32);
}

fn fixture(code: &[u8]) -> (LongModeState, MachineBus) {
    let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 0x10000).unwrap();
    let map =
        MachineMemoryMap::new(36, vec![MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)]).unwrap();
    let mut bus = MachineBus::new(map, RamLimits::new(0x10000), vec![], vec![], vec![]).unwrap();
    for (address, pte) in [
        (ROOT, 0x2003),
        (0x2000, 0x3003),
        (0x3010, 0x4003),
        (0x3020, 0x5003),
        (0x4000, CODE_PHYSICAL | 3),
        (0x5000, DATA_PHYSICAL | 3),
    ] {
        write(&mut bus, address, AccessWidth::Qword, pte);
    }
    for (index, byte) in code.iter().copied().enumerate() {
        write(
            &mut bus,
            CODE_PHYSICAL + index as u64,
            AccessWidth::Byte,
            u64::from(byte),
        );
    }
    write(&mut bus, CODE_PHYSICAL + 0x100, AccessWidth::Byte, 0x90);
    write(&mut bus, CODE_PHYSICAL + 0x140, AccessWidth::Byte, 0x90);
    write(
        &mut bus,
        DATA_PHYSICAL + 0x208,
        AccessWidth::Qword,
        0x00af9b000000ffff,
    );
    write(
        &mut bus,
        DATA_PHYSICAL + 0x210,
        AccessWidth::Qword,
        0x00cf93000000ffff,
    );
    set_gate(&mut bus, 3, BREAKPOINT);
    set_gate(&mut bus, 14, PAGE_FAULT);
    let mut state = LongModeState {
        rip: CODE,
        rflags: 2,
        cr0: Some(0x8001_0033),
        cr3: Some(ROOT),
        cr4: Some(0x620),
        cs_selector: Some(8),
        cs_cache: Some(CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x9b,
            flags: 0xa,
        }),
        ss_selector: Some(16),
        ss_cache: Some(StackSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x93,
            flags: 0xc,
        }),
        gdtr: Some(Gdtr {
            base: DATA + 0x200,
            limit: 23,
        }),
        idtr: Some(Idtr {
            base: DATA,
            limit: 0xfff,
        }),
        sti_shadow: Some(StiShadow::Clear),
        activity: Some(CpuActivity::Running),
        ..LongModeState::default()
    };
    state.gpr[Register::Rsp as usize] = STACK;
    state.gpr[Register::Rbx as usize] = GUARD + 8;
    state.gpr[Register::Rax as usize] = 0x1234;
    (state, bus)
}

fn step(state: &mut LongModeState, bus: &mut MachineBus, budget: u64) -> PagedProbeQuantumReport {
    run_paged_firmware_probe(
        state,
        bus,
        ROOT,
        EFER,
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        },
        budget,
        od_isa::INTEGER_FLAGS,
        || false,
        |_| {},
        None,
    )
}

#[test]
fn firmware_breakpoint_retires_and_saves_the_rip_after_int3() {
    let (mut state, mut bus) = fixture(&[0xfd, 0xcc]);
    let original = state.clone();
    let zero = step(&mut state, &mut bus, 0);
    assert_eq!(zero.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(state, original);

    let report = step(&mut state, &mut bus, 2);
    assert_eq!(report.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(report.instructions_executed, 2);
    assert_eq!(state.rip, BREAKPOINT);
    assert_eq!(state.cr2, None);
    let rsp = state.gpr[Register::Rsp as usize];
    assert_eq!(rsp, STACK - 40);
    assert_eq!(
        read_frame(&mut bus, rsp, 5),
        [CODE + 2, 8, 2 | DF, STACK, 16]
    );
}

#[test]
fn firmware_page_fault_enters_its_handler_without_retiring_the_load() {
    let (mut state, mut bus) = fixture(&[0x48, 0x8b, 0x03]);
    let report = step(&mut state, &mut bus, 1);
    assert_eq!(report.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(report.instructions_executed, 1);
    assert_eq!(state.rip, PAGE_FAULT + 1);
    assert_eq!(state.cr2, Some(GUARD + 8));
    assert_eq!(state.gpr[Register::Rax as usize], 0x1234);
    let rsp = state.gpr[Register::Rsp as usize];
    assert_eq!(rsp, STACK - 48);
    assert_eq!(
        read_frame(&mut bus, rsp, 6),
        [0, CODE, 8, 2 | RF, STACK, 16]
    );
}

#[test]
fn firmware_exception_failure_is_bounded_and_preserves_fault_provenance() {
    let (mut state, mut bus) = fixture(&[0x48, 0x8b, 0x03]);
    state.idtr.as_mut().unwrap().base = GUARD - 14 * 16;
    let before = state.clone();
    let report = step(&mut state, &mut bus, 8);
    assert_eq!(report.instructions_executed, 0);
    assert_eq!(state, before);
    let original = report.page_fault.unwrap();
    assert_eq!(original.cr2_value(), GUARD + 8);
    assert_eq!(original.context().origin, AccessOrigin::Explicit);
    let PagedProbeExit::IntegrationError(PagedProbeIntegrationError::FirmwarePageFaultDelivery {
        failure:
            Cpl0PageFaultDelivery::Aborted {
                phase: Cpl0PageFaultDeliveryPhase::IdtGateRead,
                ..
            },
        delivery_fault: Some(delivery),
    }) = report.exit
    else {
        panic!("unexpected firmware #PF delivery result: {:?}", report.exit);
    };
    assert_eq!(delivery.cr2_value(), GUARD);
    assert_eq!(delivery.context().origin, AccessOrigin::DescriptorTable);

    let (mut state, mut bus) = fixture(&[0xcc]);
    state.idtr.as_mut().unwrap().base = GUARD - 3 * 16;
    let before = state.clone();
    let report = step(&mut state, &mut bus, 8);
    assert_eq!(report.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(
        report.page_fault.unwrap().context().origin,
        AccessOrigin::DescriptorTable
    );
    assert!(matches!(
        report.exit,
        PagedProbeExit::IntegrationError(PagedProbeIntegrationError::FirmwareBreakpointDelivery {
            failure: Cpl0Int3Delivery::Aborted {
                phase: Cpl0Int3Phase::IdtGateRead,
                ..
            },
        })
    ));

    let (mut state, mut bus) = fixture(&[0x48, 0x8b, 0x03]);
    set_gate(&mut bus, 14, GUARD);
    let report = step(&mut state, &mut bus, 8);
    assert_eq!(report.instructions_executed, 0);
    assert_eq!(state.rip, GUARD);
    assert_eq!(state.cr2, Some(GUARD + 8));
    assert_eq!(state.gpr[Register::Rsp as usize], STACK - 48);
    assert!(matches!(report.exit, PagedProbeExit::Fault(_)));
    assert_eq!(report.page_fault.unwrap().access(), AccessKind::Fetch);
}

#[test]
fn firmware_without_idtr_keeps_reporting_faults_at_the_boundary() {
    let (mut state, mut bus) = fixture(&[0xcc]);
    state.idtr = None;
    let before = state.clone();
    let report = step(&mut state, &mut bus, 8);
    assert_eq!(report.exit, PagedProbeExit::Unsupported { opcode: 0xcc });
    assert_eq!(state, before);

    let (mut state, mut bus) = fixture(&[0x48, 0x8b, 0x03]);
    state.idtr = None;
    let before = state.clone();
    let report = step(&mut state, &mut bus, 8);
    assert!(matches!(report.exit, PagedProbeExit::Fault(_)));
    assert_eq!(report.page_fault.unwrap().cr2_value(), GUARD + 8);
    assert_eq!(report.instructions_executed, 0);
    assert_eq!(state, before);
}

#[test]
fn firmware_synchronous_faults_keep_original_rip_and_optional_error_code() {
    let absent_data = 0x00cf13000000ffff;
    let present_code = 0x00af9b000000ffff;
    for (vector, code, rax, descriptor, error_code) in [
        (6, &[0x0f, 0x0b][..], 0, absent_data, None), // UD2
        (11, &[0x8e, 0xd8][..], 0x10, absent_data, Some(0x10)), // MOV DS,AX
        (12, &[0x8e, 0xd0][..], 0x10, absent_data, Some(0x10)), // MOV SS,AX
        (13, &[0x0f, 0x22, 0xe0][..], 0x600, absent_data, Some(0)), // MOV CR4,RAX clears PAE
        (13, &[0x8e, 0xd0][..], 0x10, present_code, Some(0x10)), // MOV SS,AX needs a data descriptor
    ] {
        let (mut state, mut bus) = fixture(code);
        set_gate(&mut bus, vector, PAGE_FAULT);
        write(
            &mut bus,
            DATA_PHYSICAL + 0x210,
            AccessWidth::Qword,
            descriptor,
        );
        state.gpr[Register::Rax as usize] = rax;
        state.cr2 = Some(GUARD);
        let report = step(&mut state, &mut bus, 1);
        assert_eq!(
            report.exit,
            PagedProbeExit::BudgetExhausted,
            "vector {vector}"
        );
        assert_eq!(report.instructions_executed, 1);
        assert_eq!(state.rip, PAGE_FAULT + 1);
        assert_eq!(state.cr2, Some(GUARD));
        assert_eq!(state.cr4, Some(0x620));
        assert_eq!(state.ss_selector, Some(16));
        assert_eq!(state.gpr[Register::Rax as usize], rax);
        let mut expected: Vec<u64> = error_code.into_iter().collect();
        expected.extend([CODE, 8, 2 | RF, STACK, 16]);
        let rsp = state.gpr[Register::Rsp as usize];
        assert_eq!(rsp, STACK - expected.len() as u64 * 8);
        assert_eq!(read_frame(&mut bus, rsp, expected.len()), expected);
    }
}

#[test]
fn firmware_event_entry_ends_the_mov_ss_shadow_after_successful_delivery() {
    for (next, target) in [
        (&[0xcc][..], BREAKPOINT),
        (&[0x48, 0x8b, 0x03][..], PAGE_FAULT + 1),
        (&[0x0f, 0x0b][..], PAGE_FAULT + 1),
    ] {
        let mut code = vec![0x8e, 0xd0]; // MOV SS,AX
        code.extend_from_slice(next);
        let (mut state, mut bus) = fixture(&code);
        set_gate(&mut bus, 6, PAGE_FAULT);
        state.gpr[Register::Rax as usize] = 0x10;
        let first = step(&mut state, &mut bus, 1);
        assert_eq!(first.exit, PagedProbeExit::BudgetExhausted);
        assert_eq!(state.rip, CODE + 2);
        assert_eq!(state.sti_shadow, Some(StiShadow::Blocking));
        let second = step(&mut state, &mut bus, 1);
        assert_eq!(second.exit, PagedProbeExit::BudgetExhausted);
        assert_eq!(state.rip, target);
        assert_eq!(state.sti_shadow, Some(StiShadow::Clear));
    }
}

#[test]
fn firmware_synchronous_delivery_error_keeps_the_original_exception() {
    let (mut state, mut bus) = fixture(&[0x8e, 0xd0, 0x0f, 0x0b]); // MOV SS,AX; UD2
    state.gpr[Register::Rax as usize] = 16;
    state.idtr.as_mut().unwrap().base = GUARD - 6 * 16;
    let first = step(&mut state, &mut bus, 1);
    assert_eq!(first.exit, PagedProbeExit::BudgetExhausted);
    let before = state.clone();
    let report = step(&mut state, &mut bus, 1);
    assert_eq!(report.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(state.sti_shadow, Some(StiShadow::Blocking));
    assert_eq!(report.page_fault, None);
    let PagedProbeExit::IntegrationError(
        PagedProbeIntegrationError::FirmwareSynchronousFaultDelivery {
            fault,
            failure:
                Cpl0GeneralProtectionDelivery::Aborted {
                    phase: od_isa::interrupt::Cpl0GeneralProtectionPhase::IdtGateRead,
                    ..
                },
            delivery_fault: Some(delivery),
        },
    ) = report.exit
    else {
        panic!("unexpected #UD delivery result: {:?}", report.exit);
    };
    assert_eq!((fault.vector, fault.error_code), (6, None));
    assert_eq!(delivery.cr2_value(), GUARD);
    assert_eq!(delivery.context().origin, AccessOrigin::DescriptorTable);
}
