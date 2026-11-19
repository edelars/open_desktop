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
    step_with_fp(state, &mut GuestFpState::initial(), bus, budget)
}

fn step_with_fp(
    state: &mut LongModeState,
    fp: &mut GuestFpState,
    bus: &mut MachineBus,
    budget: u64,
) -> PagedProbeQuantumReport {
    run_paged_firmware_probe(
        state,
        fp,
        bus,
        FirmwareMmio::default(),
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
fn nvram_dispatch_leaves_unserved_mmio_unwritten_and_clean() {
    let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 0x10000).unwrap();
    let lapic = GuestPhysicalRange::new(
        GuestPhysicalAddress::new(crate::LOCAL_APIC_MMIO_BASE),
        0x1000,
    )
    .unwrap();
    let nvram =
        GuestPhysicalRange::new(GuestPhysicalAddress::new(NVRAM_MMIO_BASE), NVRAM_MMIO_BYTES)
            .unwrap();
    let map = MachineMemoryMap::new(
        36,
        vec![
            MemoryRegionSpec::new(ram, MemoryRegionKind::Ram),
            MemoryRegionSpec::new(lapic, MemoryRegionKind::Mmio),
            MemoryRegionSpec::new(nvram, MemoryRegionKind::Mmio),
        ],
    )
    .unwrap();
    let memory = crate::PhysicalMemory::new(&map, RamLimits::new(0x10000)).unwrap();
    let nvram_registers = NVRAM_MMIO_BASE + crate::NVRAM_STORE_BYTES as u64;
    for (at, value) in [
        (ROOT, 0x2003u64),
        (0x2000, 0x3003),
        (0x3020, 0x5003),
        (0x5000, crate::LOCAL_APIC_MMIO_BASE | 3),
        (0x5008, nvram_registers | 3),
    ] {
        memory
            .store_bytes(GuestPhysicalAddress::new(at), &value.to_le_bytes(), |_| {})
            .unwrap();
    }
    let (state, _) = fixture(&[0xf4]);
    let paging = Long64ProbeConfig::from_cpu_state(&state, ROOT, EFER).unwrap();
    let context = AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::Explicit,
    };
    let device = crate::NvramDevice::new();
    let handle = device.handle();
    let drive = |address, operation, handle: Option<&NvramHandle>| {
        let mut probe = PagedProbeMemory {
            memory: &memory,
            paging,
            instruction_rip: CODE,
            large_ram_profile: false,
            firmware_boot_profile: true,
            mmio: FirmwareMmio {
                nvram: handle,
                ..FirmwareMmio::default()
            },
            mmio_clock: None,
            retired_before: 0,
            committed_write: &mut |_| {},
            page_fault: None,
            memory_error: None,
        };
        let access = probe.access(address, context, operation);
        (access, probe.memory_error)
    };
    let write = RamAccessOperation::Write {
        width: AccessWidth::Dword,
        value: 1,
    };
    let (_, error) = drive(DATA, write, Some(&handle));
    assert_eq!(
        error,
        Some(TrackedAccessError::DeviceUnavailable {
            address: lapic.start()
        })
    );
    let mut pte = [0; 8];
    memory
        .load_bytes(GuestPhysicalAddress::new(0x5000), &mut pte)
        .unwrap();
    assert_eq!(u64::from_le_bytes(pte) & (1 << 6), 0);

    let read = RamAccessOperation::Read {
        width: AccessWidth::Dword,
    };
    let active_len = DATA + 0x1000 + 16;
    assert_eq!(
        drive(active_len, read, Some(&handle)).0,
        Ok(TrackedAccessOutcome::Read(24))
    );
    let (_, error) = drive(active_len, read, None);
    assert!(
        matches!(error, Some(TrackedAccessError::OperandPhysical { address, .. }) if address.get() == nvram_registers + 16)
    );
}

#[test]
fn firmware_fx_image_uses_the_same_fp_context_across_quanta() {
    let (mut state, mut bus) = fixture(&[
        0x48, 0x0f, 0xae, 0x4b, 0x40, // FXRSTOR64 0x40(RBX)
        0x48, 0x0f, 0xae, 0x40, 0x40, // FXSAVE64 0x40(RAX)
    ]);
    let mut fp = GuestFpState::initial();
    let source = 0x400;
    let destination = 0x600;
    state.gpr[Register::Rbx as usize] = DATA + source - 0x40;
    state.gpr[Register::Rax as usize] = DATA + destination - 0x40;
    write(&mut bus, DATA_PHYSICAL + source, AccessWidth::Word, 0x0b7f);
    write(
        &mut bus,
        DATA_PHYSICAL + source + 24,
        AccessWidth::Dword,
        0x3f80,
    );
    let canary = 0xa5a5_6b6b_2929_c7c7;
    write(
        &mut bus,
        DATA_PHYSICAL + source + 352,
        AccessWidth::Qword,
        canary,
    );

    let restore = step_with_fp(&mut state, &mut fp, &mut bus, 1);
    assert_eq!(restore.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(restore.instructions_executed, 1);
    assert_eq!(fp.x87.control_word, 0x0b7f);
    assert_eq!(fp.sse.mxcsr, 0x3f80);
    assert_eq!(fp.sse.xmm[12][..8], canary.to_le_bytes());

    let save = step_with_fp(&mut state, &mut fp, &mut bus, 1);
    assert_eq!(save.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(save.instructions_executed, 1);
    assert_eq!(read_frame(&mut bus, DATA + destination, 1), [0x0b7f]);
    assert_eq!(read_frame(&mut bus, DATA + destination + 352, 1), [canary]);
    assert!(state.x87.is_none() && state.sse.is_none());
}

#[test]
fn firmware_scalar_loads_can_cross_nonadjacent_read_only_ram_pages() {
    for (code, loaded) in [
        (&[0x0f, 0xb7, 0x69, 0x38][..], 0x3412),
        (&[0x8b, 0x69, 0x38][..], 0x7856_3412),
        (&[0x48, 0x8b, 0x69, 0x38][..], 0xf0de_bc9a_7856_3412),
    ] {
        let (mut state, mut bus) = fixture(code);
        let operand = GUARD - 1;
        let second_frame = 0xa000;
        state.gpr[Register::Rcx as usize] = operand - 0x38;
        state.gpr[Register::Rbp as usize] = u64::MAX;
        write(&mut bus, 0x5000, AccessWidth::Qword, DATA_PHYSICAL | 1);
        write(&mut bus, 0x5008, AccessWidth::Qword, second_frame | 1);
        write(&mut bus, DATA_PHYSICAL + 0xfff, AccessWidth::Byte, 0x12);
        write(
            &mut bus,
            second_frame,
            AccessWidth::Qword,
            0xa5f0_debc_9a78_5634,
        );

        let mut expected = state.clone();
        expected.rip += code.len() as u64;
        expected.gpr[Register::Rbp as usize] = loaded;
        let report = step(&mut state, &mut bus, 1);
        assert_eq!(report.exit, PagedProbeExit::BudgetExhausted);
        assert_eq!(report.instructions_executed, 1);
        assert_eq!(state, expected);
        assert!(report.page_fault.is_none());
        for (pte, frame) in [(0x5000, DATA_PHYSICAL), (0x5008, second_frame)] {
            assert_eq!(
                bus.read_physical(GuestPhysicalAddress::new(pte), AccessWidth::Qword)
                    .unwrap(),
                frame | 0x21
            );
        }
    }
}

#[test]
fn firmware_cross_page_load_reports_the_second_page_fault_without_retiring() {
    let (mut state, mut bus) = fixture(&[0x8b, 0x69, 0x38]);
    state.idtr = None;
    state.gpr[Register::Rcx as usize] = GUARD - 1 - 0x38;
    state.gpr[Register::Rbp as usize] = u64::MAX;
    let before = state.clone();
    let report = step(&mut state, &mut bus, 1);
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0),
        linear_address: Some(GUARD),
    };
    assert_eq!(report.exit, PagedProbeExit::Fault(fault));
    assert_eq!(report.instructions_executed, 0);
    assert_eq!(state, before);
    let record = report.page_fault.unwrap();
    assert_eq!(record.access(), AccessKind::Read);
    assert_eq!(record.cr2_value(), GUARD);
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(0x5000), AccessWidth::Qword)
            .unwrap(),
        DATA_PHYSICAL | 0x23
    );
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(0x5008), AccessWidth::Qword)
            .unwrap(),
        0
    );
}

#[test]
fn firmware_cross_page_load_preserves_the_backend_error_for_unbacked_data() {
    let (mut state, mut bus) = fixture(&[0x8b, 0x69, 0x38]);
    state.gpr[Register::Rcx as usize] = GUARD - 1 - 0x38;
    let unbacked_frame = 0x1_0000;
    write(&mut bus, 0x5008, AccessWidth::Qword, unbacked_frame | 1);
    let before = state.clone();
    let report = step(&mut state, &mut bus, 1);
    assert!(matches!(report.exit, PagedProbeExit::Backend(_)));
    assert_eq!(report.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(report.page_fault.is_none());
    assert!(matches!(
        report.memory_error,
        Some(TrackedAccessError::OperandPhysical { address, .. })
            if address == GuestPhysicalAddress::new(unbacked_frame)
    ));
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
