use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};
use od_isa::state::{CodeSegmentDescriptorCache, DataSegmentDescriptorCache, Gdtr, Idtr};
use od_isa::{CpuActivity, LongModeState, Register, StiShadow};
use od_machine::{
    AccessWidth, GuestPhysicalAddress, GuestPhysicalRange, MachineBus, MachineMemoryMap,
    MemoryRegionKind, MemoryRegionSpec, RamLimits, deliver_and_step_paged_pf_handler,
    step_paged_cr0_wp_mov,
};

const CR0: u64 = 0x8001_0033;
const CR3: u64 = 0x1000;
const CR4: u64 = 1 << 5;
const EFER: u64 = (1 << 8) | (1 << 10) | (1 << 11);
const FAULT_RIP: u64 = 0x0040_1000;
const HANDLER_RIP: u64 = 0x0040_2000;
const DATA_LINEAR: u64 = 0x0080_0000;
const IDT_BASE: u64 = 0x0060_0000;
const GDT_BASE: u64 = 0x0060_1000;
const ORIGINAL_RSP: u64 = 0x0070_2027;
const DATA_PHYSICAL: u64 = 0xe000;
const SENTINEL: u64 = 0x0123_4567_89ab_cdef;
const STORE: [u8; 3] = [0x48, 0x89, 0x0b]; // MOV [RBX],RCX
const LOAD: [u8; 3] = [0x48, 0x8b, 0x03]; // MOV RAX,[RBX]

fn context() -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::Explicit,
    }
}

fn put_qword(bus: &mut MachineBus, address: u64, value: u64) {
    bus.write_physical(
        GuestPhysicalAddress::new(address),
        AccessWidth::Qword,
        value,
        |_| {},
    )
    .unwrap();
}

fn put_bytes(bus: &mut MachineBus, address: u64, bytes: &[u8]) {
    bus.patch_physical_ram(GuestPhysicalAddress::new(address), bytes)
        .unwrap();
}

fn make_bus(
    idt_present: bool,
    gdt_present: bool,
    lower_stack_page_present: bool,
    handler_present: bool,
) -> MachineBus {
    let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 0x1_0000).unwrap();
    let map =
        MachineMemoryMap::new(36, vec![MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)]).unwrap();
    let mut bus = MachineBus::new(map, RamLimits::new(0x1_0000), vec![], vec![], vec![]).unwrap();

    // One shared PML4/PDPT; private 4 KiB page tables for code, descriptors,
    // stack and data. A bits on the code-fetch path are pre-set.
    put_qword(&mut bus, 0x1000, 0x2027);
    put_qword(&mut bus, 0x2000, 0x3027);
    put_qword(&mut bus, 0x3010, 0x4027); // 4 MiB code branch
    put_qword(&mut bus, 0x3018, 0x5027); // 6–8 MiB table/stack branch
    put_qword(&mut bus, 0x3020, 0x6007); // 8 MiB data branch, A clear
    put_qword(&mut bus, 0x4008, 0x8000 | 0x27);
    put_qword(
        &mut bus,
        0x4010,
        0x9000 | if handler_present { 0x27 } else { 0x26 },
    );
    put_qword(
        &mut bus,
        0x5000,
        0xa000 | if idt_present { 0x27 } else { 0x26 },
    );
    put_qword(
        &mut bus,
        0x5008,
        0xb000 | if gdt_present { 0x27 } else { 0x26 },
    );
    put_qword(
        &mut bus,
        0x5808,
        0xc000 | if lower_stack_page_present { 0x27 } else { 0x26 },
    ); // 0x701000
    put_qword(&mut bus, 0x5810, 0xd000 | 0x27); // 0x702000
    put_qword(&mut bus, 0x6000, DATA_PHYSICAL | 0x1); // present, supervisor, read-only, A clear
    put_bytes(&mut bus, 0x8000, &STORE);
    put_bytes(&mut bus, 0x9000, &LOAD);
    put_qword(&mut bus, DATA_PHYSICAL, SENTINEL);

    // Interrupt gate 14, same CS, IST=0, target in canonical Long64 code.
    let gate = 0xa000 + 14 * 16;
    let gate_bytes = [
        (HANDLER_RIP as u16).to_le_bytes()[0],
        (HANDLER_RIP as u16).to_le_bytes()[1],
        0x08,
        0x00,
        0x00,
        0x8e,
        (HANDLER_RIP >> 16) as u8,
        (HANDLER_RIP >> 24) as u8,
        (HANDLER_RIP >> 32) as u8,
        (HANDLER_RIP >> 40) as u8,
        (HANDLER_RIP >> 48) as u8,
        (HANDLER_RIP >> 56) as u8,
        0,
        0,
        0,
        0,
    ];
    put_bytes(&mut bus, gate, &gate_bytes);
    // Accessed=1 is an explicit bounded profile requirement: delivery does
    // not synthesize a descriptor write; walker A/D effects remain tracked.
    put_bytes(&mut bus, 0xb008, &[0xff, 0xff, 0, 0, 0, 0x9b, 0xaf, 0]);
    bus
}

fn initial_state(rflags: u64) -> LongModeState {
    let mut state = LongModeState {
        rip: FAULT_RIP,
        cr0: Some(CR0),
        cr3: Some(CR3),
        cr4: Some(CR4),
        rflags,
        cs_selector: Some(0x8),
        cs_cache: Some(CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x9b,
            flags: 2,
        }),
        ss_selector: Some(0x10),
        ss_cache: Some(DataSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x93,
            flags: 0,
        }),
        gdtr: Some(Gdtr {
            base: GDT_BASE,
            limit: 0x1000,
        }),
        idtr: Some(Idtr {
            base: IDT_BASE,
            limit: 0x1000,
        }),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ..Default::default()
    };
    state.gpr[Register::Rbx as usize] = DATA_LINEAR;
    state.gpr[Register::Rcx as usize] = 0xdead_beef_cafe_babe;
    state.gpr[Register::Rsp as usize] = ORIGINAL_RSP;
    state
}

fn start_faulting_store(
    bus: &mut MachineBus,
    state: &mut LongModeState,
) -> od_machine::PagedCr0WpMovStepReport {
    let mut writes = Vec::new();
    let step = step_paged_cr0_wp_mov(
        state,
        bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        |range| writes.push(range),
    )
    .unwrap();
    assert_eq!(step.cpu.instructions_executed, 0);
    let Some(record) = &step.page_fault else {
        panic!("the WP=1 store must produce a paged #PF: {step:?}");
    };
    assert_eq!(record.fault().vector, 14);
    assert_eq!(record.fault().error_code, Some(3));
    assert_eq!(record.saved_rip(), FAULT_RIP);
    assert_eq!(record.cr2_value(), DATA_LINEAR);
    assert_eq!(state.rip, FAULT_RIP);
    assert_eq!(
        step.ad_updates,
        [
            od_machine::AdUpdate {
                level: od_machine::PageTableLevel::Pd,
                address: GuestPhysicalAddress::new(0x3020),
                before: 0x6007,
                after: 0x6027,
                bit: od_machine::AdBit::Accessed,
            },
            od_machine::AdUpdate {
                level: od_machine::PageTableLevel::Pt,
                address: GuestPhysicalAddress::new(0x6000),
                before: DATA_PHYSICAL | 0x1,
                after: DATA_PHYSICAL | 0x21,
                bit: od_machine::AdBit::Accessed,
            },
        ]
    );
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(DATA_PHYSICAL), AccessWidth::Qword)
            .unwrap(),
        SENTINEL
    );
    assert!(writes.iter().any(|range| range.start().get() == 0x3020));
    step
}

fn run_case(
    idt_present: bool,
    gdt_present: bool,
    lower_stack_page_present: bool,
    handler_present: bool,
    rflags: u64,
) -> (
    MachineBus,
    LongModeState,
    od_machine::PagedExceptionHandlerEntryReport,
    Vec<GuestPhysicalRange>,
) {
    let mut bus = make_bus(
        idt_present,
        gdt_present,
        lower_stack_page_present,
        handler_present,
    );
    let mut state = initial_state(rflags);
    let fault = start_faulting_store(&mut bus, &mut state)
        .page_fault
        .unwrap();
    let mut writes = Vec::new();
    let report = deliver_and_step_paged_pf_handler(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        &fault,
        |range| writes.push(range),
    )
    .unwrap();
    (bus, state, report, writes)
}

#[test]
fn wp_write_fault_delivers_with_if_clear_and_executes_first_handler_mov() {
    let initial_flags = 0x2; // synchronous #PF delivery is independent of IF
    let (mut bus, state, report, writes) = run_case(true, true, true, true, initial_flags);
    assert!(
        matches!(
            report.delivery.delivery,
            od_isa::PageFaultDeliveryResult::Delivered {
                target_rip: HANDLER_RIP,
                new_rsp: 0x0070_1ff0,
            }
        ),
        "delivery report: {:?}",
        report.delivery
    );
    assert_eq!(report.delivery.original_fault.fault().error_code, Some(3));
    assert_eq!(state.cr2, Some(DATA_LINEAR));
    assert_eq!(state.rip, HANDLER_RIP + 3);
    assert_eq!(state.gpr[Register::Rax as usize], SENTINEL);
    assert_eq!(state.gpr[Register::Rsp as usize], 0x0070_1ff0);
    assert_eq!(state.rflags & (1 << 16), 0);
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(DATA_PHYSICAL), AccessWidth::Qword)
            .unwrap(),
        SENTINEL
    );
    let handler = report.handler_step.unwrap().unwrap();
    assert_eq!(handler.cpu.instructions_executed, 1);
    assert!(handler.page_fault.is_none());
    assert!(report.delivery.secondary_page_fault.is_none());

    let expected_frame = [
        (0x0070_2018, 0x10),
        (0x0070_2010, ORIGINAL_RSP),
        (0x0070_2008, initial_flags | (1 << 16)),
        (0x0070_2000, 0x8),
        (0x0070_1ff8, FAULT_RIP),
        (0x0070_1ff0, 3),
    ];
    // Reconstruct from the callback ordered ranges: six guest stack writes
    // must appear in push order after their tracked page-table A/D writes.
    let stack_writes = writes
        .iter()
        .filter(|range| range.len() == 8 && (0xc000..0xe000).contains(&range.start().get()))
        .count();
    assert_eq!(stack_writes, 6);
    let ordered_writes: Vec<_> = writes
        .iter()
        .map(|range| (range.start().get(), range.len()))
        .collect();
    assert_eq!(
        ordered_writes,
        [
            (0x5810, 8),
            (0xd018, 8),
            (0xd010, 8),
            (0xd008, 8),
            (0xd000, 8),
            (0x5808, 8),
            (0xcff8, 8),
            (0xcff0, 8),
        ]
    );
    for (index, (linear, value)) in expected_frame.into_iter().enumerate() {
        let physical = if linear >= 0x0070_2000 {
            0xd000 + (linear & 0xfff)
        } else {
            0xc000 + (linear & 0xfff)
        };
        let actual = bus
            .read_physical(GuestPhysicalAddress::new(physical), AccessWidth::Qword)
            .unwrap();
        assert_eq!(actual, value, "frame slot {index}");
    }
    let dirty_updates: Vec<_> = report
        .delivery
        .ad_updates
        .iter()
        .filter(|update| update.bit == od_machine::AdBit::Dirty)
        .map(|update| update.address.get())
        .collect();
    assert_eq!(dirty_updates, [0x5810, 0x5808]);
}

#[test]
fn secondary_idt_gdt_and_partial_stack_faults_replace_cr2_and_keep_original() {
    for (
        idt_present,
        gdt_present,
        lower_stack_page_present,
        expected_phase,
        expected_cr2,
        writes_before_fault,
    ) in [
        (
            false,
            true,
            true,
            od_isa::PageFaultDeliveryPhase::IdtGateRead,
            IDT_BASE + 14 * 16,
            0,
        ),
        (
            true,
            false,
            true,
            od_isa::PageFaultDeliveryPhase::TargetCodeDescriptorRead,
            GDT_BASE + 8,
            0,
        ),
        (
            true,
            true,
            false,
            od_isa::PageFaultDeliveryPhase::StackWrite {
                slot: od_isa::PageFaultFrameSlot::SavedRip,
            },
            0x0070_1ff8,
            4,
        ),
    ] {
        let (_, state, report, writes) = run_case(
            idt_present,
            gdt_present,
            lower_stack_page_present,
            true,
            0x202,
        );
        let original = &report.delivery.original_fault;
        assert_eq!(original.fault().error_code, Some(3));
        assert_eq!(original.cr2_value(), DATA_LINEAR);
        assert_eq!(
            state.cr2,
            Some(expected_cr2),
            "delivery report: {:?}",
            report.delivery
        );
        assert!(
            matches!(report.delivery.delivery, od_isa::PageFaultDeliveryResult::DeliveryAborted { phase, .. } if phase == expected_phase)
        );
        assert!(report.handler_step.is_none());
        let secondary = report.delivery.secondary_page_fault.as_ref().unwrap();
        assert_eq!(secondary.cr2_value(), expected_cr2);
        assert_eq!(
            writes
                .iter()
                .filter(|range| range.len() == 8 && (0xc000..0xe000).contains(&range.start().get()))
                .count(),
            writes_before_fault
        );
        if matches!(
            expected_phase,
            od_isa::PageFaultDeliveryPhase::StackWrite { .. }
        ) {
            let ordered_writes: Vec<_> = writes
                .iter()
                .map(|range| (range.start().get(), range.len()))
                .collect();
            assert_eq!(
                ordered_writes,
                [
                    (0x5810, 8),
                    (0xd018, 8),
                    (0xd010, 8),
                    (0xd008, 8),
                    (0xd000, 8)
                ]
            );
        }
    }
}

#[test]
fn handler_fetch_fault_is_reported_after_successful_delivery() {
    let (_, state, report, _) = run_case(true, true, true, false, 0x202);
    assert!(
        matches!(
            report.delivery.delivery,
            od_isa::PageFaultDeliveryResult::Delivered { .. }
        ),
        "delivery report: {:?}",
        report.delivery
    );
    let handler = report.handler_step.unwrap().unwrap();
    assert_eq!(handler.cpu.instructions_executed, 0);
    assert_eq!(handler.page_fault.as_ref().unwrap().fault().vector, 14);
    assert_eq!(
        handler.page_fault.as_ref().unwrap().cr2_value(),
        HANDLER_RIP
    );
    assert_eq!(state.cr2, Some(HANDLER_RIP));
}

#[test]
fn unaligned_original_rsp_is_saved_while_frame_uses_aligned_push_base() {
    let (_, state, report, _) = run_case(true, true, true, true, 0x2);
    assert!(
        matches!(
            report.delivery.delivery,
            od_isa::PageFaultDeliveryResult::Delivered {
                new_rsp: 0x0070_1ff0,
                ..
            }
        ),
        "delivery report: {:?}",
        report.delivery
    );
    assert_eq!(state.gpr[Register::Rsp as usize], 0x0070_1ff0);
}

#[test]
fn unaligned_explicit_store_faults_without_retirement_or_data_commit() {
    let mut bus = make_bus(true, true, true, true);
    let mut state = initial_state(0x202);
    state.gpr[Register::Rbx as usize] = DATA_LINEAR + 1;
    let step = step_paged_cr0_wp_mov(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        |_| {},
    )
    .unwrap();
    assert_eq!(step.cpu.instructions_executed, 0);
    assert_eq!(state.rip, FAULT_RIP);
    assert_eq!(state.cr2, Some(DATA_LINEAR + 1));
    let fault = step.page_fault.unwrap();
    assert_eq!(fault.fault().error_code, Some(3));
    assert_eq!(fault.cr2_value(), DATA_LINEAR + 1);
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(DATA_PHYSICAL), AccessWidth::Qword)
            .unwrap(),
        SENTINEL
    );
}

#[test]
fn handler_cr0_write_is_rejected_by_mov_only_machine_profile() {
    let mut bus = make_bus(true, true, true, true);
    put_bytes(&mut bus, 0x9000, &[0x0f, 0x22, 0xc0]);
    let mut state = initial_state(0x202);
    state.gpr[Register::Rax as usize] = CR0 & !(1 << 16);
    let fault = start_faulting_store(&mut bus, &mut state)
        .page_fault
        .unwrap();
    let old_cr0 = state.cr0;
    let report = deliver_and_step_paged_pf_handler(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        &fault,
        |_| {},
    )
    .unwrap();
    let step = report.handler_step.unwrap().unwrap();
    assert_eq!(step.cpu.instructions_executed, 0);
    assert_eq!(
        step.cpu.exit,
        od_isa::PagedProbeExit::Unsupported { opcode: 0x22 }
    );
    assert_eq!(state.cr0, old_cr0);
}
