use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};
use od_isa::state::{CodeSegmentDescriptorCache, DataSegmentDescriptorCache, Gdtr, Idtr};
use od_isa::{CpuActivity, LongModeState, Register, StiShadow};
use od_machine::{
    AccessWidth, GuestPhysicalAddress, GuestPhysicalRange, MachineBus, MachineMemoryMap,
    MemoryRegionKind, MemoryRegionSpec, PagedExceptionDeliveryOutcome, RamLimits,
    deliver_and_step_paged_pf_handler, deliver_paged_page_fault, step_paged_cr0_wp_mov,
    step_paged_cr0_wp_page_table_repair_mov, step_paged_exception_stack_discard, step_paged_invlpg,
    step_paged_iretq, step_paged_lgdt, step_paged_lidt,
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

#[test]
fn guest_repairs_faulting_pte_and_retires_invlpg() {
    let mut bus = make_bus(true, true, true, true);
    put_qword(&mut bus, 0x4018, 0x6027); // writable alias of the data page table
    put_bytes(&mut bus, 0x9000, &[0x48, 0x89, 0x37, 0x0f, 0x01, 0x3b]);
    let mut state = initial_state(0x202);
    state.gpr[Register::Rdi as usize] = 0x0040_3000;
    state.gpr[Register::Rsi as usize] = 0xe023;
    let original_rbx = state.gpr[Register::Rbx as usize];
    let original_rcx = state.gpr[Register::Rcx as usize];

    let fault_step = start_faulting_store(&mut bus, &mut state);
    let fault = fault_step.page_fault.unwrap();
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(0x6000), AccessWidth::Qword)
            .unwrap(),
        0xe021
    );
    let mut delivery_writes = Vec::new();
    let delivery = deliver_paged_page_fault(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        &fault,
        |range| delivery_writes.push(range),
    )
    .unwrap();
    assert_eq!(
        delivery.outcome,
        PagedExceptionDeliveryOutcome::PageFaultDelivered
    );
    assert_eq!(delivery.committed_writes, delivery_writes);
    assert_eq!(
        delivery_writes
            .iter()
            .map(|r| (r.start().get(), r.len()))
            .collect::<Vec<_>>(),
        [
            (0x5810, 8),
            (0xd018, 8),
            (0xd010, 8),
            (0xd008, 8),
            (0xd000, 8),
            (0x5808, 8),
            (0xcff8, 8),
            (0xcff0, 8)
        ]
    );
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(0x6000), AccessWidth::Qword)
            .unwrap(),
        0xe021
    );
    assert_eq!(state.rip, HANDLER_RIP);

    let mut repair_writes = Vec::new();
    let repair = step_paged_cr0_wp_page_table_repair_mov(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        |range| repair_writes.push(range),
    )
    .unwrap();
    assert_eq!(repair.cpu.instructions_executed, 1);
    assert!(repair.page_fault.is_none());
    assert!(repair.access_error.is_none());
    assert_eq!(state.rip, HANDLER_RIP + 3);
    assert_eq!(repair.committed_writes, repair_writes);
    assert_eq!(
        repair_writes
            .iter()
            .map(|r| (r.start().get(), r.len()))
            .collect::<Vec<_>>(),
        [(0x4018, 8), (0x6000, 8)]
    );
    assert_eq!(repair.ad_updates.len(), 1);
    assert_eq!(repair.ad_updates[0].address.get(), 0x4018);
    assert_eq!(repair.ad_updates[0].bit, od_machine::AdBit::Dirty);
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(0x6000), AccessWidth::Qword)
            .unwrap(),
        0xe023
    );

    let mut invlpg_writes = Vec::new();
    let invlpg = step_paged_invlpg(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        |range| invlpg_writes.push(range),
    )
    .unwrap();
    assert_eq!(invlpg.cpu.instructions_executed, 1);
    assert_eq!(
        invlpg.cpu.exit,
        od_isa::InvlpgExit::TranslationInvalidation(od_isa::TranslationInvalidation::Invlpg {
            linear_address: DATA_LINEAR
        })
    );
    assert!(invlpg.page_fault.is_none());
    assert!(invlpg.access_error.is_none());
    assert!(invlpg_writes.is_empty());
    assert_eq!(state.rip, HANDLER_RIP + 6);
    assert_eq!(state.cr2, Some(DATA_LINEAR));
    assert_eq!(state.gpr[Register::Rbx as usize], original_rbx);
    assert_eq!(state.gpr[Register::Rcx as usize], original_rcx);
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(DATA_PHYSICAL), AccessWidth::Qword)
            .unwrap(),
        SENTINEL
    );
}

#[test]
fn guest_discards_page_fault_error_code_and_returns_with_paged_iretq() {
    let mut bus = make_bus(true, true, true, true);
    put_qword(&mut bus, 0x4018, 0x6027);
    put_bytes(
        &mut bus,
        0x9000,
        &[
            0x48, 0x89, 0x37, // MOV [RDI], RSI: repair only R/W, preserve U/S=0 and A
            0x0f, 0x01, 0x3b, // INVLPG [RBX]
            0x48, 0x8d, 0x64, 0x24, 0x08, // LEA RSP, [RSP + 8]: discard #PF error code
            0x48, 0xcf, // IRETQ
        ],
    );
    // The return profile reads the real, pre-accessed writable CPL0 SS descriptor.
    put_bytes(&mut bus, 0xb010, &[0xff, 0xff, 0, 0, 0, 0x93, 0xcf, 0]);
    let mut state = initial_state(0x202);
    state.gpr[Register::Rdi as usize] = 0x0040_3000;
    state.gpr[Register::Rsi as usize] = 0xe023;
    let original_rbx = state.gpr[Register::Rbx as usize];
    let original_rcx = state.gpr[Register::Rcx as usize];

    let fault = start_faulting_store(&mut bus, &mut state)
        .page_fault
        .unwrap();
    let delivery =
        deliver_paged_page_fault(&mut state, &mut bus, CR3, EFER, context(), &fault, |_| {})
            .unwrap();
    assert_eq!(
        delivery.outcome,
        PagedExceptionDeliveryOutcome::PageFaultDelivered
    );
    assert_eq!(state.rip, HANDLER_RIP);
    assert_eq!(state.cr2, Some(DATA_LINEAR));

    for (step, expected_rip) in [(0, HANDLER_RIP + 3), (1, HANDLER_RIP + 6)] {
        let report = if step == 0 {
            step_paged_cr0_wp_page_table_repair_mov(
                &mut state,
                &mut bus,
                CR3,
                EFER,
                context(),
                od_isa::INTEGER_FLAGS,
                |_| {},
            )
            .unwrap()
        } else {
            let report = step_paged_invlpg(
                &mut state,
                &mut bus,
                CR3,
                EFER,
                context(),
                od_isa::INTEGER_FLAGS,
                |_| {},
            )
            .unwrap();
            assert_eq!(report.cpu.instructions_executed, 1);
            assert_eq!(state.rip, expected_rip);
            continue;
        };
        assert_eq!(report.cpu.instructions_executed, 1);
        assert_eq!(state.rip, expected_rip);
    }

    let error_code_rsp = state.gpr[Register::Rsp as usize];
    let discard = step_paged_exception_stack_discard(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        |_| {},
    )
    .unwrap();
    assert_eq!(discard.cpu.instructions_executed, 1);
    assert_eq!(discard.page_fault, None);
    assert_eq!(state.rip, HANDLER_RIP + 11);
    assert_eq!(state.gpr[Register::Rsp as usize], error_code_rsp + 8);

    let returned = step_paged_iretq(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        |_| {},
    )
    .unwrap();
    assert_eq!(returned.cpu, od_isa::iretq::IretqResult::Retired);
    assert_eq!(returned.rflags_defined, od_isa::INTEGER_FLAGS);
    assert!(returned.page_fault.is_none());
    assert!(returned.access_error.is_none());
    assert_eq!(state.rip, FAULT_RIP);
    assert_eq!(state.gpr[Register::Rsp as usize], ORIGINAL_RSP);
    assert_eq!(state.rflags, 0x1_0202);
    assert_eq!(state.cr0, Some(CR0));
    assert_eq!(state.cr3, Some(CR3));
    assert_eq!(state.cr4, Some(CR4));
    assert_eq!(state.cr2, Some(DATA_LINEAR));
    assert_eq!(state.gpr[Register::Rbx as usize], original_rbx);
    assert_eq!(state.gpr[Register::Rcx as usize], original_rcx);
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(DATA_PHYSICAL), AccessWidth::Qword)
            .unwrap(),
        SENTINEL
    );
}

#[test]
fn guest_retries_faulting_store_after_paged_iretq() {
    const CANARY: u64 = 0x6a35_91c7_d248_b0ef;
    let mut bus = make_bus(true, true, true, true);
    put_qword(&mut bus, DATA_PHYSICAL + 8, CANARY);
    put_qword(&mut bus, 0x4018, 0x6027);
    put_bytes(
        &mut bus,
        0x8000,
        &[STORE.as_slice(), LOAD.as_slice()].concat(),
    );
    put_bytes(
        &mut bus,
        0x9000,
        &[
            0x48, 0x89, 0x37, // MOV [RDI], RSI
            0x0f, 0x01, 0x3b, // INVLPG [RBX]
            0x48, 0x8d, 0x64, 0x24, 0x08, // LEA RSP, [RSP + 8]
            0x48, 0xcf, // IRETQ
        ],
    );
    put_bytes(&mut bus, 0xb010, &[0xff, 0xff, 0, 0, 0, 0x93, 0xcf, 0]);
    let mut state = initial_state(0x202);
    state.gpr[Register::Rdi as usize] = 0x0040_3000;
    state.gpr[Register::Rsi as usize] = 0xe023;
    let value = state.gpr[Register::Rcx as usize];

    let fault = start_faulting_store(&mut bus, &mut state)
        .page_fault
        .unwrap();
    let delivery =
        deliver_paged_page_fault(&mut state, &mut bus, CR3, EFER, context(), &fault, |_| {})
            .unwrap();
    assert_eq!(
        delivery.outcome,
        PagedExceptionDeliveryOutcome::PageFaultDelivered
    );
    let repair = step_paged_cr0_wp_page_table_repair_mov(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        |_| {},
    )
    .unwrap();
    assert_eq!(repair.cpu.instructions_executed, 1);
    assert!(repair.page_fault.is_none() && repair.access_error.is_none());
    let invlpg = step_paged_invlpg(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        |_| {},
    )
    .unwrap();
    assert_eq!(invlpg.cpu.instructions_executed, 1);
    assert_eq!(
        invlpg.cpu.exit,
        od_isa::InvlpgExit::TranslationInvalidation(od_isa::TranslationInvalidation::Invlpg {
            linear_address: DATA_LINEAR
        })
    );
    let discard = step_paged_exception_stack_discard(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        |_| {},
    )
    .unwrap();
    assert_eq!(discard.cpu.instructions_executed, 1);
    let returned = step_paged_iretq(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        |_| {},
    )
    .unwrap();
    assert_eq!(returned.cpu, od_isa::iretq::IretqResult::Retired);
    assert!(returned.page_fault.is_none() && returned.access_error.is_none());
    assert_eq!(state.rip, FAULT_RIP);
    assert_eq!(state.rflags, 0x1_0202);
    assert_eq!(state.gpr[Register::Rsp as usize], ORIGINAL_RSP);
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(DATA_PHYSICAL), AccessWidth::Qword)
            .unwrap(),
        SENTINEL
    );

    let mut retry_writes = Vec::new();
    let retry = step_paged_cr0_wp_mov(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        |range| retry_writes.push(range),
    )
    .unwrap();
    assert_eq!(retry.cpu.instructions_executed, 1);
    assert!(retry.page_fault.is_none() && retry.access_error.is_none());
    assert_eq!(state.rip, FAULT_RIP + 3);
    let retry_rflags = state.rflags;
    assert_eq!(retry.ad_updates.len(), 1);
    assert_eq!(retry.ad_updates[0].address.get(), 0x6000);
    assert_eq!(retry.ad_updates[0].bit, od_machine::AdBit::Dirty);
    assert_eq!(retry.ad_updates[0].before, 0xe023);
    assert_eq!(retry.ad_updates[0].after, 0xe063);
    assert_eq!(retry.committed_writes, retry_writes);
    assert_eq!(
        retry_writes
            .iter()
            .map(|r| (r.start().get(), r.len()))
            .collect::<Vec<_>>(),
        [(0x6000, 8), (DATA_PHYSICAL, 8)]
    );
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(0x6000), AccessWidth::Qword)
            .unwrap(),
        0xe063
    );
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(DATA_PHYSICAL), AccessWidth::Qword)
            .unwrap(),
        value
    );

    assert_eq!(
        bus.read_physical(
            GuestPhysicalAddress::new(DATA_PHYSICAL + 8),
            AccessWidth::Qword,
        )
        .unwrap(),
        CANARY
    );

    let mut load_writes = Vec::new();
    let load = step_paged_cr0_wp_mov(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        |range| load_writes.push(range),
    )
    .unwrap();
    assert_eq!(load.cpu.instructions_executed, 1);
    assert!(load.page_fault.is_none() && load.access_error.is_none());
    assert!(load_writes.is_empty());
    assert_eq!(state.rip, FAULT_RIP + 6);
    assert_eq!(state.gpr[Register::Rax as usize], value);
    assert_eq!(
        bus.read_physical(
            GuestPhysicalAddress::new(DATA_PHYSICAL + 8),
            AccessWidth::Qword,
        )
        .unwrap(),
        CANARY
    );
    assert_eq!(state.gpr[Register::Rsp as usize], ORIGINAL_RSP);
    assert_eq!(state.cr2, Some(DATA_LINEAR));
    assert_eq!(state.cr0, Some(CR0));
    assert_eq!(state.cr3, Some(CR3));
    assert_eq!(state.cr4, Some(CR4));
    assert_eq!(
        retry_rflags & (1 << 16),
        0,
        "RF must clear on the retried MOV"
    );
}

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
    make_bus_with_ram_len(
        idt_present,
        gdt_present,
        lower_stack_page_present,
        handler_present,
        0x1_0000,
    )
}

fn make_bus_with_ram_len(
    idt_present: bool,
    gdt_present: bool,
    lower_stack_page_present: bool,
    handler_present: bool,
    ram_len: u64,
) -> MachineBus {
    let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), ram_len).unwrap();
    let map =
        MachineMemoryMap::new(36, vec![MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)]).unwrap();
    let mut bus = MachineBus::new(map, RamLimits::new(ram_len), vec![], vec![], vec![]).unwrap();

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
    // The default case starts with A=1 and therefore needs no descriptor write.
    // Separate cases exercise the bounded emulator profile's A=0 update.
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
    assert_eq!(record.saved_rip(), state.rip);
    assert_eq!(record.cr2_value(), DATA_LINEAR);
    assert_eq!(state.rip, record.saved_rip());
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
fn unaccessed_target_descriptor_is_updated_after_frame_before_handler_entry() {
    let mut bus = make_bus(true, true, true, true);
    put_bytes(&mut bus, 0xb00d, &[0x9a]);
    let mut state = initial_state(0x2);
    let fault = start_faulting_store(&mut bus, &mut state)
        .page_fault
        .unwrap();
    let mut committed_writes = Vec::new();
    let report = deliver_and_step_paged_pf_handler(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        &fault,
        |range| committed_writes.push(range),
    )
    .unwrap();

    assert!(matches!(
        report.delivery.delivery,
        od_isa::PageFaultDeliveryResult::Delivered {
            target_rip: HANDLER_RIP,
            new_rsp: 0x0070_1ff0,
        }
    ));
    assert_eq!(
        report
            .handler_step
            .unwrap()
            .unwrap()
            .cpu
            .instructions_executed,
        1
    );
    assert_eq!(state.rip, HANDLER_RIP + 3);
    assert_eq!(state.cs_selector, Some(0x8));
    assert_eq!(state.cs_cache.unwrap().access, 0x9b);
    assert_eq!(state.gpr[Register::Rsp as usize], 0x0070_1ff0);
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(0xb008), AccessWidth::Qword)
            .unwrap(),
        u64::from_le_bytes([0xff, 0xff, 0, 0, 0, 0x9b, 0xaf, 0])
    );
    assert_eq!(
        committed_writes
            .iter()
            .map(|range| (range.start().get(), range.len()))
            .collect::<Vec<_>>(),
        [
            (0x5810, 8),
            (0xd018, 8),
            (0xd010, 8),
            (0xd008, 8),
            (0xd000, 8),
            (0x5808, 8),
            (0xcff8, 8),
            (0xcff0, 8),
            (0x5008, 8),
            (0xb00d, 1),
        ]
    );
}

#[test]
fn read_only_unaccessed_descriptor_update_faults_after_frame_without_entry_commit() {
    let mut bus = make_bus(true, true, true, true);
    put_qword(&mut bus, 0x5008, 0xb000 | 0x25); // Present, A=1, read-only GDT mapping.
    put_bytes(&mut bus, 0xb00d, &[0x9a]);
    let mut state = initial_state(0x202);
    let fault = start_faulting_store(&mut bus, &mut state)
        .page_fault
        .unwrap();
    let before_delivery = state.clone();
    let mut committed_writes = Vec::new();
    let report = deliver_and_step_paged_pf_handler(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        &fault,
        |range| committed_writes.push(range),
    )
    .unwrap();

    assert!(matches!(
        &report.delivery.delivery,
        od_isa::PageFaultDeliveryResult::DeliveryAborted {
            phase: od_isa::PageFaultDeliveryPhase::TargetCodeDescriptorAccessedWrite,
            cause: od_isa::memory::MemoryError::Architectural(fault),
        } if fault.vector == 14
            && fault.error_code == Some(3)
            && fault.linear_address == Some(GDT_BASE + 13)
    ));
    assert!(report.handler_step.is_none());
    let secondary = report.delivery.secondary_page_fault.unwrap();
    assert_eq!(secondary.fault().vector, 14);
    assert_eq!(secondary.fault().error_code, Some(3));
    assert_eq!(secondary.cr2_value(), GDT_BASE + 13);
    assert_eq!(state.cr2, Some(GDT_BASE + 13));
    assert_eq!(state.rip, before_delivery.rip);
    assert_eq!(state.gpr[Register::Rsp as usize], ORIGINAL_RSP);
    assert_eq!(state.cs_selector, before_delivery.cs_selector);
    assert_eq!(state.cs_cache, before_delivery.cs_cache);
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(0xb008), AccessWidth::Qword)
            .unwrap(),
        u64::from_le_bytes([0xff, 0xff, 0, 0, 0, 0x9a, 0xaf, 0])
    );
    assert_eq!(
        committed_writes
            .iter()
            .map(|range| (range.start().get(), range.len()))
            .collect::<Vec<_>>(),
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
    for (physical, expected) in [
        (0xd018, 0x10),
        (0xd010, ORIGINAL_RSP),
        (0xd008, 0x1_0202),
        (0xd000, 0x8),
        (0xcff8, FAULT_RIP),
        (0xcff0, 3),
    ] {
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(physical), AccessWidth::Qword)
                .unwrap(),
            expected
        );
    }
}

// Install separate same-CPL IST=0 gates and code descriptors for #PF and #DF.
fn configure_double_fault_gates(bus: &mut MachineBus) {
    put_bytes(bus, 0xb010, &[0xff, 0xff, 0, 0, 0, 0x9a, 0xaf, 0]);
    put_bytes(bus, 0xa000 + 14 * 16 + 2, &[0x10, 0]);
    let gate = [
        (HANDLER_RIP as u16) as u8,
        (HANDLER_RIP >> 8) as u8,
        0x08,
        0,
        0,
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
    put_bytes(bus, 0xa000 + 8 * 16, &gate);
}

#[test]
fn descriptor_write_page_fault_delivers_double_fault_and_runs_handler() {
    let mut bus = make_bus(true, true, true, true);
    configure_double_fault_gates(&mut bus);
    put_qword(&mut bus, 0x5008, 0xb000 | 0x25);
    let mut state = initial_state(0x202);
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

    assert_eq!(
        report.delivery.outcome,
        PagedExceptionDeliveryOutcome::DoubleFaultDelivered
    );
    assert!(matches!(&report.delivery.delivery,
        od_isa::PageFaultDeliveryResult::DeliveryAborted {
            phase: od_isa::PageFaultDeliveryPhase::TargetCodeDescriptorAccessedWrite,
            cause: od_isa::memory::MemoryError::Architectural(fault),
        } if fault.vector == 14 && fault.error_code == Some(3)
            && fault.linear_address == Some(GDT_BASE + 0x15)));
    assert_eq!(
        report
            .delivery
            .secondary_page_fault
            .as_ref()
            .unwrap()
            .cr2_value(),
        GDT_BASE + 0x15
    );
    let double_fault = report.delivery.double_fault.as_ref().unwrap();
    assert_eq!(double_fault.original(), &fault);
    assert_eq!(
        double_fault.secondary(),
        report.delivery.secondary_page_fault.as_ref().unwrap()
    );
    assert!(matches!(
        report.delivery.double_fault_delivery,
        Some(od_isa::PageFaultDeliveryResult::Delivered {
            target_rip: HANDLER_RIP,
            new_rsp: 0x0070_1ff0
        })
    ));
    assert!(report.delivery.double_fault_secondary_page_fault.is_none());
    assert!(report.delivery.double_fault_access_error.is_none());
    assert_eq!(state.cr2, Some(GDT_BASE + 0x15));
    assert_eq!(state.rip, HANDLER_RIP + 3);
    assert_eq!(state.gpr[Register::Rax as usize], SENTINEL);
    assert_eq!(state.cs_selector, Some(0x08));
    assert_eq!(
        report
            .handler_step
            .unwrap()
            .unwrap()
            .cpu
            .instructions_executed,
        1
    );
    for (physical, expected) in [
        (0xd018, 0x10),
        (0xd010, ORIGINAL_RSP),
        (0xd008, 0x202),
        (0xd000, 0x08),
        (0xcff8, FAULT_RIP),
        (0xcff0, 0),
    ] {
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(physical), AccessWidth::Qword)
                .unwrap(),
            expected
        );
    }
    assert_eq!(
        writes
            .iter()
            .filter(|r| (0xc000..0xe000).contains(&r.start().get()))
            .count(),
        12
    );
    assert_eq!(writes, report.delivery.committed_writes);
}

#[test]
fn double_fault_stack_page_fault_shuts_down_and_preserves_partial_writes() {
    let mut bus = make_bus(true, true, false, true);
    configure_double_fault_gates(&mut bus);
    let mut state = initial_state(0x202);
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

    assert_eq!(
        report.delivery.outcome,
        PagedExceptionDeliveryOutcome::Shutdown
    );
    assert!(matches!(
        report.delivery.delivery,
        od_isa::PageFaultDeliveryResult::DeliveryAborted {
            phase: od_isa::PageFaultDeliveryPhase::StackWrite {
                slot: od_isa::PageFaultFrameSlot::SavedRip
            },
            ..
        }
    ));
    assert_eq!(
        report
            .delivery
            .secondary_page_fault
            .as_ref()
            .unwrap()
            .cr2_value(),
        0x0070_1ff8
    );
    assert!(report.delivery.double_fault.is_some());
    assert!(matches!(
        report.delivery.double_fault_delivery,
        Some(od_isa::PageFaultDeliveryResult::DeliveryAborted {
            phase: od_isa::PageFaultDeliveryPhase::StackWrite {
                slot: od_isa::PageFaultFrameSlot::SavedRip
            },
            cause: od_isa::memory::MemoryError::Architectural(_),
        })
    ));
    assert_eq!(
        report
            .delivery
            .double_fault_secondary_page_fault
            .as_ref()
            .unwrap()
            .cr2_value(),
        0x0070_1ff8
    );
    assert_eq!(state.cr2, Some(0x0070_1ff8));
    assert_eq!(state.activity, Some(CpuActivity::Shutdown));
    assert!(report.handler_step.is_none());
    assert_eq!(writes, report.delivery.committed_writes);
    assert_eq!(
        writes
            .iter()
            .filter(|r| (0xc000..0xe000).contains(&r.start().get()))
            .count(),
        8
    );
    for (physical, expected) in [
        (0xd018, 0x10),
        (0xd010, ORIGINAL_RSP),
        (0xd008, 0x202),
        (0xd000, 0x08),
    ] {
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(physical), AccessWidth::Qword)
                .unwrap(),
            expected
        );
    }
    // An unmapped instruction address makes any accidental fetch observable as #PF.
    state.rip = HANDLER_RIP + 0x1000;
    let before = state.clone();
    let mut later_writes = Vec::new();
    let step = step_paged_cr0_wp_mov(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        |r| later_writes.push(r),
    )
    .unwrap();
    assert_eq!(step.cpu.exit, od_isa::PagedProbeExit::Shutdown);
    assert_eq!(step.cpu.instructions_executed, 0);
    assert!(step.page_fault.is_none());
    assert!(step.ad_updates.is_empty());
    assert!(step.committed_writes.is_empty());
    assert!(later_writes.is_empty());
    assert_eq!(state, before);
}

#[test]
fn already_accessed_descriptor_does_not_write_read_only_gdt() {
    let mut bus = make_bus(true, true, true, true);
    put_qword(&mut bus, 0x5008, 0xb000 | 0x25); // Present, A=1, read-only GDT mapping.
    let mut state = initial_state(0x202);
    let fault = start_faulting_store(&mut bus, &mut state)
        .page_fault
        .unwrap();
    let mut committed_writes = Vec::new();
    let report = deliver_and_step_paged_pf_handler(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        &fault,
        |range| committed_writes.push(range),
    )
    .unwrap();

    assert!(matches!(
        report.delivery.delivery,
        od_isa::PageFaultDeliveryResult::Delivered { .. }
    ));
    assert_eq!(
        report
            .handler_step
            .unwrap()
            .unwrap()
            .cpu
            .instructions_executed,
        1
    );
    assert_eq!(state.cs_cache.unwrap().access, 0x9b);
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(0x5008), AccessWidth::Qword)
            .unwrap(),
        0xb000 | 0x25
    );
    assert_eq!(
        committed_writes
            .iter()
            .map(|range| range.start().get())
            .collect::<Vec<_>>(),
        [
            0x5810, 0xd018, 0xd010, 0xd008, 0xd000, 0x5808, 0xcff8, 0xcff0
        ]
    );
}

#[test]
fn invalid_unaccessed_target_is_rejected_before_frame_or_descriptor_write() {
    let mut bus = make_bus(true, true, true, true);
    put_bytes(&mut bus, 0xb00d, &[0x92]); // Present data segment, A=0, not a code target.
    let mut state = initial_state(0x202);
    let fault = start_faulting_store(&mut bus, &mut state)
        .page_fault
        .unwrap();
    let before_delivery = state.clone();
    let mut committed_writes = Vec::new();
    let report = od_machine::deliver_paged_page_fault(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        &fault,
        |range| committed_writes.push(range),
    )
    .unwrap();

    assert_eq!(
        report.delivery,
        od_isa::PageFaultDeliveryResult::OutsideProfile(
            od_isa::PageFaultDeliveryOutsideProfile::InvalidTargetCodeDescriptor
        )
    );
    assert_eq!(state.rip, before_delivery.rip);
    assert_eq!(state.gpr[Register::Rsp as usize], ORIGINAL_RSP);
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(0xb00d), AccessWidth::Byte)
            .unwrap(),
        0x92
    );
    assert!(
        !committed_writes
            .iter()
            .any(|range| (0xc000..0xe000).contains(&range.start().get()))
    );
}

#[test]
fn target_descriptor_physical_alias_with_frame_is_rejected_without_corruption() {
    let mut bus = make_bus(true, true, true, true);
    put_qword(&mut bus, 0x5008, 0xd000 | 0x27); // GDT maps onto the writable stack frame page.
    put_bytes(&mut bus, 0xd008, &[0xff, 0xff, 0, 0, 0, 0x9a, 0xaf, 0]);
    let mut state = initial_state(0x202);
    let fault = start_faulting_store(&mut bus, &mut state)
        .page_fault
        .unwrap();
    let before_delivery = state.clone();
    let mut committed_writes = Vec::new();
    let report = deliver_and_step_paged_pf_handler(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        &fault,
        |range| committed_writes.push(range),
    )
    .unwrap();

    assert!(matches!(
        &report.delivery.delivery,
        od_isa::PageFaultDeliveryResult::DeliveryAborted {
            phase: od_isa::PageFaultDeliveryPhase::TargetCodeDescriptorAccessedPreflight,
            cause: od_isa::memory::MemoryError::Backend(message),
        } if message.contains("aliases a planned frame")
    ));
    assert!(report.handler_step.is_none());
    assert_eq!(state.rip, before_delivery.rip);
    assert_eq!(state.gpr[Register::Rsp as usize], ORIGINAL_RSP);
    assert_eq!(state.cs_selector, before_delivery.cs_selector);
    assert_eq!(state.cs_cache, before_delivery.cs_cache);
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(0xd008), AccessWidth::Qword)
            .unwrap(),
        u64::from_le_bytes([0xff, 0xff, 0, 0, 0, 0x9a, 0xaf, 0])
    );
    assert!(committed_writes.is_empty());
}

#[test]
fn target_descriptor_preflight_rejects_frame_overwriting_its_gdt_pte() {
    for code_offset in [1_u64, 0x63] {
        let mut bus = make_bus_with_ram_len(true, true, true, true, 0x0040_2000);
        put_qword(&mut bus, 0x4000, 0x8000 | 0x27); // faulting code page
        put_qword(&mut bus, 0x4008, 0xb000 | 0x27); // GDT page
        put_qword(&mut bus, 0x5810, 0x4000 | 0x27); // stack maps onto the PT page
        put_bytes(&mut bus, 0x8000 + code_offset, &STORE);
        put_bytes(&mut bus, 0x0040_000d, &[0x5a]);

        let fault_rip = 0x0040_0000 + code_offset;
        let mut state = initial_state(0x202);
        state.rip = fault_rip;
        state.gdtr = Some(Gdtr {
            base: 0x0040_1000,
            limit: 0x17,
        });
        state.gpr[Register::Rsp as usize] = 0x0070_2037;
        put_bytes(&mut bus, 0xb00d, &[0x9a]); // valid target code descriptor, A=0

        let fault = start_faulting_store(&mut bus, &mut state)
            .page_fault
            .unwrap();
        let before_delivery = state.clone();
        let mut committed_writes = Vec::new();
        let report = deliver_and_step_paged_pf_handler(
            &mut state,
            &mut bus,
            CR3,
            EFER,
            context(),
            od_isa::INTEGER_FLAGS,
            &fault,
            |range| committed_writes.push(range),
        )
        .unwrap();

        assert!(matches!(
            &report.delivery.delivery,
            od_isa::PageFaultDeliveryResult::DeliveryAborted {
                phase: od_isa::PageFaultDeliveryPhase::TargetCodeDescriptorAccessedPreflight,
                cause: od_isa::memory::MemoryError::Backend(message),
            } if message.contains("aliases a planned frame")
        ));
        assert!(report.handler_step.is_none());
        assert_eq!(state.rip, before_delivery.rip);
        assert_eq!(
            state.gpr[Register::Rsp as usize],
            before_delivery.gpr[Register::Rsp as usize]
        );
        assert_eq!(state.cs_cache, before_delivery.cs_cache);
        assert!(committed_writes.is_empty());
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(0x4008), AccessWidth::Qword)
                .unwrap(),
            0xb000 | 0x27
        );
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(0x0040_000d), AccessWidth::Byte)
                .unwrap(),
            0x5a
        );
    }
}

#[test]
fn frame_store_cannot_remap_a_later_slot_onto_the_target_descriptor() {
    let mut bus = make_bus_with_ram_len(true, true, true, true, 0x0070_3000);
    put_qword(&mut bus, 0x5008, 0x0070_2000 | 0x27); // GDT maps to the future frame target
    put_qword(&mut bus, 0x5810, 0x5000 | 0x27); // stack maps onto its own PT
    put_bytes(&mut bus, 0x0070_2803, &[0xff, 0xff, 0, 0, 0, 0x9a, 0xaf, 0]);

    let mut state = initial_state(0x202);
    state.gdtr = Some(Gdtr {
        base: 0x0060_17fb,
        limit: 0x17,
    });
    state.gpr[Register::Rsp as usize] = 0x0070_2827;
    let fault = start_faulting_store(&mut bus, &mut state)
        .page_fault
        .unwrap();
    let mut committed_writes = Vec::new();
    let report = deliver_and_step_paged_pf_handler(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        &fault,
        |range| committed_writes.push(range),
    )
    .unwrap();

    assert!(matches!(
        &report.delivery.delivery,
        od_isa::PageFaultDeliveryResult::DeliveryAborted {
            phase: od_isa::PageFaultDeliveryPhase::TargetCodeDescriptorAccessedPreflight,
            cause: od_isa::memory::MemoryError::Backend(message),
        } if message.contains("aliases a planned frame")
    ));
    assert!(report.handler_step.is_none());
    assert!(committed_writes.is_empty());
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(0x5810), AccessWidth::Qword)
            .unwrap(),
        0x5000 | 0x27
    );
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(0x0070_2803), AccessWidth::Qword)
            .unwrap(),
        u64::from_le_bytes([0xff, 0xff, 0, 0, 0, 0x9a, 0xaf, 0])
    );
}

#[test]
fn first_read_only_stack_slot_overlapping_descriptor_reports_page_fault() {
    let mut bus = make_bus(true, true, true, true);
    put_qword(&mut bus, 0x5810, 0xd000 | 0x21);
    put_bytes(&mut bus, 0xd003, &[0xff, 0xff, 0, 0, 0, 0x9a, 0xaf, 0]);

    let mut state = initial_state(0x202);
    state.gdtr = Some(Gdtr {
        base: 0x0070_1ffb,
        limit: 0x17,
    });
    state.gpr[Register::Rsp as usize] = 0x0070_2017;
    let fault = start_faulting_store(&mut bus, &mut state)
        .page_fault
        .unwrap();
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

    assert!(matches!(
        &report.delivery.delivery,
        od_isa::PageFaultDeliveryResult::DeliveryAborted {
            phase: od_isa::PageFaultDeliveryPhase::StackWrite {
                slot: od_isa::PageFaultFrameSlot::OldSs,
            },
            cause: od_isa::memory::MemoryError::Architectural(fault),
        } if fault.vector == 14 && fault.error_code == Some(3)
            && fault.linear_address == Some(0x0070_2008)
    ));
    assert_eq!(state.cr2, Some(0x0070_2008));
    assert!(report.delivery.committed_writes.is_empty());
    assert!(report.handler_step.is_none());
}

#[test]
fn first_stack_fault_is_not_masked_by_an_unreachable_descriptor_alias() {
    let mut bus = make_bus_with_ram_len(true, true, true, true, 0x1_0000);
    put_qword(&mut bus, 0x5008, 0xc000 | 0x27); // descriptor and lower stack share a page
    put_qword(&mut bus, 0x5808, 0xc000 | 0x27); // lower stack page
    put_qword(&mut bus, 0x5810, 0xd000 | 0x26); // upper stack page absent
    put_bytes(&mut bus, 0xcff3, &[0xff, 0xff, 0, 0, 0, 0x9a, 0xaf, 0]);

    let mut state = initial_state(0x202);
    state.gdtr = Some(Gdtr {
        base: 0x0070_1feb, // Accessed byte coincides with a later frame slot
        limit: 0x17,
    });
    state.gpr[Register::Rsp as usize] = 0x0070_2017;
    let fault = start_faulting_store(&mut bus, &mut state)
        .page_fault
        .unwrap();
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

    assert!(matches!(
        &report.delivery.delivery,
        od_isa::PageFaultDeliveryResult::DeliveryAborted {
            phase: od_isa::PageFaultDeliveryPhase::StackWrite {
                slot: od_isa::PageFaultFrameSlot::OldSs,
            },
            cause: od_isa::memory::MemoryError::Architectural(fault),
        } if fault.vector == 14 && fault.linear_address == Some(0x0070_2008)
    ));
    assert_eq!(state.cr2, Some(0x0070_2008));
    assert!(report.delivery.committed_writes.is_empty());
}

#[test]
fn first_stack_physical_hole_error_is_not_masked_by_a_later_alias() {
    let mut bus = make_bus_with_ram_len(true, true, true, true, 0x1_0000);
    put_qword(&mut bus, 0x5008, 0xc000 | 0x27); // descriptor maps to lower stack page
    put_qword(&mut bus, 0x5808, 0xc000 | 0x27);
    put_qword(&mut bus, 0x5810, 0x1_0000 | 0x27); // first frame slot translates to a RAM hole
    put_bytes(&mut bus, 0xcff3, &[0xff, 0xff, 0, 0, 0, 0x9a, 0xaf, 0]);

    let mut state = initial_state(0x202);
    state.gdtr = Some(Gdtr {
        base: 0x0060_1feb,
        limit: 0x17,
    });
    state.gpr[Register::Rsp as usize] = 0x0070_2017;
    let fault = start_faulting_store(&mut bus, &mut state)
        .page_fault
        .unwrap();
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

    assert!(matches!(
        &report.delivery.delivery,
        od_isa::PageFaultDeliveryResult::DeliveryAborted {
            phase: od_isa::PageFaultDeliveryPhase::StackWrite {
                slot: od_isa::PageFaultFrameSlot::OldSs,
            },
            cause: od_isa::memory::MemoryError::Backend(_),
        }
    ));
    assert!(matches!(
        report.delivery.access_error,
        Some(od_machine::TrackedAccessError::OperandPhysical { address, .. })
            if address.get() == 0x1_0008
    ));
    assert!(report.delivery.committed_writes.is_empty());
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
            Some(if idt_present {
                expected_cr2
            } else {
                IDT_BASE + 8 * 16
            }),
            "delivery report: {:?}",
            report.delivery
        );
        assert_eq!(
            report.delivery.outcome,
            if idt_present {
                PagedExceptionDeliveryOutcome::Undelivered
            } else {
                PagedExceptionDeliveryOutcome::Shutdown
            }
        );
        assert!(report.delivery.double_fault.is_some());
        if !idt_present {
            assert_eq!(
                report
                    .delivery
                    .double_fault_secondary_page_fault
                    .as_ref()
                    .unwrap()
                    .cr2_value(),
                IDT_BASE + 8 * 16
            );
        }
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

#[test]
fn guest_lidt_and_lgdt_feed_the_following_page_fault_delivery() {
    const INITIAL_IDT_BASE: u64 = 0x0062_0000;
    const INITIAL_GDT_BASE: u64 = 0x0063_0000;
    const GDTR_SOURCE: u64 = 0x0070_1fd0;

    let mut bus = make_bus(true, true, true, true);
    put_qword(&mut bus, 0x5008, 0xb000 | 0x07); // GDT mapping starts Accessed=0.
    put_bytes(&mut bus, 0xb00d, &[0x9a]); // Guest-loaded target descriptor starts A=0.
    let mut instructions = vec![
        0x0f, 0x01, 0x5c, 0x24, 0x99, // LIDT [RSP-103] -> 0x701fc0
        0x0f, 0x01, 0x55, 0xf6, // LGDT [RBP-10] -> 0x701fd0
    ];
    instructions.extend_from_slice(&STORE);
    put_bytes(&mut bus, 0x8000, &instructions);
    let mut idtr_bytes = Vec::from(0x0fffu16.to_le_bytes());
    idtr_bytes.extend_from_slice(&IDT_BASE.to_le_bytes());
    put_bytes(&mut bus, 0xcfc0, &idtr_bytes);
    let mut gdtr_bytes = Vec::from(0x17u16.to_le_bytes());
    gdtr_bytes.extend_from_slice(&GDT_BASE.to_le_bytes());
    put_bytes(&mut bus, 0xcfd0, &gdtr_bytes);

    let mut state = initial_state(0x202);
    let old_cs_cache = state.cs_cache;
    let old_ss_cache = state.ss_cache;
    state.idtr = Some(Idtr {
        base: INITIAL_IDT_BASE,
        limit: 0x0fff,
    });
    state.gdtr = Some(Gdtr {
        base: INITIAL_GDT_BASE,
        limit: 0x17,
    });
    state.gpr[Register::Rbp as usize] = GDTR_SOURCE + 10;

    let lidt = step_paged_lidt(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        |_| {},
    )
    .unwrap();
    assert_eq!(lidt.cpu.instructions_executed, 1);
    assert_eq!(state.rip, FAULT_RIP + 5);
    assert_eq!(
        state.idtr,
        Some(Idtr {
            base: IDT_BASE,
            limit: 0x0fff
        })
    );
    assert_eq!(state.gdtr.unwrap().base, INITIAL_GDT_BASE);

    let lgdt = step_paged_lgdt(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        |_| {},
    )
    .unwrap();
    assert_eq!(lgdt.cpu.instructions_executed, 1);
    assert_eq!(state.rip, FAULT_RIP + 9);
    assert_eq!(
        state.gdtr,
        Some(Gdtr {
            base: GDT_BASE,
            limit: 0x17
        })
    );
    assert_eq!(state.cs_cache, old_cs_cache);
    assert_eq!(state.ss_cache, old_ss_cache);

    let fault_step = step_paged_cr0_wp_mov(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        |_| {},
    )
    .unwrap();
    assert_eq!(fault_step.cpu.instructions_executed, 0);
    let original_fault = fault_step.page_fault.unwrap();
    assert_eq!(original_fault.fault().error_code, Some(3));
    assert_eq!(original_fault.saved_rip(), FAULT_RIP + 9);
    assert_eq!(original_fault.cr2_value(), DATA_LINEAR);

    let mut committed_writes = Vec::new();
    let handler = deliver_and_step_paged_pf_handler(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        od_isa::INTEGER_FLAGS,
        &original_fault,
        |range| committed_writes.push(range),
    )
    .unwrap();
    assert!(matches!(
        handler.delivery.delivery,
        od_isa::PageFaultDeliveryResult::Delivered { .. }
    ));
    assert_eq!(handler.delivery.original_fault.saved_rip(), FAULT_RIP + 9);
    assert_eq!(state.cr2, Some(DATA_LINEAR));
    assert_eq!(state.rip, HANDLER_RIP + 3);
    assert_eq!(state.gpr[Register::Rax as usize], SENTINEL);
    assert_eq!(state.gpr[Register::Rsp as usize], 0x0070_1ff0);
    assert!(handler.delivery.secondary_page_fault.is_none());
    assert_eq!(state.cs_cache.unwrap().flags, 0xa);
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(0x5008), AccessWidth::Qword)
            .unwrap(),
        0xb000 | 0x67
    );
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(0xb00d), AccessWidth::Byte)
            .unwrap(),
        0x9b
    );
    assert_eq!(
        handler
            .delivery
            .ad_updates
            .iter()
            .map(|update| (update.address.get(), update.bit))
            .collect::<Vec<_>>(),
        [
            (0x5008, od_machine::AdBit::Accessed),
            (0x5810, od_machine::AdBit::Dirty),
            (0x5808, od_machine::AdBit::Dirty),
            (0x5008, od_machine::AdBit::Dirty),
        ]
    );
    let expected_frame = [
        (0x0070_2018, 0x10),
        (0x0070_2010, ORIGINAL_RSP),
        (0x0070_2008, 0x1_0202),
        (0x0070_2000, 0x8),
        (0x0070_1ff8, FAULT_RIP + 9),
        (0x0070_1ff0, 3),
    ];
    let expected_writes = [
        (0x5008, 8),
        (0x5810, 8),
        (0xd018, 8),
        (0xd010, 8),
        (0xd008, 8),
        (0xd000, 8),
        (0x5808, 8),
        (0xcff8, 8),
        (0xcff0, 8),
        (0x5008, 8),
        (0xb00d, 1),
    ];
    assert_eq!(
        committed_writes
            .iter()
            .map(|range| (range.start().get(), range.len()))
            .collect::<Vec<_>>(),
        expected_writes
    );
    for (index, (linear, expected)) in expected_frame.into_iter().enumerate() {
        let physical = if linear >= 0x0070_2000 {
            0xd000 + (linear & 0xfff)
        } else {
            0xc000 + (linear & 0xfff)
        };
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(physical), AccessWidth::Qword)
                .unwrap(),
            expected,
            "frame slot {index}"
        );
    }
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(0xcff8), AccessWidth::Qword)
            .unwrap(),
        FAULT_RIP + 9
    );
}

#[test]
fn cpl3_mov_after_iretq_enforces_user_page_permissions() {
    use od_isa::execute::PagedProbeExit;
    use od_isa::iretq::IretqResult;
    use od_machine::{AdBit, step_paged_cpl3_mov, step_paged_iretq_and_user_load};
    for (entry, clear) in [
        (None, 0),
        (None, 0x200),
        (Some(0x1008), 2),
        (Some(0xf000), 2),
        (Some(0x10000), 2),
        (Some(0x11000), 2),
        (Some(0x1008), 4),
        (Some(0xf000), 4),
        (Some(0x10000), 4),
        (Some(0x11000), 4),
        (Some(0x11000), 0x100),
    ] {
        let mut bus = make_bus_with_ram_len(true, true, true, true, 0x20000);
        put_bytes(&mut bus, 0x8000, &[0x48, 0xcf]);
        let user_rip = HANDLER_RIP + if clear == 0x200 { 0xffd } else { 0 };
        if clear == 0x200 {
            put_bytes(&mut bus, 0x9ffd, &[0x48, 0x89, 0x0b]);
        } else if matches!(clear, 0 | 2) {
            put_bytes(
                &mut bus,
                0x9000,
                &[
                    0x48, 0x8b, 0x03, 0x48, 0x89, 0x0b, 0x48, 0x8b, 0x03, 0x0f, 0x22, 0xc0,
                ],
            );
        } else {
            put_bytes(&mut bus, 0x9000, &[0x48, 0x89, 0x0b, 0x48, 0x8b, 0x03]);
        }
        put_bytes(&mut bus, 0xb018, &[0xff, 0xff, 0, 0, 0, 0xfb, 0xaf, 0]);
        put_bytes(&mut bus, 0xb020, &[0xff, 0xff, 0, 0, 0, 0xf3, 0xcf, 0]);
        for (address, value) in [
            (0x1008, 0xf007),
            (0xf000, 0x10007),
            (0x10000, 0x11007),
            (0x11000, DATA_PHYSICAL | 7),
        ] {
            put_qword(&mut bus, address, value);
        }
        for (offset, value) in [
            (0, user_rip),
            (8, 0x1b),
            (16, 0x202),
            (24, 0x0070_2000),
            (32, 0x23),
        ] {
            put_qword(&mut bus, 0xd027 + offset, value);
        }
        if let Some(address) = entry {
            let value = bus
                .read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                .unwrap();
            put_qword(
                &mut bus,
                address,
                if clear == 0x100 {
                    0x30007
                } else {
                    value & !clear
                },
            );
        }
        let mut state = initial_state(0x202);
        state.cs_cache.as_mut().unwrap().flags = 0xa;
        state.gpr[Register::Rbx as usize] = 0x80_0000_0000;
        state.gpr[Register::Rcx as usize] = 0xabc0_1234_5678_9012;
        let initial = bus
            .read_physical(GuestPhysicalAddress::new(DATA_PHYSICAL), AccessWidth::Qword)
            .unwrap();
        let mut prior_writes = Vec::new();
        let iret =
            step_paged_iretq_and_user_load(&mut state, &mut bus, od_isa::INTEGER_FLAGS, |r| {
                prior_writes.push(r)
            })
            .unwrap();
        assert_eq!(iret.iretq.cpu, IretqResult::Retired);
        let first_user = iret.mov.unwrap();
        if matches!(clear, 0 | 2) {
            assert_eq!(first_user.cpu.exit, PagedProbeExit::Retired);
            assert_eq!(state.rip, user_rip + 3);
        } else {
            assert!(matches!(
                first_user.cpu.exit,
                PagedProbeExit::Unsupported { .. }
            ));
        }
        let user_rip = state.rip;
        let expected_prior_writes = [
            iret.iretq.committed_writes.clone(),
            first_user.committed_writes.clone(),
        ]
        .concat();
        assert_eq!(prior_writes, expected_prior_writes);
        let table_addresses = [0x1008, 0xf000, 0x10000, 0x11000];
        let before_entries = table_addresses.map(|address| {
            bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                .unwrap()
        });
        let committed = state.clone();
        let mut writes = Vec::new();
        let store = step_paged_cpl3_mov(&mut state, &mut bus, od_isa::INTEGER_FLAGS, |r| {
            writes.push(r)
        })
        .unwrap();
        assert_eq!(writes, store.committed_writes);
        let retired_store = store.cpu.exit == PagedProbeExit::Retired;
        let mut expected_updates = Vec::new();
        let mut after_entries = before_entries;
        for (index, address) in table_addresses.into_iter().enumerate() {
            if after_entries[index] & 0x20 == 0 {
                expected_updates.push((address, AdBit::Accessed));
                after_entries[index] |= 0x20;
            }
        }
        if retired_store {
            expected_updates.push((0x11000, AdBit::Dirty));
            after_entries[3] |= 0x40;
        }
        assert_eq!(
            store
                .ad_updates
                .iter()
                .map(|u| (u.address.get(), u.bit))
                .collect::<Vec<_>>(),
            expected_updates
        );
        let mut expected_ranges = expected_updates
            .iter()
            .map(|(address, _)| (*address, 8))
            .collect::<Vec<_>>();
        if retired_store {
            expected_ranges.push((DATA_PHYSICAL, 8));
        }
        assert_eq!(
            writes
                .iter()
                .map(|r| (r.start().get(), r.len()))
                .collect::<Vec<_>>(),
            expected_ranges
        );
        for (address, expected) in table_addresses.into_iter().zip(after_entries) {
            assert_eq!(
                bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                    .unwrap(),
                expected
            );
        }
        assert_eq!(state.cs_selector, committed.cs_selector);
        assert_eq!(state.cs_cache, committed.cs_cache);
        assert_eq!(state.ss_selector, committed.ss_selector);
        assert_eq!(state.ss_cache, committed.ss_cache);
        assert_eq!(state.rflags, committed.rflags);
        assert_eq!(store.cpu.rflags_defined, od_isa::INTEGER_FLAGS);
        assert_eq!(
            state.gpr[Register::Rsp as usize],
            committed.gpr[Register::Rsp as usize]
        );
        assert_eq!(state.cr3, committed.cr3);
        if entry.is_none() {
            assert_eq!(store.cpu.exit, PagedProbeExit::Retired);
            assert_eq!(state.rip, user_rip + 3);
            assert_eq!(
                bus.read_physical(GuestPhysicalAddress::new(DATA_PHYSICAL), AccessWidth::Qword)
                    .unwrap(),
                0xabc0_1234_5678_9012
            );
            assert!(store.ad_updates.iter().any(|u| u.bit == AdBit::Dirty));
            let load =
                step_paged_cpl3_mov(&mut state, &mut bus, od_isa::INTEGER_FLAGS, |_| {}).unwrap();
            if clear == 0x200 {
                let fault = load.page_fault.unwrap();
                assert_eq!(fault.fault().error_code, Some(0x14));
                assert_eq!(fault.saved_rip(), user_rip + 3);
                assert_eq!(fault.cr2_value(), user_rip + 3);
                assert_eq!(state.rip, user_rip + 3);
                assert_eq!(
                    bus.read_physical(GuestPhysicalAddress::new(DATA_PHYSICAL), AccessWidth::Qword)
                        .unwrap(),
                    0xabc0_1234_5678_9012
                );
            } else {
                assert_eq!(load.cpu.exit, PagedProbeExit::Retired);
                assert_eq!(state.gpr[Register::Rax as usize], 0xabc0_1234_5678_9012);
                assert_eq!(state.rip, HANDLER_RIP + 9);
                let before = state.clone();
                let fault = od_machine::step_paged_cpl3_mov_cr0(
                    &mut state,
                    &mut bus,
                    od_isa::INTEGER_FLAGS,
                    |_| {},
                )
                .unwrap();
                assert!(
                    matches!(fault.cpu.exit, PagedProbeExit::Fault(ref f) if f.vector == 13 && f.error_code == Some(0))
                );
                assert_eq!(state, before);
                assert_eq!(
                    bus.read_physical(GuestPhysicalAddress::new(DATA_PHYSICAL), AccessWidth::Qword)
                        .unwrap(),
                    0xabc0_1234_5678_9012
                );
            }
        } else if clear == 0x100 {
            assert!(matches!(store.cpu.exit, PagedProbeExit::Backend(_)));
            assert!(store.page_fault.is_none());
            assert!(matches!(
                store.access_error,
                Some(od_machine::TrackedAccessError::OperandPhysical { address, .. })
                    if address.get() == 0x30000
            ));
            assert_eq!(state.rip, HANDLER_RIP);
            assert_eq!(state.cr2, committed.cr2);
            assert!(writes.iter().all(|r| r.start().get() != 0x30000));
        } else {
            let fault = store.page_fault.unwrap();
            assert_eq!(fault.fault().error_code, Some(7));
            assert_eq!(fault.cr2_value(), 0x80_0000_0000);
            assert_eq!(fault.saved_rip(), user_rip);
            assert_eq!(state.cr2, Some(0x80_0000_0000));
            assert_eq!(state.rip, user_rip);
            assert!(store.ad_updates.iter().all(|u| u.bit == AdBit::Accessed));
            assert!(writes.iter().all(|r| r.start().get() != DATA_PHYSICAL));
            assert_eq!(
                bus.read_physical(GuestPhysicalAddress::new(DATA_PHYSICAL), AccessWidth::Qword)
                    .unwrap(),
                initial
            );
            if clear == 2 {
                state.rip = HANDLER_RIP + 6;
                let load = step_paged_cpl3_mov(&mut state, &mut bus, od_isa::INTEGER_FLAGS, |_| {})
                    .unwrap();
                assert_eq!(load.cpu.exit, PagedProbeExit::Retired);
                assert_eq!(state.gpr[Register::Rax as usize], initial);
            }
            if clear == 4 {
                state.rip = HANDLER_RIP + 3;
                let load = step_paged_cpl3_mov(&mut state, &mut bus, od_isa::INTEGER_FLAGS, |_| {})
                    .unwrap();
                let fault = load.page_fault.unwrap();
                assert_eq!(fault.fault().error_code, Some(5));
                assert_eq!(fault.cr2_value(), 0x80_0000_0000);
                assert_eq!(fault.saved_rip(), HANDLER_RIP + 3);
                assert_eq!(state.cr2, Some(0x80_0000_0000));
            }
        }
    }
}

#[test]
fn cpl3_mov_entry_is_first_user_fetch_and_reports_exact_page_walk_effects() {
    use od_isa::{execute::PagedProbeExit, iretq::IretqResult};
    use od_machine::{AdBit, step_paged_cpl3_mov, step_paged_iretq_to_cpl3};

    const USER_CODE: u64 = 0x80_0040_2000;
    let code_pte = [
        (0x1008, 0xf007),
        (0xf000, 0x10007),
        (0x10010, 0x12007),
        (0x12010, 0x9007),
    ];
    for (fault_level, nx) in [
        (0, false),
        (1, false),
        (2, false),
        (3, false),
        (3, true),
        (4, false),
    ] {
        let mut bus = make_bus_with_ram_len(true, true, true, true, 0x20000);
        put_bytes(&mut bus, 0x8000, &[0x48, 0xcf]);
        put_bytes(&mut bus, 0x9000, &LOAD);
        put_bytes(&mut bus, 0xb018, &[0xff, 0xff, 0, 0, 0, 0xfb, 0xaf, 0]);
        put_bytes(&mut bus, 0xb020, &[0xff, 0xff, 0, 0, 0, 0xf3, 0xcf, 0]);
        for (address, value) in code_pte {
            put_qword(&mut bus, address, value);
        }
        // The user data mapping is separate from the user code leaf.
        for (address, value) in [(0x10000, 0x11007), (0x11000, DATA_PHYSICAL | 7)] {
            put_qword(&mut bus, address, value);
        }
        for (offset, value) in [
            (0, USER_CODE),
            (8, 0x1b),
            (16, 0x202),
            (24, 0x0070_2000),
            (32, 0x23),
        ] {
            put_qword(&mut bus, 0xd027 + offset, value);
        }
        let mut expected_code_ptes = code_pte.map(|(a, v)| (a, v));
        if nx {
            expected_code_ptes[3].1 |= 1 << 63;
            put_qword(&mut bus, expected_code_ptes[3].0, expected_code_ptes[3].1);
        } else if fault_level == 4 {
            expected_code_ptes[3].1 &= !1;
            put_qword(&mut bus, expected_code_ptes[3].0, expected_code_ptes[3].1);
        } else {
            expected_code_ptes[fault_level].1 &= !4;
            put_qword(
                &mut bus,
                expected_code_ptes[fault_level].0,
                expected_code_ptes[fault_level].1,
            );
        }
        let mut state = initial_state(0x202);
        state.cs_cache.as_mut().unwrap().flags = 0xa;
        state.gpr[Register::Rbx as usize] = 0x80_0000_0000;
        let mut callbacks = Vec::new();
        let iret = step_paged_iretq_to_cpl3(&mut state, &mut bus, od_isa::INTEGER_FLAGS, |r| {
            callbacks.push(r)
        })
        .unwrap();
        assert_eq!(iret.cpu, IretqResult::Retired);
        // None of these user-code paging entries was touched by the ring-0 return.
        for (address, _) in expected_code_ptes {
            assert_eq!(
                bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                    .unwrap()
                    & 0x20,
                0
            );
        }
        callbacks.clear();
        let before = state.clone();
        let mov = step_paged_cpl3_mov(&mut state, &mut bus, od_isa::INTEGER_FLAGS, |r| {
            callbacks.push(r)
        })
        .unwrap();
        let fault = mov.page_fault.as_ref().unwrap();
        assert_eq!(mov.cpu.exit, PagedProbeExit::Fault(fault.fault().clone()));
        assert_eq!(mov.cpu.instructions_executed, 0);
        assert_eq!(
            fault.fault().error_code,
            Some(if fault_level == 4 { 0x14 } else { 0x15 })
        );
        assert_eq!(fault.cr2_value(), USER_CODE);
        assert_eq!(fault.saved_rip(), USER_CODE);
        assert_eq!(state.cr2, Some(USER_CODE));
        assert_eq!(state.rip, before.rip);
        assert_eq!(state.cs_selector, before.cs_selector);
        assert_eq!(state.ss_selector, before.ss_selector);
        assert_eq!(state.gpr, before.gpr);
        let expected_updates = [
            (0x1008, expected_code_ptes[0].1 | 0x20),
            (0xf000, expected_code_ptes[1].1 | 0x20),
            (0x10010, expected_code_ptes[2].1 | 0x20),
            (
                0x12010,
                if fault_level == 4 {
                    expected_code_ptes[3].1
                } else {
                    expected_code_ptes[3].1 | 0x20
                },
            ),
        ];
        let expected_updates = if fault_level == 4 {
            &expected_updates[..3]
        } else {
            &expected_updates[..]
        };
        assert_eq!(
            mov.ad_updates
                .iter()
                .map(|u| (u.address.get(), u.after, u.bit))
                .collect::<Vec<_>>(),
            expected_updates
                .iter()
                .copied()
                .map(|(address, after)| (address, after, AdBit::Accessed))
                .collect::<Vec<_>>()
        );
        let expected_callbacks = [0x1008, 0xf000, 0x10010, 0x12010]
            .into_iter()
            .take(expected_updates.len())
            .map(|address| GuestPhysicalRange::new(GuestPhysicalAddress::new(address), 8).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(callbacks, expected_callbacks);
        for (address, expected) in expected_updates.iter().copied() {
            assert_eq!(
                bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                    .unwrap(),
                expected
            );
        }
        assert!(mov.ad_updates.iter().all(|u| u.bit == AdBit::Accessed));
    }

    for store in [false, true] {
        let mut bus = make_bus_with_ram_len(true, true, true, true, 0x20000);
        put_bytes(&mut bus, 0x8000, &[0x48, 0xcf]);
        put_bytes(
            &mut bus,
            0x9000,
            if store { &[0x48, 0x89, 0x0b] } else { &LOAD },
        );
        put_bytes(&mut bus, 0xb018, &[0xff, 0xff, 0, 0, 0, 0xfb, 0xaf, 0]);
        put_bytes(&mut bus, 0xb020, &[0xff, 0xff, 0, 0, 0, 0xf3, 0xcf, 0]);
        for (address, value) in code_pte {
            put_qword(&mut bus, address, value);
        }
        put_qword(&mut bus, 0x10000, 0x11007);
        put_qword(&mut bus, 0x11000, DATA_PHYSICAL | 6); // nonpresent, user RW bits retained
        for (offset, value) in [
            (0, USER_CODE),
            (8, 0x1b),
            (16, 0x202),
            (24, 0x0070_2000),
            (32, 0x23),
        ] {
            put_qword(&mut bus, 0xd027 + offset, value);
        }
        let mut state = initial_state(0x202);
        state.cs_cache.as_mut().unwrap().flags = 0xa;
        state.gpr[Register::Rbx as usize] = 0x80_0000_0000;
        state.gpr[Register::Rcx as usize] = 0xabc0_1234_5678_9012;
        let iret =
            step_paged_iretq_to_cpl3(&mut state, &mut bus, od_isa::INTEGER_FLAGS, |_| {}).unwrap();
        assert_eq!(iret.cpu, IretqResult::Retired);
        let before = state.clone();
        let mut callbacks = Vec::new();
        let mov = step_paged_cpl3_mov(&mut state, &mut bus, od_isa::INTEGER_FLAGS, |r| {
            callbacks.push(r)
        })
        .unwrap();
        let fault = mov.page_fault.as_ref().unwrap();
        assert_eq!(
            fault.fault().error_code,
            Some(if store { 0x06 } else { 0x04 })
        );
        assert_eq!(fault.cr2_value(), 0x80_0000_0000);
        assert_eq!(fault.saved_rip(), USER_CODE);
        assert_eq!(state.cr2, Some(0x80_0000_0000));
        assert_eq!(state.rip, USER_CODE);
        assert_eq!(mov.cpu.instructions_executed, 0);
        assert_eq!(
            state.gpr[Register::Rax as usize],
            before.gpr[Register::Rax as usize]
        );
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(0x11000), AccessWidth::Qword)
                .unwrap(),
            DATA_PHYSICAL | 6
        );
        assert!(mov.ad_updates.iter().all(|u| u.bit == AdBit::Accessed));
        assert_eq!(
            mov.ad_updates
                .iter()
                .map(|u| (u.address.get(), u.after))
                .collect::<Vec<_>>(),
            [
                (0x1008, 0xf027),
                (0xf000, 0x10027),
                (0x10010, 0x12027),
                (0x12010, 0x9027),
                (0x10000, 0x11027)
            ]
        );
        assert_eq!(
            callbacks,
            [0x1008, 0xf000, 0x10010, 0x12010, 0x10000].map(|address| GuestPhysicalRange::new(
                GuestPhysicalAddress::new(address),
                8
            )
            .unwrap())
        );
        for (address, expected) in [
            (0x1008, 0xf027),
            (0xf000, 0x10027),
            (0x10010, 0x12027),
            (0x12010, 0x9027),
            (0x10000, 0x11027),
            (0x11000, DATA_PHYSICAL | 6),
        ] {
            assert_eq!(
                bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                    .unwrap(),
                expected
            );
        }
        assert!(callbacks.iter().all(|r| r.start().get() != DATA_PHYSICAL));
    }
}

#[test]
fn user_return_and_load_enforces_paging_privileges() {
    use od_isa::execute::PagedProbeExit;
    use od_isa::iretq::IretqResult;
    use od_machine::{AdBit, step_paged_iretq_and_user_load};

    // Each entry is made supervisor-only in turn. Shared upper levels also
    // deny the instruction fetch before a data access can be attempted.
    for (entry, data_fault) in [
        (None, false),
        (Some(0x1000), false),
        (Some(0x2000), false),
        (Some(0x3010), false),
        (Some(0x4010), false),
        (Some(0x1008), true),
        (Some(0xf000), true),
        (Some(0x10000), true),
        (Some(0x11000), true),
    ] {
        let mut bus = make_bus_with_ram_len(true, true, true, true, 0x20000);
        put_bytes(&mut bus, 0x8000, &[0x48, 0xcf]);
        put_bytes(&mut bus, 0x9000, &LOAD);
        put_bytes(&mut bus, 0xb018, &[0xff, 0xff, 0, 0, 0, 0xfb, 0xaf, 0]);
        put_bytes(&mut bus, 0xb020, &[0xff, 0xff, 0, 0, 0, 0xf3, 0xcf, 0]);
        put_qword(&mut bus, 0x1008, 0xf007);
        put_qword(&mut bus, 0xf000, 0x10007);
        put_qword(&mut bus, 0x10000, 0x11007);
        put_qword(&mut bus, 0x11000, DATA_PHYSICAL | 7);
        let frame = 0xd027;
        for (offset, value) in [
            (0, HANDLER_RIP),
            (8, 0x1b),
            (16, 0x202),
            (24, 0x0070_2000),
            (32, 0x23),
        ] {
            put_qword(&mut bus, frame + offset, value);
        }
        if let Some(address) = entry {
            let value = bus
                .read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                .unwrap();
            put_qword(&mut bus, address, value & !4);
        }
        let mut state = initial_state(0x202);
        state.cs_cache.as_mut().unwrap().flags = 0xa;
        state.gpr[Register::Rbx as usize] = 0x80_0000_0000;
        let mut writes = Vec::new();
        let report =
            step_paged_iretq_and_user_load(&mut state, &mut bus, od_isa::INTEGER_FLAGS, |range| {
                writes.push(range)
            })
            .unwrap();
        assert_eq!(report.iretq.cpu, IretqResult::Retired, "entry={entry:?}");
        let mov = report.mov.unwrap();
        assert_eq!(state.cs_selector, Some(0x1b));
        assert_eq!(state.cr3, Some(CR3));
        assert_eq!(
            writes,
            [
                report.iretq.committed_writes.clone(),
                mov.committed_writes.clone()
            ]
            .concat()
        );
        assert!(mov.ad_updates.iter().all(|a| a.bit == AdBit::Accessed));
        assert!(
            mov.committed_writes
                .iter()
                .all(|r| r.start().get() != DATA_PHYSICAL)
        );
        if entry.is_none() {
            assert_eq!(mov.cpu.exit, PagedProbeExit::Retired);
            assert_eq!(state.gpr[Register::Rax as usize], SENTINEL);
            assert_eq!(state.rip, HANDLER_RIP + 3);
            assert_eq!(
                mov.ad_updates
                    .iter()
                    .map(|a| (a.address.get(), a.bit))
                    .collect::<Vec<_>>(),
                [
                    (0x1008, AdBit::Accessed),
                    (0xf000, AdBit::Accessed),
                    (0x10000, AdBit::Accessed),
                    (0x11000, AdBit::Accessed),
                ]
            );
            assert_eq!(
                mov.committed_writes
                    .iter()
                    .map(|r| (r.start().get(), r.len()))
                    .collect::<Vec<_>>(),
                [(0x1008, 8), (0xf000, 8), (0x10000, 8), (0x11000, 8)]
            );
            for (address, expected) in [
                (0x1008, 0xf027),
                (0xf000, 0x10027),
                (0x10000, 0x11027),
                (0x11000, DATA_PHYSICAL | 0x27),
            ] {
                assert_eq!(
                    bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                        .unwrap(),
                    expected
                );
            }
            assert!(
                mov.committed_writes
                    .iter()
                    .all(|r| r.start().get() != DATA_PHYSICAL)
            );
        } else {
            let fault = mov.page_fault.unwrap();
            let address = if data_fault {
                0x80_0000_0000
            } else {
                HANDLER_RIP
            };
            assert_eq!(
                fault.fault().error_code,
                Some(if data_fault { 5 } else { 0x15 })
            );
            assert_eq!(fault.cr2_value(), address);
            assert_eq!(fault.saved_rip(), HANDLER_RIP);
            assert_eq!(state.cr2, Some(address));
            assert_eq!(state.rip, HANDLER_RIP);
            assert!(matches!(mov.cpu.exit, PagedProbeExit::Fault(_)));
            if data_fault && entry == Some(0x11000) {
                assert_eq!(
                    mov.ad_updates
                        .iter()
                        .map(|a| (a.address.get(), a.bit))
                        .collect::<Vec<_>>(),
                    [
                        (0x1008, AdBit::Accessed),
                        (0xf000, AdBit::Accessed),
                        (0x10000, AdBit::Accessed),
                        (0x11000, AdBit::Accessed),
                    ]
                );
                assert_eq!(
                    mov.committed_writes
                        .iter()
                        .map(|r| (r.start().get(), r.len()))
                        .collect::<Vec<_>>(),
                    [(0x1008, 8), (0xf000, 8), (0x10000, 8), (0x11000, 8)]
                );
                for (pte, value) in [
                    (0x1008, 0xf027),
                    (0xf000, 0x10027),
                    (0x10000, 0x11027),
                    (0x11000, DATA_PHYSICAL | 0x23),
                ] {
                    assert_eq!(
                        bus.read_physical(GuestPhysicalAddress::new(pte), AccessWidth::Qword)
                            .unwrap(),
                        value
                    );
                }
                assert!(
                    mov.committed_writes
                        .iter()
                        .all(|r| r.start().get() != DATA_PHYSICAL)
                );
            }
        }
    }
}

#[test]
fn user_fetch_at_page_offset_ffd_reads_three_bytes_and_faults_with_user_pfec() {
    use od_isa::execute::PagedProbeExit;
    use od_machine::step_paged_iretq_and_user_load;
    const USER_RIP: u64 = 0x0040_2ffd;
    for (code, supervisor, unsupported) in [
        (&LOAD[..], false, false),
        (&LOAD[..], true, false),
        (&[0x48, 0x8b, 0x43][..], false, true),
        (&[0xf0, 0x0f, 0x22][..], false, true),
    ] {
        let mut bus = make_bus_with_ram_len(true, true, true, true, 0x20000);
        put_bytes(&mut bus, 0x8000, &[0x48, 0xcf]);
        put_bytes(&mut bus, 0x9ffd, code);
        put_bytes(&mut bus, 0xb018, &[0xff, 0xff, 0, 0, 0, 0xfb, 0xaf, 0]);
        put_bytes(&mut bus, 0xb020, &[0xff, 0xff, 0, 0, 0, 0xf3, 0xcf, 0]);
        put_qword(&mut bus, 0x1008, 0xf007);
        put_qword(&mut bus, 0xf000, 0x10007);
        put_qword(&mut bus, 0x10000, 0x11007);
        put_qword(&mut bus, 0x11000, DATA_PHYSICAL | 7);
        for (offset, value) in [
            (0, USER_RIP),
            (8, 0x1b),
            (16, 0x202),
            (24, 0x0070_2000),
            (32, 0x23),
        ] {
            put_qword(&mut bus, 0xd027 + offset, value);
        }
        if supervisor {
            put_qword(&mut bus, 0x4010, 0x9000 | 0x27 & !4);
        }
        let mut state = initial_state(0x202);
        state.cs_cache.as_mut().unwrap().flags = 0xa;
        state.gpr[Register::Rbx as usize] = 0x80_0000_0000;
        let report =
            step_paged_iretq_and_user_load(&mut state, &mut bus, od_isa::INTEGER_FLAGS, |_| {})
                .unwrap();
        assert_eq!(report.iretq.cpu, od_isa::iretq::IretqResult::Retired);
        let mov = report.mov.unwrap();
        if unsupported {
            assert!(matches!(mov.cpu.exit, PagedProbeExit::Unsupported { .. }));
            assert!(mov.page_fault.is_none());
            assert!(mov.access_error.is_none());
            assert_eq!(state.rip, USER_RIP);
        } else if supervisor {
            let fault = mov.page_fault.unwrap();
            assert_eq!(fault.fault().error_code, Some(0x15));
            assert_eq!(fault.cr2_value(), USER_RIP);
            assert_eq!(fault.saved_rip(), USER_RIP);
            assert_eq!(state.cr2, Some(USER_RIP));
            assert_eq!(state.rip, USER_RIP);
            assert_eq!(state.cs_selector, Some(0x1b));
        } else {
            assert_eq!(mov.cpu.exit, PagedProbeExit::Retired);
            assert_eq!(state.gpr[Register::Rax as usize], SENTINEL);
            assert_eq!(state.rip, USER_RIP + 3);
            assert_eq!(state.cr3, Some(CR3));
        }
    }
}

#[test]
fn iretq_ss_descriptor_fault_keeps_cpu_state_uncommitted() {
    use od_isa::iretq::IretqResult;
    use od_machine::step_paged_iretq_and_user_load;
    let mut bus = make_bus(true, true, true, true);
    put_bytes(&mut bus, 0x8000, &[0x48, 0xcf]);
    // CS is the last descriptor in the mapped GDT page; SS is the first byte
    // range on the following, absent page.
    put_bytes(&mut bus, 0xbff8, &[0xff, 0xff, 0, 0, 0, 0xfb, 0xaf, 0]);
    let mut state = initial_state(0x202);
    state.cs_cache.as_mut().unwrap().flags = 0xa;
    state.gdtr.as_mut().unwrap().base = 0x601fe0;
    state.gdtr.as_mut().unwrap().limit = 0x1000;
    state.gpr[Register::Rsp as usize] = 0x702000;
    for (offset, value) in [
        (0, HANDLER_RIP),
        (8, 0x1b),
        (16, 0x202),
        (24, 0x702100),
        (32, 0x23),
    ] {
        put_qword(&mut bus, 0xd000 + offset, value);
    }
    let mut expected = state.clone();
    expected.cr2 = Some(0x602000);
    let report =
        step_paged_iretq_and_user_load(&mut state, &mut bus, od_isa::INTEGER_FLAGS, |_| {})
            .unwrap();
    assert!(
        matches!(report.iretq.cpu, IretqResult::Fault(_)),
        "{:?}",
        report.iretq
    );
    assert_eq!(
        report.iretq.page_fault.as_ref().unwrap().cr2_value(),
        0x602000
    );
    assert_eq!(
        report.iretq.page_fault.as_ref().unwrap().saved_rip(),
        FAULT_RIP
    );
    assert_eq!(state, expected);
    assert!(report.mov.is_none());
}

#[test]
fn user_return_preserves_cpl3_on_backend_read_abort() {
    use od_isa::execute::PagedProbeExit;
    use od_isa::iretq::IretqResult;
    use od_machine::{TrackedAccessError, step_paged_iretq_and_user_load};

    let mut bus = make_bus_with_ram_len(true, true, true, true, 0x20000);
    put_bytes(&mut bus, 0x8000, &[0x48, 0xcf]);
    put_bytes(&mut bus, 0x9000, &LOAD);
    put_bytes(&mut bus, 0xb018, &[0xff, 0xff, 0, 0, 0, 0xfb, 0xaf, 0]);
    put_bytes(&mut bus, 0xb020, &[0xff, 0xff, 0, 0, 0, 0xf3, 0xcf, 0]);
    put_qword(&mut bus, 0x1008, 0xf007);
    put_qword(&mut bus, 0xf000, 0x10007);
    put_qword(&mut bus, 0x10000, 0x11007);
    put_qword(&mut bus, 0x11000, 0x30007); // present mapping to physical hole
    for (offset, value) in [
        (0, HANDLER_RIP),
        (8, 0x1b),
        (16, 0x202),
        (24, 0x0070_2000),
        (32, 0x23),
    ] {
        put_qword(&mut bus, 0xd027 + offset, value);
    }
    let mut state = initial_state(0x202);
    state.cs_cache.as_mut().unwrap().flags = 0xa;
    state.gpr[Register::Rbx as usize] = 0x80_0000_0000;
    state.cr2 = Some(0x1234);
    let mut callbacks = Vec::new();
    let report =
        step_paged_iretq_and_user_load(&mut state, &mut bus, od_isa::INTEGER_FLAGS, |range| {
            callbacks.push(range)
        })
        .unwrap();
    assert_eq!(report.iretq.cpu, IretqResult::Retired);
    let mov = report.mov.unwrap();
    assert!(matches!(mov.cpu.exit, PagedProbeExit::Backend(_)));
    assert!(matches!(
        mov.access_error,
        Some(TrackedAccessError::OperandPhysical { .. })
    ));
    assert!(mov.page_fault.is_none());
    assert_eq!(state.cr2, Some(0x1234));
    assert_eq!(state.cs_selector, Some(0x1b));
    assert_eq!(state.rip, HANDLER_RIP);
    assert_eq!(state.cr3, Some(CR3));
    assert_eq!(
        mov.ad_updates
            .iter()
            .map(|a| (a.address.get(), a.bit))
            .collect::<Vec<_>>(),
        [
            (0x1008, od_machine::AdBit::Accessed),
            (0xf000, od_machine::AdBit::Accessed),
            (0x10000, od_machine::AdBit::Accessed),
            (0x11000, od_machine::AdBit::Accessed),
        ]
    );
    assert_eq!(
        mov.committed_writes
            .iter()
            .map(|r| (r.start().get(), r.len()))
            .collect::<Vec<_>>(),
        [(0x1008, 8), (0xf000, 8), (0x10000, 8), (0x11000, 8)]
    );
    for (pte, value) in [
        (0x1008, 0xf027),
        (0xf000, 0x10027),
        (0x10000, 0x11027),
        (0x11000, 0x30027),
    ] {
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(pte), AccessWidth::Qword)
                .unwrap(),
            value
        );
    }
    assert!(
        mov.committed_writes
            .iter()
            .all(|r| r.start().get() != 0x30000)
    );
    assert_eq!(
        callbacks,
        [report.iretq.committed_writes, mov.committed_writes].concat()
    );
}

fn syscall_fixture() -> (MachineBus, LongModeState) {
    let mut bus = make_bus_with_ram_len(true, true, true, true, 0x20000);
    put_qword(&mut bus, 0x4010, 0x9023); // kernel code, supervisor
    put_qword(&mut bus, 0x6000, DATA_PHYSICAL | 3); // supervisor RW alias
    put_qword(&mut bus, 0x1008, 0xf007);
    put_qword(&mut bus, 0xf000, 0x10007);
    put_qword(&mut bus, 0x10000, 0x11007);
    put_qword(&mut bus, 0x11000, DATA_PHYSICAL | 5); // user RO alias
    put_bytes(&mut bus, 0x8000, &[0x0f, 0x05, 0x48, 0x8b, 0x03]);
    put_bytes(&mut bus, 0x9000, &[0x4c, 0x89, 0x17, 0x48, 0x0f, 0x07]);
    let mut state = initial_state(0x202);
    state.cs_selector = Some(0x23);
    state.ss_selector = Some(0x1b);
    state.cs_cache = Some(CodeSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: 0xfb,
        flags: 0xa,
    });
    state.ss_cache = Some(DataSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: 0xf3,
        flags: 0xc,
    });
    state.syscall = Some(od_isa::syscall::SyscallState {
        efer: 0xd01,
        star: (0x10u64 << 48) | (8u64 << 32),
        lstar: HANDLER_RIP,
        fmask: 0x200,
        r11_flags_defined: None,
    });
    state.gpr[Register::Rax as usize] = 0x1234;
    state.gpr[Register::R10 as usize] = 0xaabb_ccdd_1234_5678;
    state.gpr[Register::Rdi as usize] = DATA_LINEAR;
    state.gpr[Register::Rbx as usize] = 0x80_0000_0000;
    (bus, state)
}

#[test]
fn syscall_roundtrip_uses_paged_aliases_and_preserves_rsp() {
    use od_isa::syscall::{SyscallInstruction::*, SyscallStepExit};
    let (mut bus, mut state) = syscall_fixture();
    // The transition must not consult GDT or stack mappings.
    put_qword(&mut bus, 0x5008, 0);
    put_qword(&mut bus, 0x5810, 0);
    let mut writes = Vec::new();
    let mut defined = od_isa::syscall::SYSCALL_FLAGS;
    for (index, instruction) in [Syscall, KernelStore, Sysret, UserLoad]
        .into_iter()
        .enumerate()
    {
        let report = od_machine::step_paged_syscall_profile(
            &mut state,
            &mut bus,
            defined,
            instruction,
            |r| writes.push(r),
        )
        .unwrap();
        assert_eq!(
            report.cpu.exit,
            SyscallStepExit::Retired,
            "step {index}: {report:?}"
        );
        for update in &report.ad_updates {
            let current = bus
                .read_physical(update.address, AccessWidth::Qword)
                .unwrap();
            assert_eq!(current & update.after, update.after);
            assert!(
                report
                    .committed_writes
                    .iter()
                    .any(|range| range.start() == update.address)
            );
        }
        assert!(writes.ends_with(&report.committed_writes));
        defined = report.cpu.rflags_defined;
        assert_eq!(state.gpr[Register::Rsp as usize], ORIGINAL_RSP);
        assert_eq!(state.cr3, Some(CR3));
        match index {
            0 => {
                assert_eq!(state.rip, HANDLER_RIP);
                assert_eq!(state.cs_selector, Some(8));
                assert_eq!(state.ss_selector, Some(0x10));
                assert_eq!(state.gpr[1], FAULT_RIP + 2);
                assert_eq!(state.gpr[11], 0x202);
                assert_eq!(state.rflags & 0x200, 0);
                assert_ne!(defined & 0x200, 0);
                assert_eq!(
                    state.cs_cache.unwrap(),
                    CodeSegmentDescriptorCache {
                        base: 0,
                        limit: u32::MAX,
                        access: 0x9b,
                        flags: 0xa
                    }
                );
                assert_eq!(
                    state.ss_cache.unwrap(),
                    DataSegmentDescriptorCache {
                        base: 0,
                        limit: u32::MAX,
                        access: 0x93,
                        flags: 0xc
                    }
                );
            }
            1 => {
                assert_eq!(state.rip, HANDLER_RIP + 3);
            }
            2 => {
                assert_eq!(state.rip, FAULT_RIP + 2);
                assert_eq!(state.cs_selector, Some(0x23));
                assert_eq!(state.ss_selector, Some(0x1b));
                assert_eq!(state.rflags & 0x200, 0x200);
                assert_ne!(defined & 0x200, 0);
                assert_eq!(
                    state.cs_cache.unwrap(),
                    CodeSegmentDescriptorCache {
                        base: 0,
                        limit: u32::MAX,
                        access: 0xfb,
                        flags: 0xa
                    }
                );
                assert_eq!(
                    state.ss_cache.unwrap(),
                    DataSegmentDescriptorCache {
                        base: 0,
                        limit: u32::MAX,
                        access: 0xf3,
                        flags: 0xc
                    }
                );
                assert_eq!(state.gpr[1], FAULT_RIP + 2);
                assert_eq!(state.gpr[11], 0x202);
            }
            _ => {
                assert_eq!(state.rip, FAULT_RIP + 5);
                assert_eq!(state.gpr[0], 0xaabb_ccdd_1234_5678);
            }
        }
    }
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(DATA_PHYSICAL), AccessWidth::Qword)
            .unwrap(),
        0xaabb_ccdd_1234_5678
    );
    assert_eq!(
        writes
            .iter()
            .filter(|r| r.start().get() == DATA_PHYSICAL)
            .count(),
        1
    );
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(0x6000), AccessWidth::Qword)
            .unwrap(),
        DATA_PHYSICAL | 0x63
    );
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(0x11000), AccessWidth::Qword)
            .unwrap(),
        DATA_PHYSICAL | 0x25
    );
}

#[test]
fn syscall_profile_faults_preserve_committed_boundaries() {
    use od_isa::syscall::{SyscallInstruction::*, SyscallStepExit};
    // SCE=0 produces #UD before changing privilege state.
    let (mut bus, mut state) = syscall_fixture();
    state.syscall.as_mut().unwrap().efer = 0xd00;
    let before = state.clone();
    let report = od_machine::step_paged_syscall_profile(
        &mut state,
        &mut bus,
        od_isa::INTEGER_FLAGS,
        Syscall,
        |_| {},
    )
    .unwrap();
    assert!(matches!(report.cpu.exit, SyscallStepExit::Fault(ref f) if f.vector == 6));
    assert_eq!(state, before);

    // Kernel write to a supervisor read-only mapping faults after SYSCALL retired.
    let (mut bus, mut state) = syscall_fixture();
    put_qword(&mut bus, 0x6000, DATA_PHYSICAL | 1);
    let reports = od_machine::run_paged_syscall_roundtrip(
        &mut state,
        &mut bus,
        od_isa::INTEGER_FLAGS,
        |_| {},
    )
    .unwrap();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0].cpu.exit, SyscallStepExit::Retired);
    let fault = reports[1].page_fault.as_ref().unwrap();
    assert_eq!(
        (
            fault.fault().error_code,
            fault.saved_rip(),
            fault.cr2_value()
        ),
        (Some(3), HANDLER_RIP, DATA_LINEAR)
    );
    assert_eq!(state.cs_selector, Some(8));
    assert_eq!(state.rip, HANDLER_RIP);
    assert_eq!(state.cr2, Some(DATA_LINEAR));
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(0x6000), AccessWidth::Qword)
            .unwrap(),
        DATA_PHYSICAL | 0x21
    );
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(DATA_PHYSICAL), AccessWidth::Qword)
            .unwrap(),
        SENTINEL
    );

    // User read of supervisor mapping faults after both transitions and the store.
    let (mut bus, mut state) = syscall_fixture();
    put_qword(&mut bus, 0x11000, DATA_PHYSICAL | 1);
    let mut callbacks = Vec::new();
    let reports = od_machine::run_paged_syscall_roundtrip(
        &mut state,
        &mut bus,
        od_isa::INTEGER_FLAGS,
        |range| callbacks.push(range),
    )
    .unwrap();
    assert_eq!(reports.len(), 4);
    assert!(
        reports[..3]
            .iter()
            .all(|r| r.cpu.exit == SyscallStepExit::Retired)
    );
    let fault = reports[3].page_fault.as_ref().unwrap();
    assert_eq!(
        (
            fault.fault().error_code,
            fault.saved_rip(),
            fault.cr2_value()
        ),
        (Some(5), FAULT_RIP + 2, 0x80_0000_0000)
    );
    assert_eq!(state.cs_selector, Some(0x23));
    assert_eq!(state.rip, FAULT_RIP + 2);
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(0x11000), AccessWidth::Qword)
            .unwrap(),
        DATA_PHYSICAL | 0x21
    );
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(DATA_PHYSICAL), AccessWidth::Qword)
            .unwrap(),
        0xaabb_ccdd_1234_5678
    );

    let ranges = |addresses: &[u64]| {
        addresses
            .iter()
            .map(|&address| GuestPhysicalRange::new(GuestPhysicalAddress::new(address), 8).unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        callbacks,
        ranges(&[
            0x3020,
            0x6000,
            0x6000,
            DATA_PHYSICAL,
            0x1008,
            0xf000,
            0x10000,
            0x11000
        ])
    );
    assert_eq!(
        reports[3].committed_writes,
        ranges(&[0x1008, 0xf000, 0x10000, 0x11000])
    );
    for (address, expected) in [
        (0x3020, 0x6027),
        (0x6000, DATA_PHYSICAL | 0x63),
        (0x1008, 0xf027),
        (0xf000, 0x10027),
        (0x10000, 0x11027),
        (0x11000, DATA_PHYSICAL | 0x21),
    ] {
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                .unwrap(),
            expected
        );
    }

    // SYSRETQ #GP for CPL3 and noncanonical RCX leaves CPU unchanged.
    for wrong_cpl in [true, false] {
        let (mut bus, mut state) = syscall_fixture();
        if wrong_cpl {
            put_bytes(&mut bus, 0x8000, &[0x48, 0x0f, 0x07]);
        } else {
            let r = od_machine::step_paged_syscall_profile(
                &mut state,
                &mut bus,
                od_isa::INTEGER_FLAGS,
                Syscall,
                |_| {},
            )
            .unwrap();
            assert_eq!(r.cpu.exit, SyscallStepExit::Retired);
            put_bytes(&mut bus, 0x9000, &[0x48, 0x0f, 0x07]);
            state.gpr[1] = 0x0000_8000_0000_0000;
        }
        let before = state.clone();
        let report = od_machine::step_paged_syscall_profile(
            &mut state,
            &mut bus,
            od_isa::INTEGER_FLAGS,
            Sysret,
            |_| {},
        )
        .unwrap();
        assert!(
            matches!(report.cpu.exit, SyscallStepExit::Fault(ref f) if f.vector == 13 && f.error_code == Some(0))
        );
        assert_eq!(state, before);
    }
}

#[test]
fn syscall_profile_fetch_and_backend_faults_are_distinct() {
    use od_isa::syscall::{SyscallInstruction::*, SyscallStepExit};
    let (mut bus, mut state) = syscall_fixture();
    // The post-SYSRET instruction is on a separate supervisor page.
    put_bytes(&mut bus, 0x8ffe, &[0x0f, 0x05]);
    put_qword(&mut bus, 0x4010, 0x9003);
    put_qword(&mut bus, 0x4018, 0xa003);
    put_bytes(&mut bus, 0xa000, &[0x4c, 0x89, 0x17, 0x48, 0x0f, 0x07]);
    state.rip = FAULT_RIP + 0xffe;
    state.syscall.as_mut().unwrap().lstar = 0x0040_3000;
    let mut callbacks = Vec::new();
    let reports = od_machine::run_paged_syscall_roundtrip(
        &mut state,
        &mut bus,
        od_isa::INTEGER_FLAGS,
        |range| callbacks.push(range),
    )
    .unwrap();
    assert_eq!(reports.len(), 4);
    assert!(
        reports[..3]
            .iter()
            .all(|r| r.cpu.exit == SyscallStepExit::Retired)
    );
    let fault = reports[3].page_fault.as_ref().unwrap();
    assert_eq!(
        (
            fault.fault().error_code,
            fault.saved_rip(),
            fault.cr2_value()
        ),
        (Some(0x15), HANDLER_RIP, HANDLER_RIP)
    );
    assert_eq!(state.cs_selector, Some(0x23));
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(DATA_PHYSICAL), AccessWidth::Qword)
            .unwrap(),
        0xaabb_ccdd_1234_5678
    );

    let ranges = |addresses: &[u64]| {
        addresses
            .iter()
            .map(|&address| GuestPhysicalRange::new(GuestPhysicalAddress::new(address), 8).unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        callbacks,
        ranges(&[0x4018, 0x3020, 0x6000, 0x6000, DATA_PHYSICAL, 0x4010])
    );
    assert_eq!(reports[3].committed_writes, ranges(&[0x4010]));
    for (address, expected) in [
        (0x4018, 0xa023),
        (0x4010, 0x9023),
        (0x3020, 0x6027),
        (0x6000, DATA_PHYSICAL | 0x63),
    ] {
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                .unwrap(),
            expected
        );
    }

    let (mut bus, mut state) = syscall_fixture();
    put_qword(&mut bus, 0x11000, 0x30000 | 5); // valid PTE, physical operand outside RAM
    let old_cr2 = state.cr2;
    let mut callbacks = Vec::new();
    let reports = od_machine::run_paged_syscall_roundtrip(
        &mut state,
        &mut bus,
        od_isa::INTEGER_FLAGS,
        |range| callbacks.push(range),
    )
    .unwrap();
    assert_eq!(reports.len(), 4);
    assert!(matches!(reports[3].cpu.exit, SyscallStepExit::Backend(_)));
    assert!(reports[3].page_fault.is_none());
    assert!(reports[3].access_error.is_some());
    assert_eq!(state.cr2, old_cr2);
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(0x11000), AccessWidth::Qword)
            .unwrap(),
        0x30025
    );
    assert_eq!(state.cs_selector, Some(0x23));
    assert_eq!(state.rip, FAULT_RIP + 2);

    assert_eq!(
        callbacks,
        ranges(&[
            0x3020,
            0x6000,
            0x6000,
            DATA_PHYSICAL,
            0x1008,
            0xf000,
            0x10000,
            0x11000
        ])
    );
    assert_eq!(
        reports[3].committed_writes,
        ranges(&[0x1008, 0xf000, 0x10000, 0x11000])
    );
    for (address, expected) in [
        (0x3020, 0x6027),
        (0x6000, DATA_PHYSICAL | 0x63),
        (0x1008, 0xf027),
        (0xf000, 0x10027),
        (0x10000, 0x11027),
        (0x11000, 0x30025),
        (DATA_PHYSICAL, 0xaabb_ccdd_1234_5678),
    ] {
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                .unwrap(),
            expected
        );
    }

    // A mismatch on the last byte must not fetch a fourth byte from the next page.
    let (mut bus, mut state) = syscall_fixture();
    state.rip = FAULT_RIP + 0xffd;
    put_bytes(&mut bus, 0x8ffd, &[0x48, 0x8b, 0x43]);
    put_qword(&mut bus, 0x4010, 0);
    let report = od_machine::step_paged_syscall_profile(
        &mut state,
        &mut bus,
        od_isa::INTEGER_FLAGS,
        UserLoad,
        |_| {},
    )
    .unwrap();
    assert_eq!(report.cpu.exit, SyscallStepExit::Unsupported);
    assert!(report.page_fault.is_none());
}

#[test]
fn syscall_user_permissions_all_page_levels() {
    use od_isa::syscall::{SyscallInstruction::*, SyscallStepExit};
    for (instruction, entries, expected_fault, address) in [
        (Syscall, [0x1000, 0x2000, 0x3010, 0x4008], 0x15, FAULT_RIP),
        (
            UserLoad,
            [0x1008, 0xf000, 0x10000, 0x11000],
            0x05,
            0x80_0000_0000,
        ),
    ] {
        for entry in entries {
            let (mut bus, mut state) = syscall_fixture();
            if instruction == UserLoad {
                state.rip = FAULT_RIP + 2;
            }
            let value = bus
                .read_physical(GuestPhysicalAddress::new(entry), AccessWidth::Qword)
                .unwrap();
            put_qword(&mut bus, entry, value & !4);
            let report = od_machine::step_paged_syscall_profile(
                &mut state,
                &mut bus,
                od_isa::INTEGER_FLAGS,
                instruction,
                |_| {},
            )
            .unwrap();
            assert!(matches!(report.cpu.exit, SyscallStepExit::Fault(_)));
            let fault = report.page_fault.unwrap();
            assert_eq!(
                (
                    fault.fault().error_code,
                    fault.cr2_value(),
                    fault.saved_rip()
                ),
                (Some(expected_fault), address, state.rip)
            );
            assert_eq!(state.cr2, Some(address));
        }
    }
}

#[test]
fn syscall_late_faults_preserve_completed_transition_and_store() {
    use od_isa::syscall::{SyscallInstruction::*, SyscallStepExit};
    let (mut bus, mut state) = syscall_fixture();
    put_qword(&mut bus, 0x4010, 0);
    let reports = od_machine::run_paged_syscall_roundtrip(
        &mut state,
        &mut bus,
        od_isa::INTEGER_FLAGS,
        |_| {},
    )
    .unwrap();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0].cpu.exit, SyscallStepExit::Retired);
    let fault = reports[1].page_fault.as_ref().unwrap();
    assert_eq!(
        (fault.cr2_value(), fault.saved_rip()),
        (HANDLER_RIP, HANDLER_RIP)
    );
    assert_eq!(state.cs_selector, Some(8));

    let (mut bus, mut state) = syscall_fixture();
    for instruction in [Syscall, KernelStore] {
        let report = od_machine::step_paged_syscall_profile(
            &mut state,
            &mut bus,
            od_isa::INTEGER_FLAGS,
            instruction,
            |_| {},
        )
        .unwrap();
        assert_eq!(report.cpu.exit, SyscallStepExit::Retired);
    }
    state.gpr[1] = 0x8000_0000_0000;
    let before = state.clone();
    let report = od_machine::step_paged_syscall_profile(
        &mut state,
        &mut bus,
        od_isa::INTEGER_FLAGS,
        Sysret,
        |_| {},
    )
    .unwrap();
    assert!(
        matches!(report.cpu.exit, SyscallStepExit::Fault(f) if f.vector == 13 && f.error_code == Some(0))
    );
    assert_eq!(state, before);
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(DATA_PHYSICAL), AccessWidth::Qword)
            .unwrap(),
        0xaabb_ccdd_1234_5678
    );

    let (mut bus, mut state) = syscall_fixture();
    put_qword(&mut bus, 0x6000, 0x30003);
    let old_cr2 = state.cr2;
    let reports = od_machine::run_paged_syscall_roundtrip(
        &mut state,
        &mut bus,
        od_isa::INTEGER_FLAGS,
        |_| {},
    )
    .unwrap();
    assert_eq!(reports.len(), 2);
    assert!(matches!(reports[1].cpu.exit, SyscallStepExit::Backend(_)));
    assert!(reports[1].page_fault.is_none());
    assert!(reports[1].access_error.is_some());
    assert_eq!(state.cr2, old_cr2);
    assert_eq!(state.cs_selector, Some(8));
}

#[test]
fn syscall_instruction_and_operand_page_boundaries_fail_closed() {
    use od_isa::syscall::{SyscallInstruction::*, SyscallStepExit};
    for (instruction, bytes) in [
        (Syscall, &b"\x0f\x05"[..]),
        (KernelStore, &b"\x4c\x89\x17"[..]),
        (Sysret, &b"\x48\x0f\x07"[..]),
        (UserLoad, &b"\x48\x8b\x03"[..]),
    ] {
        for crossing in [false, true] {
            let (mut bus, mut state) = syscall_fixture();
            if matches!(instruction, KernelStore | Sysret) {
                let r = od_machine::step_paged_syscall_profile(
                    &mut state,
                    &mut bus,
                    od_isa::INTEGER_FLAGS,
                    Syscall,
                    |_| {},
                )
                .unwrap();
                assert_eq!(r.cpu.exit, SyscallStepExit::Retired);
            }
            let offset = 0x1000 - bytes.len() as u64 + u64::from(crossing);
            let (linear, physical) = if matches!(instruction, KernelStore | Sysret) {
                (HANDLER_RIP, 0x9000)
            } else {
                (FAULT_RIP, 0x8000)
            };
            state.rip = linear + offset;
            if instruction == Sysret {
                state.gpr[1] = FAULT_RIP;
            }
            put_bytes(&mut bus, physical + offset, bytes);
            let before = state.clone();
            let report = od_machine::step_paged_syscall_profile(
                &mut state,
                &mut bus,
                od_isa::INTEGER_FLAGS,
                instruction,
                |_| {},
            )
            .unwrap();
            if crossing {
                assert_eq!(report.cpu.exit, SyscallStepExit::Unsupported);
                assert_eq!(state, before);
                assert!(report.page_fault.is_none());
            } else {
                assert_eq!(report.cpu.exit, SyscallStepExit::Retired);
            }
        }
    }
    for crossing in [false, true] {
        let (mut bus, mut state) = syscall_fixture();
        state.rip = FAULT_RIP + 2;
        state.gpr[Register::Rbx as usize] = 0x80_0000_0000 + if crossing { 0xff9 } else { 0xff8 };
        put_qword(&mut bus, DATA_PHYSICAL + 0xff8, 0x12345678);
        let before = state.clone();
        let r = od_machine::step_paged_syscall_profile(
            &mut state,
            &mut bus,
            od_isa::INTEGER_FLAGS,
            UserLoad,
            |_| {},
        )
        .unwrap();
        if crossing {
            assert_eq!(r.cpu.exit, SyscallStepExit::Rejected);
            assert_eq!(state, before);
        } else {
            assert_eq!(r.cpu.exit, SyscallStepExit::Retired);
            assert_eq!(state.gpr[Register::Rax as usize], 0x12345678);
        }
    }
    for crossing in [false, true] {
        let (mut bus, mut state) = syscall_fixture();
        let r = od_machine::step_paged_syscall_profile(
            &mut state,
            &mut bus,
            od_isa::INTEGER_FLAGS,
            Syscall,
            |_| {},
        )
        .unwrap();
        assert_eq!(r.cpu.exit, SyscallStepExit::Retired);
        state.gpr[Register::Rdi as usize] = DATA_LINEAR + if crossing { 0xff9 } else { 0xff8 };
        let before = state.clone();
        let mut writes = vec![];
        let r = od_machine::step_paged_syscall_profile(
            &mut state,
            &mut bus,
            od_isa::INTEGER_FLAGS,
            KernelStore,
            |w| writes.push(w),
        )
        .unwrap();
        if crossing {
            assert_eq!(r.cpu.exit, SyscallStepExit::Rejected);
            assert_eq!(state, before);
            assert!(writes.iter().all(|w| w.start().get() < DATA_PHYSICAL));
        } else {
            assert_eq!(r.cpu.exit, SyscallStepExit::Retired);
            assert_eq!(
                bus.read_physical(
                    GuestPhysicalAddress::new(DATA_PHYSICAL + 0xff8),
                    AccessWidth::Qword
                )
                .unwrap(),
                0xaabb_ccdd_1234_5678
            );
        }
    }
}

#[test]
fn syscall_paging_effects_exact_on_success_and_store_failures() {
    use od_isa::syscall::SyscallStepExit;
    use od_machine::{AdBit::*, PageTableLevel::*};

    for scenario in ["success", "page_fault", "backend"] {
        let (mut bus, mut state) = syscall_fixture();
        put_qword(&mut bus, 0x4008, 0x8007); // user instruction fetch sets A
        put_qword(&mut bus, 0x4010, 0x9003); // kernel instruction fetch sets A
        let leaf = match scenario {
            "page_fault" => DATA_PHYSICAL | 1,
            "backend" => 0x30003,
            _ => DATA_PHYSICAL | 3,
        };
        put_qword(&mut bus, 0x6000, leaf);
        let old_cr2 = state.cr2;
        let mut callbacks = Vec::new();
        let reports = od_machine::run_paged_syscall_roundtrip(
            &mut state,
            &mut bus,
            od_isa::INTEGER_FLAGS,
            |range| callbacks.push(range),
        )
        .unwrap();
        assert_eq!(reports.len(), if scenario == "success" { 4 } else { 2 });
        assert_eq!(reports[0].cpu.exit, SyscallStepExit::Retired);
        assert_eq!(
            reports[0]
                .ad_updates
                .iter()
                .map(|u| (u.level, u.address.get(), u.before, u.after, u.bit))
                .collect::<Vec<_>>(),
            [(Pt, 0x4008, 0x8007, 0x8027, Accessed)]
        );

        let mut store_updates = vec![
            (Pt, 0x4010, 0x9003, 0x9023, Accessed),
            (Pd, 0x3020, 0x6007, 0x6027, Accessed),
            (Pt, 0x6000, leaf, leaf | 0x20, Accessed),
        ];
        let mut store_writes = vec![0x4010, 0x3020, 0x6000];
        if scenario == "success" {
            store_updates.push((Pt, 0x6000, leaf | 0x20, leaf | 0x60, Dirty));
            store_writes.extend([0x6000, DATA_PHYSICAL]);
            assert_eq!(reports[1].cpu.exit, SyscallStepExit::Retired);
        } else if scenario == "page_fault" {
            let fault = reports[1].page_fault.as_ref().unwrap();
            assert_eq!(
                (
                    fault.fault().error_code,
                    fault.cr2_value(),
                    fault.saved_rip()
                ),
                (Some(3), DATA_LINEAR, HANDLER_RIP)
            );
            assert!(matches!(reports[1].cpu.exit, SyscallStepExit::Fault(_)));
            assert_eq!(state.cr2, Some(DATA_LINEAR));
        } else {
            assert!(matches!(reports[1].cpu.exit, SyscallStepExit::Backend(_)));
            assert!(reports[1].page_fault.is_none());
            assert!(matches!(
                reports[1].access_error,
                Some(od_machine::TrackedAccessError::OperandPhysical { .. })
            ));
            assert_eq!(state.cr2, old_cr2);
        }
        let actual_updates = |report: &od_machine::PagedSyscallStepReport| {
            report
                .ad_updates
                .iter()
                .map(|u| (u.level, u.address.get(), u.before, u.after, u.bit))
                .collect::<Vec<_>>()
        };
        let ranges = |addresses: &[u64]| {
            addresses
                .iter()
                .map(|&address| {
                    GuestPhysicalRange::new(GuestPhysicalAddress::new(address), 8).unwrap()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(reports[0].committed_writes, ranges(&[0x4008]));
        assert_eq!(actual_updates(&reports[1]), store_updates, "{scenario}");
        assert_eq!(
            reports[1].committed_writes,
            ranges(&store_writes),
            "{scenario}"
        );
        let mut expected_callbacks = vec![0x4008];
        expected_callbacks.extend(store_writes);
        if scenario == "success" {
            assert_eq!(reports[2].cpu.exit, SyscallStepExit::Retired);
            assert!(reports[2].ad_updates.is_empty());
            assert!(reports[2].committed_writes.is_empty());
            assert_eq!(reports[3].cpu.exit, SyscallStepExit::Retired);
            assert_eq!(
                actual_updates(&reports[3]),
                [
                    (Pml4, 0x1008, 0xf007, 0xf027, Accessed),
                    (Pdpt, 0xf000, 0x10007, 0x10027, Accessed),
                    (Pd, 0x10000, 0x11007, 0x11027, Accessed),
                    (
                        Pt,
                        0x11000,
                        DATA_PHYSICAL | 5,
                        DATA_PHYSICAL | 0x25,
                        Accessed
                    ),
                ]
            );
            let user_writes = [0x1008, 0xf000, 0x10000, 0x11000];
            assert_eq!(reports[3].committed_writes, ranges(&user_writes));
            expected_callbacks.extend(user_writes);
            assert_eq!(state.gpr[Register::Rax as usize], 0xaabb_ccdd_1234_5678);
        } else {
            // SYSCALL retired before the failed store; no operand write occurred.
            assert_eq!(state.cs_selector, Some(8));
            assert_eq!(state.rip, HANDLER_RIP);
            assert_eq!(state.gpr[Register::Rcx as usize], FAULT_RIP + 2);
        }
        assert_eq!(callbacks, ranges(&expected_callbacks), "{scenario}");
        for (address, expected) in [
            (0x4008, 0x8027),
            (0x4010, 0x9023),
            (0x3020, 0x6027),
            (
                0x6000,
                leaf | if scenario == "success" { 0x60 } else { 0x20 },
            ),
            (
                0x1008,
                if scenario == "success" {
                    0xf027
                } else {
                    0xf007
                },
            ),
            (
                0xf000,
                if scenario == "success" {
                    0x10027
                } else {
                    0x10007
                },
            ),
            (
                0x10000,
                if scenario == "success" {
                    0x11027
                } else {
                    0x11007
                },
            ),
            (
                0x11000,
                DATA_PHYSICAL | if scenario == "success" { 0x25 } else { 5 },
            ),
            (
                DATA_PHYSICAL,
                if scenario == "success" {
                    0xaabb_ccdd_1234_5678
                } else {
                    SENTINEL
                },
            ),
        ] {
            assert_eq!(
                bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                    .unwrap(),
                expected,
                "{scenario}: {address:#x}"
            );
        }
    }
}

#[test]
fn cpl3_mov_cr0_first_fetch_enforces_user_page_permissions() {
    use od_isa::{execute::PagedProbeExit, iretq::IretqResult};
    use od_machine::{AdBit, step_paged_cpl3_mov_cr0, step_paged_iretq_to_cpl3};

    const USER_CODE: u64 = 0x80_0040_2000;
    let code_pte = [
        (0x1008, 0xf007),
        (0xf000, 0x10007),
        (0x10010, 0x12007),
        (0x12010, 0x9007),
    ];
    for (fault_level, nx) in [
        (0, false),
        (1, false),
        (2, false),
        (3, false),
        (3, true),
        (4, false),
    ] {
        let mut bus = make_bus_with_ram_len(true, true, true, true, 0x20000);
        put_bytes(&mut bus, 0x8000, &[0x48, 0xcf]);
        put_bytes(&mut bus, 0x9000, &[0x0f, 0x22, 0xc0]);
        put_bytes(&mut bus, 0xb018, &[0xff, 0xff, 0, 0, 0, 0xfb, 0xaf, 0]);
        put_bytes(&mut bus, 0xb020, &[0xff, 0xff, 0, 0, 0, 0xf3, 0xcf, 0]);
        for (address, value) in code_pte {
            put_qword(&mut bus, address, value);
        }
        // The user data mapping is separate from the user code leaf.
        for (address, value) in [(0x10000, 0x11007), (0x11000, DATA_PHYSICAL | 7)] {
            put_qword(&mut bus, address, value);
        }
        for (offset, value) in [
            (0, USER_CODE),
            (8, 0x1b),
            (16, 0x202),
            (24, 0x0070_2000),
            (32, 0x23),
        ] {
            put_qword(&mut bus, 0xd027 + offset, value);
        }
        let mut expected_code_ptes = code_pte.map(|(a, v)| (a, v));
        if nx {
            expected_code_ptes[3].1 |= 1 << 63;
            put_qword(&mut bus, expected_code_ptes[3].0, expected_code_ptes[3].1);
        } else if fault_level == 4 {
            expected_code_ptes[3].1 &= !1;
            put_qword(&mut bus, expected_code_ptes[3].0, expected_code_ptes[3].1);
        } else {
            expected_code_ptes[fault_level].1 &= !4;
            put_qword(
                &mut bus,
                expected_code_ptes[fault_level].0,
                expected_code_ptes[fault_level].1,
            );
        }
        let mut state = initial_state(0x202);
        state.cs_cache.as_mut().unwrap().flags = 0xa;
        state.gpr[Register::Rbx as usize] = 0x80_0000_0000;
        let mut callbacks = Vec::new();
        let iret = step_paged_iretq_to_cpl3(&mut state, &mut bus, od_isa::INTEGER_FLAGS, |r| {
            callbacks.push(r)
        })
        .unwrap();
        assert_eq!(iret.cpu, IretqResult::Retired);
        // None of these user-code paging entries was touched by the ring-0 return.
        for (address, _) in expected_code_ptes {
            assert_eq!(
                bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                    .unwrap()
                    & 0x20,
                0
            );
        }
        callbacks.clear();
        let before = state.clone();
        let mov = step_paged_cpl3_mov_cr0(&mut state, &mut bus, od_isa::INTEGER_FLAGS, |r| {
            callbacks.push(r)
        })
        .unwrap();
        let fault = mov.page_fault.as_ref().unwrap();
        assert_eq!(mov.cpu.exit, PagedProbeExit::Fault(fault.fault().clone()));
        assert_eq!(mov.cpu.instructions_executed, 0);
        assert_eq!(
            fault.fault().error_code,
            Some(if fault_level == 4 { 0x14 } else { 0x15 })
        );
        assert_eq!(fault.fault().linear_address, Some(USER_CODE));
        assert_eq!(fault.cr2_value(), USER_CODE);
        assert_eq!(fault.saved_rip(), USER_CODE);
        assert_eq!(state.cr2, Some(USER_CODE));
        let mut expected_state = before.clone();
        expected_state.cr2 = Some(USER_CODE);
        assert_eq!(state, expected_state);
        assert_eq!(state.rip, before.rip);
        assert_eq!(state.cs_selector, before.cs_selector);
        assert_eq!(state.ss_selector, before.ss_selector);
        assert_eq!(state.gpr, before.gpr);
        let expected_updates = [
            (0x1008, expected_code_ptes[0].1 | 0x20),
            (0xf000, expected_code_ptes[1].1 | 0x20),
            (0x10010, expected_code_ptes[2].1 | 0x20),
            (
                0x12010,
                if fault_level == 4 {
                    expected_code_ptes[3].1
                } else {
                    expected_code_ptes[3].1 | 0x20
                },
            ),
        ];
        let expected_updates = if fault_level == 4 {
            &expected_updates[..3]
        } else {
            &expected_updates[..]
        };
        assert_eq!(
            mov.ad_updates
                .iter()
                .map(|u| (u.address.get(), u.after, u.bit))
                .collect::<Vec<_>>(),
            expected_updates
                .iter()
                .copied()
                .map(|(address, after)| (address, after, AdBit::Accessed))
                .collect::<Vec<_>>()
        );
        let expected_callbacks = [0x1008, 0xf000, 0x10010, 0x12010]
            .into_iter()
            .take(expected_updates.len())
            .map(|address| GuestPhysicalRange::new(GuestPhysicalAddress::new(address), 8).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(callbacks, expected_callbacks);
        for (address, expected) in expected_updates.iter().copied() {
            assert_eq!(
                bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                    .unwrap(),
                expected
            );
        }
        assert!(mov.ad_updates.iter().all(|u| u.bit == AdBit::Accessed));
        assert!(mov.access_error.is_none());
    }
}

#[test]
fn cpl3_mov_cr0_faults_without_committing_cpu_state() {
    use od_isa::{execute::PagedProbeExit, iretq::IretqResult};
    use od_machine::{step_paged_cpl3_mov_cr0, step_paged_iretq_to_cpl3};
    const USER_CODE: u64 = 0x80_0040_2000;
    for (locked, rax, iopl) in [
        (false, 0x80000033, 0),
        (false, 0x80010033, 3),
        (false, 0, 0),
        (true, 0, 0),
    ] {
        let mut bus = make_bus_with_ram_len(true, true, true, true, 0x20000);
        put_bytes(&mut bus, 0x8000, &[0x48, 0xcf]);
        put_bytes(
            &mut bus,
            0x9000,
            if locked {
                &[0xf0, 0x0f, 0x22, 0xc0]
            } else {
                &[0x0f, 0x22, 0xc0]
            },
        );
        put_bytes(&mut bus, 0xb018, &[0xff, 0xff, 0, 0, 0, 0xfb, 0xaf, 0]);
        put_bytes(&mut bus, 0xb020, &[0xff, 0xff, 0, 0, 0, 0xf3, 0xcf, 0]);
        for (address, value) in [
            (0x1008, 0xf007),
            (0xf000, 0x10007),
            (0x10010, 0x12007),
            (0x12010, 0x9007),
        ] {
            put_qword(&mut bus, address, value);
        }
        for (offset, value) in [
            (0, USER_CODE),
            (8, 0x1b),
            (16, 0x202),
            (24, 0x0070_2000),
            (32, 0x23),
        ] {
            put_qword(&mut bus, 0xd027 + offset, value);
        }
        let mut state = initial_state(0x202);
        state.cs_cache.as_mut().unwrap().flags = 0xa;
        state.gpr[Register::Rax as usize] = rax;
        state.cr2 = Some(0x1234);
        assert_eq!(
            step_paged_iretq_to_cpl3(&mut state, &mut bus, od_isa::INTEGER_FLAGS, |_| {})
                .unwrap()
                .cpu,
            IretqResult::Retired
        );
        let profile_state = state.clone();
        for invalid in [
            {
                let mut s = profile_state.clone();
                s.cs_selector = Some(8);
                s
            },
            {
                let mut s = profile_state.clone();
                s.cr0 = Some(0x8000_0033);
                s
            },
            {
                let mut s = profile_state.clone();
                s.cr4 = Some(0);
                s
            },
            {
                let mut s = profile_state.clone();
                s.rflags |= 1 << 8;
                s
            },
            {
                let mut s = profile_state.clone();
                s.rflags |= 1 << 16;
                s
            },
            {
                let mut s = profile_state.clone();
                s.rflags |= 1 << 18;
                s
            },
            {
                let mut s = profile_state.clone();
                s.sti_shadow = Some(od_isa::StiShadow::Blocking);
                s
            },
        ] {
            let mut invalid = invalid;
            let snapshot = invalid.clone();
            let mut callbacks = Vec::new();
            let rejected =
                step_paged_cpl3_mov_cr0(&mut invalid, &mut bus, od_isa::INTEGER_FLAGS, |r| {
                    callbacks.push(r)
                })
                .unwrap();
            assert!(matches!(rejected.cpu.exit, PagedProbeExit::Rejected(_)));
            assert_eq!(invalid, snapshot);
            assert!(callbacks.is_empty());
        }
        put_bytes(&mut bus, 0x9000, &[0x90]);
        let mut other_form = profile_state.clone();
        let mut callbacks = Vec::new();
        let other =
            step_paged_cpl3_mov_cr0(&mut other_form, &mut bus, od_isa::INTEGER_FLAGS, |r| {
                callbacks.push(r)
            })
            .unwrap();
        assert!(matches!(other.cpu.exit, PagedProbeExit::Unsupported { .. }));
        assert_eq!(other.cpu.instructions_executed, 0);
        assert_eq!(other_form.rip, profile_state.rip);
        assert!(
            callbacks
                .iter()
                .all(|r| ![DATA_PHYSICAL].contains(&r.start().get()))
        );
        assert!(
            callbacks
                .iter()
                .all(|r| ![0x30000, DATA_PHYSICAL].contains(&r.start().get()))
        );
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(DATA_PHYSICAL), AccessWidth::Qword)
                .unwrap(),
            SENTINEL
        );
        put_bytes(
            &mut bus,
            0x9000,
            if locked {
                &[0xf0, 0x0f, 0x22, 0xc0]
            } else {
                &[0x0f, 0x22, 0xc0]
            },
        );
        for (address, value) in [
            (0x1008, 0xf007),
            (0xf000, 0x10007),
            (0x10010, 0x12007),
            (0x12010, 0x9007),
        ] {
            put_qword(&mut bus, address, value);
        }
        state.rflags |= iopl << 12;
        let before = state.clone();
        let mut callbacks = Vec::new();
        let report = step_paged_cpl3_mov_cr0(&mut state, &mut bus, od_isa::INTEGER_FLAGS, |r| {
            callbacks.push(r)
        })
        .unwrap();
        match report.cpu.exit {
            PagedProbeExit::Fault(fault) => {
                assert_eq!(fault.vector, if locked { 6 } else { 13 });
                assert_eq!(fault.error_code, if locked { None } else { Some(0) });
                assert_eq!(fault.linear_address, None);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(state, before);
        assert_eq!(report.cpu.instructions_executed, 0);
        assert!(report.page_fault.is_none() && report.access_error.is_none());
        assert_eq!(
            callbacks
                .iter()
                .map(|r| (r.start().get(), r.len()))
                .collect::<Vec<_>>(),
            [(0x1008, 8), (0xf000, 8), (0x10010, 8), (0x12010, 8)]
        );
        for (address, expected) in [
            (0x1008, 0xf027),
            (0xf000, 0x10027),
            (0x10010, 0x12027),
            (0x12010, 0x9027),
        ] {
            assert_eq!(
                bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                    .unwrap(),
                expected
            );
        }
    }
}

#[test]
fn cpl3_mov_cr0_physical_fetch_failure_keeps_walk_effects_without_retirement() {
    use od_isa::{execute::PagedProbeExit, iretq::IretqResult};
    use od_machine::{
        AdBit, TrackedAccessError, step_paged_cpl3_mov_cr0, step_paged_iretq_to_cpl3,
    };
    const USER_CODE: u64 = 0x80_0040_2000;
    let mut bus = make_bus_with_ram_len(true, true, true, true, 0x20000);
    put_bytes(&mut bus, 0x8000, &[0x48, 0xcf]);
    put_bytes(&mut bus, 0xb018, &[0xff, 0xff, 0, 0, 0, 0xfb, 0xaf, 0]);
    put_bytes(&mut bus, 0xb020, &[0xff, 0xff, 0, 0, 0, 0xf3, 0xcf, 0]);
    for (address, value) in [
        (0x1008, 0xf007),
        (0xf000, 0x10007),
        (0x10010, 0x12007),
        (0x12010, 0x30007),
    ] {
        put_qword(&mut bus, address, value);
    }
    for (offset, value) in [
        (0, USER_CODE),
        (8, 0x1b),
        (16, 0x202),
        (24, 0x0070_2000),
        (32, 0x23),
    ] {
        put_qword(&mut bus, 0xd027 + offset, value);
    }
    let mut state = initial_state(0x202);
    state.cs_cache.as_mut().unwrap().flags = 0xa;
    assert_eq!(
        step_paged_iretq_to_cpl3(&mut state, &mut bus, od_isa::INTEGER_FLAGS, |_| {})
            .unwrap()
            .cpu,
        IretqResult::Retired
    );
    let before = state.clone();
    let mut callbacks = Vec::new();
    let report = step_paged_cpl3_mov_cr0(&mut state, &mut bus, od_isa::INTEGER_FLAGS, |r| {
        callbacks.push(r)
    })
    .unwrap();
    assert!(matches!(report.cpu.exit, PagedProbeExit::Backend(_)));
    assert_eq!(report.cpu.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(report.page_fault.is_none());
    assert!(
        matches!(report.access_error, Some(TrackedAccessError::OperandPhysical { address, .. }) if address.get() == 0x30000)
    );
    assert_eq!(
        report
            .ad_updates
            .iter()
            .map(|u| (u.address.get(), u.after, u.bit))
            .collect::<Vec<_>>(),
        [
            (0x1008, 0xf027, AdBit::Accessed),
            (0xf000, 0x10027, AdBit::Accessed),
            (0x10010, 0x12027, AdBit::Accessed),
            (0x12010, 0x30027, AdBit::Accessed)
        ]
    );
    assert_eq!(
        callbacks
            .iter()
            .map(|r| (r.start().get(), r.len()))
            .collect::<Vec<_>>(),
        [(0x1008, 8), (0xf000, 8), (0x10010, 8), (0x12010, 8)]
    );
}

#[test]
fn cpl3_mov_cr0_instruction_offset_boundary_is_exact() {
    use od_isa::{execute::PagedProbeExit, iretq::IretqResult};
    use od_machine::{step_paged_cpl3_mov_cr0, step_paged_iretq_to_cpl3};
    const USER_CODE: u64 = 0x80_0040_2000;
    for (offset, expect_gp) in [(0xff1, true), (0xff2, false)] {
        let mut bus = make_bus_with_ram_len(true, true, true, true, 0x20000);
        put_bytes(&mut bus, 0x8000, &[0x48, 0xcf]);
        put_bytes(&mut bus, 0x9ff1, &[0x0f, 0x22, 0xc0]);
        put_bytes(&mut bus, 0xb018, &[0xff, 0xff, 0, 0, 0, 0xfb, 0xaf, 0]);
        put_bytes(&mut bus, 0xb020, &[0xff, 0xff, 0, 0, 0, 0xf3, 0xcf, 0]);
        for (address, value) in [
            (0x1008, 0xf007),
            (0xf000, 0x10007),
            (0x10010, 0x12007),
            (0x12010, 0x9007),
        ] {
            put_qword(&mut bus, address, value);
        }
        for (offset_in_frame, value) in [
            (0, USER_CODE + offset),
            (8, 0x1b),
            (16, 0x202),
            (24, 0x0070_2000),
            (32, 0x23),
        ] {
            put_qword(&mut bus, 0xd027 + offset_in_frame, value);
        }
        let mut state = initial_state(0x202);
        state.cs_cache.as_mut().unwrap().flags = 0xa;
        assert_eq!(
            step_paged_iretq_to_cpl3(&mut state, &mut bus, od_isa::INTEGER_FLAGS, |_| {})
                .unwrap()
                .cpu,
            IretqResult::Retired
        );
        let mut callbacks = Vec::new();
        let report = step_paged_cpl3_mov_cr0(&mut state, &mut bus, od_isa::INTEGER_FLAGS, |r| {
            callbacks.push(r)
        })
        .unwrap();
        if expect_gp {
            assert!(
                matches!(report.cpu.exit, PagedProbeExit::Fault(ref f) if f.vector == 13 && f.error_code == Some(0))
            );
            assert_eq!(report.cpu.instructions_executed, 0);
            assert_eq!(callbacks.len(), 4);
        } else {
            assert!(matches!(
                report.cpu.exit,
                PagedProbeExit::Unsupported { .. }
            ));
            assert_eq!(report.cpu.instructions_executed, 0);
            assert!(callbacks.is_empty());
            assert!(report.ad_updates.is_empty());
        }
    }
}
