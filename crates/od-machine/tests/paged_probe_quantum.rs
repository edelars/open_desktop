use od_isa::memory::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, InstructionMode, PrivilegeLevel,
};
use od_isa::{CpuActivity, INTEGER_FLAGS, LongModeState, Register};
use od_machine::{
    AccessWidth, GuestPhysicalAddress, GuestPhysicalRange, MachineBus, MachineMemoryMap,
    MemoryRegionKind, MemoryRegionSpec, PagedProbeExit, PagedProbeRejection, PortAddress,
    PortRange, PortRegionBinding, RamLimits, Uart16550Handle, Uart16550PollingTx,
    run_paged_cr0_wp_probe, run_paged_cr3_probe, run_paged_cr3_probe_2m_ram, run_paged_probe,
};

const CR0: u64 = 0x8001_0033;
const CR0_WP_CLEAR: u64 = CR0 & !(1 << 16);
const CR4: u64 = 1 << 5;
const EFER: u64 = (1 << 8) | (1 << 10) | (1 << 11);
const ROOT: u64 = 0x1000;
const CODE_LINEAR: u64 = 0x0040_0000;
const CODE_PHYSICAL: u64 = 0x7000;
const DATA_LINEAR: u64 = 0x0080_0000;
const DATA_PHYSICAL: u64 = 0x9000;
const COM1: u16 = 0x3f8;
const RAM_SIZE: u64 = 0x20_000;
const FLAGS_DEFINED: u64 = INTEGER_FLAGS & !(1 << 4);

fn fixture() -> (MachineBus, Uart16550Handle) {
    let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM_SIZE).unwrap();
    let map =
        MachineMemoryMap::new(36, vec![MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)]).unwrap();
    let (uart, handle) = Uart16550PollingTx::new_with_handle();
    let mut bus = MachineBus::new(
        map,
        RamLimits::new(RAM_SIZE),
        vec![Box::new(uart)],
        vec![],
        vec![PortRegionBinding {
            range: PortRange::new(PortAddress::new(COM1), 8).unwrap(),
            device_index: 0,
        }],
    )
    .unwrap();
    put(&mut bus, ROOT, ROOT + 0x1007);
    put(&mut bus, ROOT + 0x1000, ROOT + 0x2007);
    put(&mut bus, ROOT + 0x2010, ROOT + 0x3007);
    put(&mut bus, ROOT + 0x3000, CODE_PHYSICAL | 7);
    put(&mut bus, ROOT + 0x2020, ROOT + 0x4007);
    put(&mut bus, ROOT + 0x4000, DATA_PHYSICAL | 7);
    let code = [0x48, 0x89, 0x0b, 0x48, 0x8b, 0x03, 0xee, 0xf4];
    for (offset, byte) in code.into_iter().enumerate() {
        put_byte(&mut bus, CODE_PHYSICAL + offset as u64, byte);
    }
    for (port, value) in [
        (COM1 + 3, 0x83),
        (COM1, 0x34),
        (COM1 + 1, 0x12),
        (COM1 + 3, 0x03),
    ] {
        bus.write_port(PortAddress::new(port), AccessWidth::Byte, value)
            .unwrap();
    }
    (bus, handle)
}

fn cr3_switch_fixture() -> MachineBus {
    let (mut bus, _uart) = fixture();
    const ROOT_B: u64 = 0x11_000;
    const PDPT_B: u64 = 0x12_000;
    const PD_B: u64 = 0x13_000;
    const CODE_PT_B: u64 = 0x14_000;
    const DATA_PT_B: u64 = 0x15_000;
    const CODE_B_PHYSICAL: u64 = 0x16_000;
    const DATA_A_PHYSICAL: u64 = 0x9000;
    const DATA_B_PHYSICAL: u64 = 0x17_000;

    // Build an independent four-level tree for root B. It maps the same code
    // and data virtual addresses to different physical pages from root A.
    put(&mut bus, ROOT_B, PDPT_B | 7);
    put(&mut bus, PDPT_B, PD_B | 7);
    put(&mut bus, PD_B + 0x10, CODE_PT_B | 7);
    put(&mut bus, PD_B + 0x20, DATA_PT_B | 7);
    put(&mut bus, CODE_PT_B, CODE_B_PHYSICAL | 7);
    put(&mut bus, CODE_PT_B + 0x20, DATA_B_PHYSICAL | 7);
    put(&mut bus, DATA_PT_B, DATA_B_PHYSICAL | 7);

    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x0f, 0x22, 0xd8, 0x90]);
    put_bytes_at(&mut bus, CODE_B_PHYSICAL + 3, &[0x48, 0x8b, 0x03]);
    put(&mut bus, DATA_A_PHYSICAL, 0x1111_2222_3333_4444);
    put(&mut bus, DATA_B_PHYSICAL, 0xaabb_ccdd_eeff_1234);
    bus
}

fn cr0_wp_fixture() -> MachineBus {
    let (mut bus, _uart) = fixture();
    // Keep the data PTE read-only for both following stores. The exact MOV
    // CR0 remains in executable RAM and the operand is wholly within one page.
    put(&mut bus, 0x5000, DATA_PHYSICAL | 5);
    put_bytes_at(
        &mut bus,
        CODE_PHYSICAL,
        &[0x0f, 0x22, 0xc0, 0x48, 0x89, 0x0b],
    );
    bus
}

fn initial_cr0_wp_state(cr0_source: u64, payload: u64) -> LongModeState {
    let mut state = initial_state(payload);
    state.cr3 = Some(ROOT);
    state.gpr[Register::Rax as usize] = cr0_source;
    state
}

fn initial_cr3_state() -> LongModeState {
    let mut state = initial_state(0);
    state.cr3 = Some(ROOT);
    state.gpr[Register::Rax as usize] = 0x11_000;
    state.gpr[Register::Rbx as usize] = DATA_LINEAR;
    state
}

fn put(bus: &mut MachineBus, address: u64, value: u64) {
    bus.write_physical(
        GuestPhysicalAddress::new(address),
        AccessWidth::Qword,
        value,
        |_| {},
    )
    .unwrap();
}

fn put_byte(bus: &mut MachineBus, address: u64, value: u8) {
    bus.write_physical(
        GuestPhysicalAddress::new(address),
        AccessWidth::Byte,
        u64::from(value),
        |_| {},
    )
    .unwrap();
}

fn put_bytes_at(bus: &mut MachineBus, address: u64, bytes: &[u8]) {
    for (offset, byte) in bytes.iter().copied().enumerate() {
        put_byte(bus, address + offset as u64, byte);
    }
}

fn qword(bus: &mut MachineBus, address: u64) -> u64 {
    bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
        .unwrap()
}

fn range(address: u64) -> GuestPhysicalRange {
    GuestPhysicalRange::new(GuestPhysicalAddress::new(address), 8).unwrap()
}

fn map_next_code_page(bus: &mut MachineBus, pte: u64) {
    // The first code PTE is at 0x4000; the next 4 KiB page is its index 1.
    put(bus, 0x4008, pte);
}

fn explicit_context() -> AccessContext {
    AccessContext {
        origin: AccessOrigin::Explicit,
        ..context()
    }
}

fn context() -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::DescriptorTable,
    }
}

fn initial_state(payload: u64) -> LongModeState {
    let mut state = LongModeState {
        cr3: None,
        rip: CODE_LINEAR,
        cr0: Some(CR0),
        cr4: Some(CR4),
        ..LongModeState::default()
    };
    state.activity = Some(CpuActivity::Running);
    state.sti_shadow = Some(od_isa::StiShadow::Clear);
    state.gpr[Register::Rbx as usize] = DATA_LINEAR;
    state.gpr[Register::Rcx as usize] = payload;
    state.gpr[Register::Rdx as usize] = u64::from(COM1);
    state.gpr[Register::Rax as usize] = 0xfeed_face_cafe_be00;
    state.rflags = 0x2d7;
    state
}

fn run(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    budget: u64,
    stop: impl FnMut() -> bool,
    writes: &mut Vec<GuestPhysicalRange>,
) -> od_machine::PagedProbeQuantumReport {
    run_paged_probe(
        state,
        bus,
        ROOT,
        EFER,
        context(),
        budget,
        FLAGS_DEFINED,
        stop,
        |range| writes.push(range),
    )
}

fn assert_payload(bus: &mut MachineBus, handle: &Uart16550Handle, payload: u64) {
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(DATA_PHYSICAL), AccessWidth::Qword)
            .unwrap(),
        payload
    );
    assert!(handle.transfer_holding_to_shift());
    assert_eq!(handle.complete_transmission(), Some(payload as u8));
}

#[test]
fn mixed_nonidentity_stream_runs_as_one_or_one_step_quanta() {
    for payload in [0xfeed_face_cafe_be41, 0x0123_4567_89ab_00a7] {
        let (mut bus, handle) = fixture();
        let mut state = initial_state(payload);
        let mut writes = Vec::new();
        let result = run(&mut state, &mut bus, 4, || false, &mut writes);
        assert_eq!(result.exit, PagedProbeExit::Halted);
        assert_eq!(result.instructions_executed, 4);
        assert_eq!(result.rflags_defined, FLAGS_DEFINED);
        assert_eq!(state.rip, CODE_LINEAR + 8);
        assert_eq!(state.activity, Some(CpuActivity::Halted));
        assert_eq!(state.rflags, 0x2d7);
        assert_payload(&mut bus, &handle, payload);

        let (mut split_bus, split_handle) = fixture();
        let mut split_state = initial_state(payload);
        let mut split_writes = Vec::new();
        for index in 0..4 {
            let result = run(
                &mut split_state,
                &mut split_bus,
                1,
                || false,
                &mut split_writes,
            );
            if index == 3 {
                assert_eq!(result.exit, PagedProbeExit::Halted);
            } else {
                assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
            }
            assert_eq!(result.instructions_executed, 1);
            assert_eq!(result.rflags_defined, FLAGS_DEFINED);
        }
        assert_eq!(split_state, state);
        assert_eq!(split_writes, writes);
        assert_payload(&mut split_bus, &split_handle, payload);
    }
}

#[test]
fn cpu_owned_cr3_is_rejected_before_stop_callback() {
    let (mut bus, _) = fixture();
    let mut state = initial_state(0);
    state.cr3 = Some(ROOT);
    let mut writes = Vec::new();
    let result = run(
        &mut state,
        &mut bus,
        4,
        || panic!("must reject before STOP"),
        &mut writes,
    );
    assert_eq!(
        result.exit,
        PagedProbeExit::Rejected(PagedProbeRejection::CpuOwnedCr3State { value: ROOT })
    );
    assert_eq!(result.instructions_executed, 0);
    assert!(writes.is_empty());
}

#[test]
fn mov_cr3_replaces_translation_root_before_next_fetch_and_operand() {
    let mut bus = cr3_switch_fixture();
    assert_eq!(qword(&mut bus, 0x13_020), 0x15_007);
    assert_eq!(qword(&mut bus, 0x15_000), 0x17_007);
    assert_eq!(qword(&mut bus, 0x17_000), 0xaabb_ccdd_eeff_1234);
    let mut state = initial_cr3_state();
    let result = run_paged_cr3_probe(
        &mut state,
        &mut bus,
        EFER,
        context(),
        2,
        FLAGS_DEFINED,
        || false,
        |_| {},
    );

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.cr3, Some(0x11_000));
    assert_eq!(state.rip, CODE_LINEAR + 6);
    assert_eq!(state.gpr[Register::Rax as usize], 0xaabb_ccdd_eeff_1234);
    assert_eq!(
        result.translation_context_changes,
        [od_isa::execute::TranslationContextChange::Cr3Write {
            old_cr3: ROOT,
            new_cr3: 0x11_000,
        }]
    );
}

#[test]
fn mov_cr3_snapshot_is_rebuilt_before_stop_and_budget_handoff() {
    let mut bus = cr3_switch_fixture();
    let mut state = initial_cr3_state();
    let first = run_paged_cr3_probe(
        &mut state,
        &mut bus,
        EFER,
        context(),
        1,
        FLAGS_DEFINED,
        || false,
        |_| {},
    );
    assert_eq!(first.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(first.instructions_executed, 1);
    assert_eq!(state.cr3, Some(0x11_000));
    assert_eq!(state.rip, CODE_LINEAR + 3);
    assert_eq!(first.translation_context_changes.len(), 1);

    let second = run_paged_cr3_probe(
        &mut state,
        &mut bus,
        EFER,
        context(),
        1,
        first.rflags_defined,
        || false,
        |_| {},
    );
    assert_eq!(second.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(second.instructions_executed, 1);
    assert_eq!(second.translation_context_changes, []);
    assert_eq!(state.rip, CODE_LINEAR + 6);
    assert_eq!(state.gpr[Register::Rax as usize], 0xaabb_ccdd_eeff_1234);
}

#[test]
fn stop_after_mov_cr3_precedes_any_walk_under_new_root() {
    let mut bus = cr3_switch_fixture();
    let root_b_entries = [
        (0x11_000, 0x12_007),
        (0x12_000, 0x13_007),
        (0x13_010, 0x14_007),
        (0x13_020, 0x15_007),
        (0x14_000, 0x16_007),
        (0x15_000, 0x17_007),
    ];
    let mut state = initial_cr3_state();
    let mut polls = 0;
    let result = run_paged_cr3_probe(
        &mut state,
        &mut bus,
        EFER,
        context(),
        2,
        FLAGS_DEFINED,
        || {
            polls += 1;
            polls == 2
        },
        |_| {},
    );

    assert_eq!(result.exit, PagedProbeExit::Stopped);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.cr3, Some(0x11_000));
    assert_eq!(state.rip, CODE_LINEAR + 3);
    assert_eq!(result.translation_context_changes.len(), 1);
    for (address, expected) in root_b_entries {
        assert_eq!(
            qword(&mut bus, address),
            expected,
            "root B entry {address:#x}"
        );
    }
}

#[test]
fn new_root_fetch_fault_keeps_mov_cr3_retirement_and_fault_context() {
    let mut bus = cr3_switch_fixture();
    // Root B is valid, but its code PTE is non-present. The MOV itself still
    // fetches through root A; only the following fetch observes this fault.
    put(&mut bus, 0x14_000, 0);
    let mut state = initial_cr3_state();
    let result = run_paged_cr3_probe(
        &mut state,
        &mut bus,
        EFER,
        context(),
        2,
        FLAGS_DEFINED,
        || false,
        |_| {},
    );

    let expected_fault = od_isa::memory::ArchitecturalFault {
        vector: 14,
        error_code: Some(0x10),
        linear_address: Some(CODE_LINEAR + 3),
    };
    assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.cr3, Some(0x11_000));
    assert_eq!(state.rip, CODE_LINEAR + 3);
    assert_eq!(result.translation_context_changes.len(), 1);
    let record = result.page_fault.as_ref().expect("next-root fetch fault");
    assert_eq!(record.fault(), &expected_fault);
    assert_eq!(record.saved_rip(), CODE_LINEAR + 3);
    assert_eq!(record.access(), AccessKind::Fetch);
}

#[test]
fn cpu_owned_cr3_profile_rejects_stores_io_and_hlt() {
    for bytes in [&[0x48, 0x89, 0x0b][..], &[0xec], &[0xee], &[0xf4]] {
        let (mut bus, _) = fixture();
        put_bytes_at(&mut bus, CODE_PHYSICAL, bytes);
        let mut state = initial_cr3_state();
        let before = state.clone();
        let result = run_paged_cr3_probe(
            &mut state,
            &mut bus,
            EFER,
            context(),
            1,
            FLAGS_DEFINED,
            || false,
            |_| {},
        );
        let opcode = if bytes[0] == 0x48 { bytes[1] } else { bytes[0] };
        assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode });
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
    }
}

#[test]
fn stop_precedes_fetch_budget_and_preserves_committed_prefix() {
    let (mut bus, _) = fixture();
    let mut state = initial_state(0x1234_5678_9abc_de41);
    let mut writes = Vec::new();
    let mut polls = 0;
    let result = run(
        &mut state,
        &mut bus,
        0,
        || {
            polls += 1;
            true
        },
        &mut writes,
    );
    assert_eq!(result.exit, PagedProbeExit::Stopped);
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(polls, 1);
    assert!(writes.is_empty());
    assert_eq!(state.rip, CODE_LINEAR);

    let (mut bus, handle) = fixture();
    let mut state = initial_state(0x1234_5678_9abc_de41);
    let mut writes = Vec::new();
    let mut polls = 0;
    let result = run(
        &mut state,
        &mut bus,
        4,
        || {
            polls += 1;
            polls == 2
        },
        &mut writes,
    );
    assert_eq!(result.exit, PagedProbeExit::Stopped);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, CODE_LINEAR + 3);
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(DATA_PHYSICAL), AccessWidth::Qword)
            .unwrap(),
        0x1234_5678_9abc_de41
    );
    assert!(
        writes
            .iter()
            .any(|range| range.start().get() == DATA_PHYSICAL)
    );
    assert!(!handle.transfer_holding_to_shift());
}

#[test]
fn stop_after_out_does_not_fetch_hlt_and_halted_repeat_does_not_fetch() {
    let (mut bus, handle) = fixture();
    let payload = 0xaabb_ccdd_eeff_0041;
    let mut state = initial_state(payload);
    let mut writes = Vec::new();
    let mut polls = 0;
    let result = run(
        &mut state,
        &mut bus,
        4,
        || {
            polls += 1;
            polls == 4
        },
        &mut writes,
    );
    assert_eq!(result.exit, PagedProbeExit::Stopped);
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(state.rip, CODE_LINEAR + 7);
    assert_payload(&mut bus, &handle, payload);

    let (mut bus, _) = fixture();
    let mut halted = initial_state(payload);
    let mut writes = Vec::new();
    let result = run(&mut halted, &mut bus, 4, || false, &mut writes);
    assert_eq!(result.exit, PagedProbeExit::Halted);
    let before = halted.clone();
    let writes_before_repeat = writes.clone();
    let result = run(&mut halted, &mut bus, 4, || false, &mut writes);
    assert_eq!(result.exit, PagedProbeExit::Halted);
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(halted, before);
    assert_eq!(writes, writes_before_repeat);
}

#[test]
fn pio_error_keeps_prior_store_and_reports_error_without_retiring_failed_instruction() {
    let (mut bus, _) = fixture();
    let mut state = initial_state(0x0102_0304_0506_0741);
    state.gpr[Register::Rdx as usize] = 0x1234;
    let mut writes = Vec::new();
    let result = run(&mut state, &mut bus, 4, || false, &mut writes);
    assert!(matches!(result.exit, PagedProbeExit::PortIo(_)));
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.rip, CODE_LINEAR + 6);
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(DATA_PHYSICAL), AccessWidth::Qword)
            .unwrap(),
        0x0102_0304_0506_0741
    );
    assert!(!writes.is_empty());
}

#[test]
fn unsupported_and_paged_fetch_fault_are_distinct_nonretiring_exits() {
    let (mut bus, _) = fixture();
    put_byte(&mut bus, CODE_PHYSICAL, 0x90);
    let mut state = initial_state(0);
    let before = state.clone();
    let mut writes = Vec::new();
    let result = run(&mut state, &mut bus, 1, || false, &mut writes);
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x90 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);

    let (mut bus, _) = fixture();
    put(&mut bus, ROOT + 0x3000, 0);
    let mut state = initial_state(0);
    let before = state.clone();
    let mut writes = Vec::new();
    let result = run(&mut state, &mut bus, 1, || false, &mut writes);
    assert!(matches!(result.exit, PagedProbeExit::Fault(_)));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(result.page_fault.is_some());
}

#[test]
fn paged_probe_handoff_field_reads_use_tracked_page_walk_and_zero_extend_registers() {
    let (mut bus, _) = fixture();
    put_bytes_at(
        &mut bus,
        CODE_PHYSICAL,
        &[
            0x0f, 0xb7, 0x7e, 0x08, // MOVZX EDI,word ptr [RSI+8]
            0x0f, 0xb7, 0x5e, 0x0a, // MOVZX EBX,word ptr [RSI+0xA]
            0x8b, 0x4e, 0x0c, // MOV ECX,dword ptr [RSI+0xC]
            0x8b, 0x4e, 0x10, // MOV ECX,dword ptr [RSI+0x10]
            0x8b, 0x7e, 0x14, // MOV EDI,dword ptr [RSI+0x14]
        ],
    );
    put(&mut bus, DATA_PHYSICAL + 8, 0xfeed_face_2468_1234);
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x0c, &[0xef, 0xcd, 0xab, 0x89]);
    put(&mut bus, DATA_PHYSICAL + 0x10, 0xcafe_babe_7654_3210);
    let mut state = initial_state(0);
    state.gpr[Register::Rsi as usize] = DATA_LINEAR;
    state.gpr[Register::Rdi as usize] = 0xaabb_ccdd_eeff_0011;
    state.gpr[Register::Rbx as usize] = 0x1122_3344_5566_7788;
    state.gpr[Register::Rcx as usize] = 0xfeed_face_0123_4567;
    let before_flags = state.rflags;
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 5, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 5);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state.rip, CODE_LINEAR + 17);
    assert_eq!(state.gpr[Register::Rdi as usize], 0xcafe_babe);
    assert_eq!(state.gpr[Register::Rbx as usize], 0x2468);
    assert_eq!(state.gpr[Register::Rcx as usize], 0x7654_3210);
    assert_eq!(state.rflags, before_flags);
    assert_eq!(
        qword(&mut bus, DATA_PHYSICAL + 8),
        0x89ab_cdef_2468_1234,
        "the handoff field loads perform no data write"
    );
    assert_eq!(
        qword(&mut bus, DATA_PHYSICAL + 0x10),
        0xcafe_babe_7654_3210,
        "the second dword handoff load performs no data write"
    );
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x2010), ROOT + 0x3007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x3000), CODE_PHYSICAL | 7 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x2020), ROOT + 0x4007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x4000), DATA_PHYSICAL | 7 | (1 << 5));
}

#[test]
fn paged_probe_movzx_r14d_byte_rsi_r11_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x46, 0x0f, 0xb6, 0x34, 0x1e]);
    put_byte(&mut bus, DATA_PHYSICAL + 0x18, 0xa7);
    let mut state = initial_state(0);
    state.gpr[Register::Rsi as usize] = DATA_LINEAR;
    state.gpr[Register::R11 as usize] = 0x18;
    state.gpr[Register::Rbp as usize] = 0x0000_8000_0000_0000; // not the memory base
    state.gpr[Register::R14 as usize] = 0xffff_ffff_dead_beef;
    let before_flags = state.rflags;
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state.rip, CODE_LINEAR + 5);
    assert_eq!(state.gpr[Register::R14 as usize], 0xa7);
    assert_eq!(state.rflags, before_flags);
    assert_eq!(
        bus.read_physical(
            GuestPhysicalAddress::new(DATA_PHYSICAL + 0x18),
            AccessWidth::Byte
        )
        .unwrap(),
        0xa7
    );
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x2020), ROOT + 0x4007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x4000), DATA_PHYSICAL | 7 | (1 << 5));
}

#[test]
fn paged_probe_movzx_r15d_byte_rsi_r11_disp8_one_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(
        &mut bus,
        CODE_PHYSICAL,
        &[0x46, 0x0f, 0xb6, 0x7c, 0x1e, 0x01],
    );
    put_byte(&mut bus, DATA_PHYSICAL + 0x19, 0x5c);
    let mut state = initial_state(0);
    state.gpr[Register::Rsi as usize] = DATA_LINEAR;
    state.gpr[Register::R11 as usize] = 0x18;
    state.gpr[Register::Rbp as usize] = 0x0000_8000_0000_0000; // not the memory base
    state.gpr[Register::R15 as usize] = 0xffff_ffff_dead_beef;
    let before_flags = state.rflags;
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state.rip, CODE_LINEAR + 6);
    assert_eq!(state.gpr[Register::R15 as usize], 0x5c);
    assert_eq!(state.rflags, before_flags);
    assert_eq!(
        bus.read_physical(
            GuestPhysicalAddress::new(DATA_PHYSICAL + 0x19),
            AccessWidth::Byte
        )
        .unwrap(),
        0x5c
    );
    // Verify Accessed bits in the four-level code and operand walks, including
    // the leaf PTEs for the code page and the data page read at RSI+R11+1.
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x2020), ROOT + 0x4007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x3000), CODE_PHYSICAL | 7 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x4000), DATA_PHYSICAL | 7 | (1 << 5));
}

#[test]
fn paged_probe_mov_edi_dword_rsi_disp8_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x8b, 0x7e, 0x14]);
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x14, &[0xef, 0xcd, 0xab, 0x89]);
    let original_qword = qword(&mut bus, DATA_PHYSICAL + 0x10);
    let mut state = initial_state(0);
    state.gpr[Register::Rsi as usize] = DATA_LINEAR;
    state.gpr[Register::Rdi as usize] = 0xaabb_ccdd_eeff_0011;
    let before_flags = state.rflags;
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state.rip, CODE_LINEAR + 3);
    assert_eq!(state.gpr[Register::Rdi as usize], 0x89ab_cdef);
    assert_eq!(state.rflags, before_flags);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x10), original_qword);
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x2020), ROOT + 0x4007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x4000), DATA_PHYSICAL | 7 | (1 << 5));
}

#[test]
fn paged_probe_mov_edi_dword_rsi_disp8_38_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x8b, 0x7e, 0x38]);
    put_bytes_at(
        &mut bus,
        DATA_PHYSICAL + 0x38,
        &0x7654_3210u32.to_le_bytes(),
    );
    let original_qword = qword(&mut bus, DATA_PHYSICAL + 0x38);
    let mut state = initial_state(0);
    state.gpr[Register::Rsi as usize] = DATA_LINEAR;
    state.gpr[Register::Rdi as usize] = 0xaabb_ccdd_eeff_0011;
    let before_flags = state.rflags;
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state.rip, CODE_LINEAR + 3);
    assert_eq!(state.gpr[Register::Rdi as usize], 0x7654_3210);
    assert_eq!(state.gpr[Register::Rsi as usize], DATA_LINEAR);
    assert_eq!(state.rflags, before_flags);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x38), original_qword);
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x2020), ROOT + 0x4007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x3000), CODE_PHYSICAL | 7 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x4000), DATA_PHYSICAL | 7 | (1 << 5));
}

#[test]
fn paged_probe_mov_r14d_dword_rsi_disp8_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x44, 0x8b, 0x76, 0x34]);
    put_bytes_at(
        &mut bus,
        DATA_PHYSICAL + 0x34,
        &0x89ab_cdefu32.to_le_bytes(),
    );
    let original_qword = qword(&mut bus, DATA_PHYSICAL + 0x30);
    let mut state = initial_state(0);
    state.gpr[Register::Rsi as usize] = DATA_LINEAR;
    state.gpr[Register::R14 as usize] = 0xaabb_ccdd_eeff_0011;
    let before_flags = state.rflags;
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state.rip, CODE_LINEAR + 4);
    assert_eq!(state.gpr[Register::R14 as usize], 0x89ab_cdef);
    assert_eq!(state.gpr[Register::Rsi as usize], DATA_LINEAR);
    assert_eq!(state.rflags, before_flags);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x30), original_qword);
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x2020), ROOT + 0x4007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x3000), CODE_PHYSICAL | 7 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x4000), DATA_PHYSICAL | 7 | (1 << 5));
}

#[test]
fn paged_probe_mov_r15d_dword_rsi_disp8_30_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x44, 0x8b, 0x7e, 0x30]);
    let source_qword = 0xa1b2_c3d4_5566_7788u64;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x30, &source_qword.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::Rsi as usize] = DATA_LINEAR;
    state.gpr[Register::R14 as usize] = 0x1122_3344_5566_7788;
    state.gpr[Register::R15 as usize] = 0xaabb_ccdd_eeff_0011;
    let before_flags = state.rflags;
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state.rip, CODE_LINEAR + 4);
    assert_eq!(state.gpr[Register::R14 as usize], 0x1122_3344_5566_7788);
    assert_eq!(state.gpr[Register::R15 as usize], 0x5566_7788);
    assert_eq!(state.gpr[Register::Rsi as usize], DATA_LINEAR);
    assert_eq!(state.rflags, before_flags);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x30), source_qword);
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x2020), ROOT + 0x4007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x3000), CODE_PHYSICAL | 7 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x4000), DATA_PHYSICAL | 7 | (1 << 5));
}

#[test]
fn paged_probe_mov_r13d_dword_rsi_disp8_3c_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x44, 0x8b, 0x6e, 0x3c]);
    let source_qword = 0xa1b2_c3d4_5566_7788u64;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x38, &source_qword.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::Rsi as usize] = DATA_LINEAR;
    state.gpr[Register::R13 as usize] = 0xaabb_ccdd_eeff_0011;
    let before_flags = state.rflags;
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state.rip, CODE_LINEAR + 4);
    assert_eq!(state.gpr[Register::R13 as usize], 0xa1b2_c3d4);
    assert_eq!(state.gpr[Register::Rsi as usize], DATA_LINEAR);
    assert_eq!(state.rflags, before_flags);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x38), source_qword);
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x2020), ROOT + 0x4007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x3000), CODE_PHYSICAL | 7 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x4000), DATA_PHYSICAL | 7 | (1 << 5));
}

#[test]
fn paged_probe_mov_r12d_dword_rsi_disp8_40_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x44, 0x8b, 0x66, 0x40]);
    let source_qword = 0xa1b2_c3d4_5566_7788u64;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x3c, &source_qword.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::Rsi as usize] = DATA_LINEAR;
    state.gpr[Register::R12 as usize] = 0xaabb_ccdd_eeff_0011;
    let before_flags = state.rflags;
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state.rip, CODE_LINEAR + 4);
    assert_eq!(state.gpr[Register::R12 as usize], 0xa1b2_c3d4);
    assert_eq!(state.gpr[Register::Rsi as usize], DATA_LINEAR);
    assert_eq!(state.rflags, before_flags);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x3c), source_qword);
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x2020), ROOT + 0x4007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x3000), CODE_PHYSICAL | 7 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x4000), DATA_PHYSICAL | 7 | (1 << 5));
}

#[test]
fn paged_probe_mov_r8d_dword_rsi_disp8_44_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x44, 0x8b, 0x46, 0x44]);
    let source_qword = 0xa1b2_c3d4_5566_7788u64;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x40, &source_qword.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::Rsi as usize] = DATA_LINEAR;
    state.gpr[Register::R8 as usize] = 0xaabb_ccdd_eeff_0011;
    let before_flags = state.rflags;
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state.rip, CODE_LINEAR + 4);
    assert_eq!(state.gpr[Register::R8 as usize], 0xa1b2_c3d4);
    assert_eq!(state.gpr[Register::Rsi as usize], DATA_LINEAR);
    assert_eq!(state.rflags, before_flags);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x40), source_qword);
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x2020), ROOT + 0x4007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x3000), CODE_PHYSICAL | 7 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x4000), DATA_PHYSICAL | 7 | (1 << 5));
}

#[test]
fn paged_probe_cmp_dword_rax_rsi_imm8_uses_tracked_four_level_read() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x83, 0x3c, 0x30, 0x00]);
    let compared = 0u32;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x30, &compared.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::Rax as usize] = DATA_LINEAR + 0x20;
    state.gpr[Register::Rsi as usize] = 0x10;
    let before_flags = state.rflags;
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state.rip, CODE_LINEAR + 4);
    assert_eq!(state.gpr[Register::Rax as usize], DATA_LINEAR + 0x20);
    assert_eq!(state.gpr[Register::Rsi as usize], 0x10);
    let arithmetic_flags = (1 << 0) | (1 << 2) | (1 << 4) | (1 << 6) | (1 << 7) | (1 << 11);
    assert_eq!(state.rflags & arithmetic_flags, (1 << 6) | (1 << 2));
    assert_eq!(
        state.rflags & !arithmetic_flags,
        before_flags & !arithmetic_flags
    );
    assert!(
        !writes.contains(&range(DATA_PHYSICAL + 0x30)),
        "CMP must not write the compared data"
    );
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x30), u64::from(compared));
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    assert_ne!(qword(&mut bus, ROOT + 0x2020) & (1 << 5), 0);
    assert_eq!(qword(&mut bus, ROOT + 0x2020) & (1 << 6), 0);
    assert_eq!(qword(&mut bus, ROOT + 0x3000), CODE_PHYSICAL | 7 | (1 << 5));
    assert_ne!(qword(&mut bus, ROOT + 0x4000) & (1 << 5), 0);
    assert_eq!(qword(&mut bus, ROOT + 0x4000) & (1 << 6), 0);
}

#[test]
fn paged_probe_cmp_dword_rax_rsi_imm8_read_fault_is_atomic() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x83, 0x3c, 0x30, 0x00]);
    put(&mut bus, ROOT + 0x4000, 0); // Data PTE is not present.
    let mut state = initial_state(0);
    state.gpr[Register::Rax as usize] = DATA_LINEAR + 0x20;
    state.gpr[Register::Rsi as usize] = 0x10;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    let expected_fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0),
        linear_address: Some(DATA_LINEAR + 0x30),
    };
    assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    let record = result
        .page_fault
        .as_ref()
        .expect("CMP data-read page fault");
    assert_eq!(record.fault(), &expected_fault);
    assert_eq!(record.access(), AccessKind::Read);
    assert!(!writes.contains(&range(DATA_PHYSICAL + 0x30)));
}

#[test]
fn paged_probe_cmp_dword_r12_rax_imm8_uses_tracked_four_level_read() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x41, 0x83, 0x3c, 0x04, 0x00]);
    let compared = 0u32;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x30, &compared.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::R12 as usize] = DATA_LINEAR;
    state.gpr[Register::Rax as usize] = 0x30;
    let before_flags = state.rflags;
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state.rip, CODE_LINEAR + 5);
    assert_eq!(state.gpr[Register::R12 as usize], DATA_LINEAR);
    assert_eq!(state.gpr[Register::Rax as usize], 0x30);
    let arithmetic_flags = (1 << 0) | (1 << 2) | (1 << 4) | (1 << 6) | (1 << 7) | (1 << 11);
    assert_eq!(state.rflags & arithmetic_flags, (1 << 6) | (1 << 2));
    assert_eq!(
        state.rflags & !arithmetic_flags,
        before_flags & !arithmetic_flags
    );
    assert!(
        !writes.contains(&range(DATA_PHYSICAL + 0x30)),
        "CMP must not write the compared data"
    );
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x30), u64::from(compared));
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    assert_ne!(qword(&mut bus, ROOT + 0x2020) & (1 << 5), 0);
    assert_eq!(qword(&mut bus, ROOT + 0x2020) & (1 << 6), 0);
    assert_eq!(qword(&mut bus, ROOT + 0x3000), CODE_PHYSICAL | 7 | (1 << 5));
    assert_ne!(qword(&mut bus, ROOT + 0x4000) & (1 << 5), 0);
    assert_eq!(qword(&mut bus, ROOT + 0x4000) & (1 << 6), 0);
}

#[test]
fn paged_probe_cmp_dword_r12_rax_imm8_read_fault_is_atomic() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x41, 0x83, 0x3c, 0x04, 0x00]);
    put(&mut bus, ROOT + 0x4000, 0); // Data PTE is not present.
    let mut state = initial_state(0);
    state.gpr[Register::R12 as usize] = DATA_LINEAR;
    state.gpr[Register::Rax as usize] = 0x30;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    let expected_fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0),
        linear_address: Some(DATA_LINEAR + 0x30),
    };
    assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    let record = result
        .page_fault
        .as_ref()
        .expect("CMP data-read page fault");
    assert_eq!(record.fault(), &expected_fault);
    assert_eq!(record.access(), AccessKind::Read);
    assert!(!writes.contains(&range(DATA_PHYSICAL + 0x30)));
}

#[test]
fn paged_probe_mov_r10d_rax_plus_rcx_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x44, 0x8b, 0x14, 0x08]);
    let source = 0x89ab_cdefu32;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x40, &source.to_le_bytes());
    let mut state = initial_state(0x20);
    state.gpr[Register::Rax as usize] = DATA_LINEAR + 0x20;
    state.gpr[Register::R10 as usize] = 0xaabb_ccdd_eeff_0011;
    let before_flags = state.rflags;
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state.rip, CODE_LINEAR + 4);
    assert_eq!(state.gpr[Register::R10 as usize], u64::from(source));
    assert_eq!(state.gpr[Register::Rax as usize], DATA_LINEAR + 0x20);
    assert_eq!(state.gpr[Register::Rcx as usize], 0x20);
    assert_eq!(state.rflags, before_flags);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x40), u64::from(source));
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x2020), ROOT + 0x4007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x3000), CODE_PHYSICAL | 7 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x4000), DATA_PHYSICAL | 7 | (1 << 5));
}

#[test]
fn paged_probe_mov_ebp_rax_plus_rcx_disp8_4_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x8b, 0x6c, 0x08, 0x04]);
    let value = 0x89ab_cdefu32;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x40, &value.to_le_bytes());
    let mut state = initial_state(0x1c);
    state.gpr[Register::Rax as usize] = DATA_LINEAR + 0x20;
    state.gpr[Register::Rbp as usize] = 0xaabb_ccdd_eeff_0011;
    let before_flags = state.rflags;
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state.rip, CODE_LINEAR + 4);
    assert_eq!(state.gpr[Register::Rbp as usize], u64::from(value));
    assert_eq!(state.gpr[Register::Rax as usize], DATA_LINEAR + 0x20);
    assert_eq!(state.gpr[Register::Rcx as usize], 0x1c);
    assert_eq!(state.rflags, before_flags);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x40), u64::from(value));
    for address in [ROOT, ROOT + 0x1000, ROOT + 0x2020, ROOT + 0x4000] {
        let entry = qword(&mut bus, address);
        assert_ne!(entry & (1 << 5), 0, "A bit missing at {address:#x}");
        assert_eq!(entry & (1 << 6), 0, "unexpected D bit at {address:#x}");
    }
}

#[test]
fn paged_probe_mov_ebp_rax_plus_rcx_disp8_4_data_page_fault_preserves_state() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x8b, 0x6c, 0x08, 0x04]);
    put(&mut bus, ROOT + 0x4000, 0); // Remove the data-page mapping.
    let mut state = initial_state(0x1c);
    state.gpr[Register::Rax as usize] = DATA_LINEAR + 0x20;
    state.gpr[Register::Rbp as usize] = 0xaabb_ccdd_eeff_0011;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    let expected_fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0),
        linear_address: Some(DATA_LINEAR + 0x40),
    };
    assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, FLAGS_DEFINED);
    assert_eq!(state, before);
    let record = result.page_fault.as_ref().expect("precise data read fault");
    assert_eq!(record.fault(), &expected_fault);
    assert_eq!(record.saved_rip(), CODE_LINEAR);
    assert_eq!(record.access(), AccessKind::Read);
    assert_eq!(qword(&mut bus, ROOT + 0x4000), 0);
}

#[test]
fn paged_probe_mov_ebp_r12_plus_rax_disp8_e8_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x41, 0x8b, 0x6c, 0x04, 0xe8]);
    let value = 0x89ab_cdefu32;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x28, &value.to_le_bytes());
    let mut state = initial_state(0x40);
    state.gpr[Register::R12 as usize] = DATA_LINEAR + 0x30;
    state.gpr[Register::Rax as usize] = 0x10;
    state.gpr[Register::Rbp as usize] = 0xaabb_ccdd_eeff_0011;
    let before_flags = state.rflags;
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state.rip, CODE_LINEAR + 5);
    assert_eq!(state.gpr[Register::Rbp as usize], u64::from(value));
    assert_eq!(state.gpr[Register::R12 as usize], DATA_LINEAR + 0x30);
    assert_eq!(state.gpr[Register::Rax as usize], 0x10);
    assert_eq!(state.rflags, before_flags);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x28), u64::from(value));
    for address in [
        ROOT,
        ROOT + 0x1000,
        ROOT + 0x2020,
        ROOT + 0x3000,
        ROOT + 0x4000,
    ] {
        let entry = qword(&mut bus, address);
        assert_ne!(entry & (1 << 5), 0, "A bit missing at {address:#x}");
        assert_eq!(entry & (1 << 6), 0, "unexpected D bit at {address:#x}");
    }
}

#[test]
fn paged_probe_mov_ebp_r12_plus_rax_disp8_e8_data_page_fault_preserves_state() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x41, 0x8b, 0x6c, 0x04, 0xe8]);
    put(&mut bus, ROOT + 0x4000, 0); // Remove the data-page mapping.
    let mut state = initial_state(0x40);
    state.gpr[Register::R12 as usize] = DATA_LINEAR + 0x30;
    state.gpr[Register::Rax as usize] = 0x10;
    state.gpr[Register::Rbp as usize] = 0xaabb_ccdd_eeff_0011;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    let expected_fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0),
        linear_address: Some(DATA_LINEAR + 0x28),
    };
    assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, FLAGS_DEFINED);
    assert_eq!(state, before);
    let record = result.page_fault.as_ref().expect("precise data read fault");
    assert_eq!(record.fault(), &expected_fault);
    assert_eq!(record.saved_rip(), CODE_LINEAR);
    assert_eq!(record.access(), AccessKind::Read);
    assert_eq!(qword(&mut bus, ROOT + 0x4000), 0);
}

#[test]
fn paged_probe_mov_r13d_rax_plus_rcx_disp8_18_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x44, 0x8b, 0x6c, 0x08, 0x18]);
    let value = 0x89ab_cdefu32;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x40, &value.to_le_bytes());
    let mut state = initial_state(0x08);
    state.gpr[Register::Rax as usize] = DATA_LINEAR + 0x20;
    state.gpr[Register::R13 as usize] = 0xaabb_ccdd_eeff_0011;
    let before_flags = state.rflags;
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state.rip, CODE_LINEAR + 5);
    assert_eq!(state.gpr[Register::R13 as usize], u64::from(value));
    assert_eq!(state.gpr[Register::Rax as usize], DATA_LINEAR + 0x20);
    assert_eq!(state.gpr[Register::Rcx as usize], 0x08);
    assert_eq!(state.rflags, before_flags);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x40), u64::from(value));
    for address in [ROOT, ROOT + 0x1000, ROOT + 0x2020, ROOT + 0x4000] {
        let entry = qword(&mut bus, address);
        assert_ne!(entry & (1 << 5), 0, "A bit missing at {address:#x}");
        assert_eq!(entry & (1 << 6), 0, "unexpected D bit at {address:#x}");
    }
}

#[test]
fn paged_probe_mov_r13d_rax_plus_rcx_disp8_18_data_page_fault_preserves_state() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x44, 0x8b, 0x6c, 0x08, 0x18]);
    put(&mut bus, ROOT + 0x4000, 0); // Remove the data-page mapping.
    let mut state = initial_state(0x08);
    state.gpr[Register::Rax as usize] = DATA_LINEAR + 0x20;
    state.gpr[Register::R13 as usize] = 0xaabb_ccdd_eeff_0011;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    let expected_fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0),
        linear_address: Some(DATA_LINEAR + 0x40),
    };
    assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, FLAGS_DEFINED);
    assert_eq!(state, before);
    let record = result.page_fault.as_ref().expect("precise data read fault");
    assert_eq!(record.fault(), &expected_fault);
    assert_eq!(record.saved_rip(), CODE_LINEAR);
    assert_eq!(record.access(), AccessKind::Read);
    assert_eq!(qword(&mut bus, ROOT + 0x4000), 0);
}

#[test]
fn paged_probe_mov_r13d_r12_plus_rax_disp8_fc_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x45, 0x8b, 0x6c, 0x04, 0xfc]);
    let value = 0x89ab_cdefu32;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x40, &value.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::R12 as usize] = DATA_LINEAR + 0x30;
    state.gpr[Register::Rax as usize] = 0x14;
    state.gpr[Register::R13 as usize] = 0xaabb_ccdd_eeff_0011;
    let before_flags = state.rflags;
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state.rip, CODE_LINEAR + 5);
    assert_eq!(state.gpr[Register::R13 as usize], u64::from(value));
    assert_eq!(state.gpr[Register::R12 as usize], DATA_LINEAR + 0x30);
    assert_eq!(state.gpr[Register::Rax as usize], 0x14);
    assert_eq!(state.rflags, before_flags);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x40), u64::from(value));
    for address in [ROOT, ROOT + 0x1000, ROOT + 0x2020, ROOT + 0x4000] {
        let entry = qword(&mut bus, address);
        assert_ne!(entry & (1 << 5), 0, "A bit missing at {address:#x}");
        assert_eq!(entry & (1 << 6), 0, "unexpected D bit at {address:#x}");
    }
}

#[test]
fn paged_probe_mov_r13d_r12_plus_rax_disp8_fc_data_page_fault_preserves_state() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x45, 0x8b, 0x6c, 0x04, 0xfc]);
    put(&mut bus, ROOT + 0x4000, 0); // Remove the data-page mapping.
    let mut state = initial_state(0);
    state.gpr[Register::R12 as usize] = DATA_LINEAR + 0x30;
    state.gpr[Register::Rax as usize] = 0x14;
    state.gpr[Register::R13 as usize] = 0xaabb_ccdd_eeff_0011;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    let expected_fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0),
        linear_address: Some(DATA_LINEAR + 0x40),
    };
    assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, FLAGS_DEFINED);
    assert_eq!(state, before);
    let record = result.page_fault.as_ref().expect("precise data read fault");
    assert_eq!(record.fault(), &expected_fault);
    assert_eq!(record.saved_rip(), CODE_LINEAR);
    assert_eq!(record.access(), AccessKind::Read);
    assert_eq!(qword(&mut bus, ROOT + 0x4000), 0);
}

#[test]
fn paged_probe_setne_rsp_disp8_07_stores_byte_with_tracked_four_level_walk() {
    for zero in [false, true] {
        let (mut bus, _) = fixture();
        put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x0f, 0x95, 0x44, 0x24, 0x07]);
        put_bytes_at(&mut bus, DATA_PHYSICAL + 6, &[0xaa, 0xbb, 0xcc]);
        let mut state = initial_state(0);
        state.gpr[Register::Rsp as usize] = DATA_LINEAR;
        if zero {
            state.rflags |= 1 << 6;
        } else {
            state.rflags &= !(1 << 6);
        }
        let before = state.clone();
        let mut writes = Vec::new();

        let result = run(&mut state, &mut bus, 1, || false, &mut writes);

        assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.page_fault, None);
        assert_eq!(result.memory_error, None);
        let mut expected = before;
        expected.rip += 5;
        assert_eq!(state, expected);
        let byte = bus
            .read_physical(
                GuestPhysicalAddress::new(DATA_PHYSICAL + 7),
                AccessWidth::Byte,
            )
            .unwrap();
        assert_eq!(byte, u64::from(!zero));
        assert_eq!(
            bus.read_physical(
                GuestPhysicalAddress::new(DATA_PHYSICAL + 6),
                AccessWidth::Byte,
            )
            .unwrap(),
            0xaa
        );
        assert_eq!(
            bus.read_physical(
                GuestPhysicalAddress::new(DATA_PHYSICAL + 8),
                AccessWidth::Byte,
            )
            .unwrap(),
            0xcc
        );
        let store_range =
            GuestPhysicalRange::new(GuestPhysicalAddress::new(DATA_PHYSICAL + 7), 1).unwrap();
        assert!(writes.contains(&store_range));
        for address in [ROOT, ROOT + 0x1000, ROOT + 0x2020, ROOT + 0x4000] {
            assert_ne!(
                qword(&mut bus, address) & (1 << 5),
                0,
                "A bit missing at {address:#x}"
            );
        }
        let data_pde = qword(&mut bus, ROOT + 0x2020);
        assert_eq!(data_pde & (1 << 6), 0);
        let data_pte = qword(&mut bus, ROOT + 0x4000);
        assert_ne!(data_pte & (1 << 6), 0, "successful byte store sets D");
    }
}

#[test]
fn paged_probe_setne_rsp_disp8_07_write_protection_fault_is_atomic() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x0f, 0x95, 0x44, 0x24, 0x07]);
    let original = [0xaa, 0xbb, 0xcc];
    put_bytes_at(&mut bus, DATA_PHYSICAL + 6, &original);
    put(&mut bus, 0x5000, DATA_PHYSICAL | 5); // supervisor read-only data page
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = DATA_LINEAR;
    state.rflags &= !(1 << 6); // SETNE would write 1 if the store succeeded.
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    let expected_fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x3),
        linear_address: Some(DATA_LINEAR + 7),
    };
    assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.memory_error, None);
    assert_eq!(state, before);
    let record = result.page_fault.as_ref().expect("byte store WP fault");
    assert_eq!(record.fault(), &expected_fault);
    assert_eq!(record.saved_rip(), CODE_LINEAR);
    assert_eq!(record.access(), AccessKind::Write);
    for (address, expected) in [(6, 0xaa), (7, 0xbb), (8, 0xcc)] {
        assert_eq!(
            bus.read_physical(
                GuestPhysicalAddress::new(DATA_PHYSICAL + address),
                AccessWidth::Byte,
            )
            .unwrap(),
            expected
        );
    }
    let leaf = qword(&mut bus, ROOT + 0x4000);
    assert_ne!(leaf & (1 << 5), 0, "faulting write sets A");
    assert_eq!(leaf & (1 << 6), 0, "faulting write does not set D");
    let store_range =
        GuestPhysicalRange::new(GuestPhysicalAddress::new(DATA_PHYSICAL + 7), 1).unwrap();
    assert!(!writes.contains(&store_range));
}

#[test]
fn paged_probe_test_rsp_disp8_07_bpl_uses_tracked_four_level_read() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x40, 0x84, 0x6c, 0x24, 0x07]);
    put_byte(&mut bus, DATA_PHYSICAL + 7, 0x03);
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = DATA_LINEAR;
    state.gpr[Register::Rbp as usize] = 0xaabb_ccdd_eeff_0003;
    state.rflags = 0x8d5;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    let mut expected = before;
    expected.rip += 5;
    expected.rflags = (expected.rflags & !0x8d5) | 0x14; // AF retained, PF set.
    assert_eq!(state, expected);
    assert_eq!(result.rflags_defined, FLAGS_DEFINED);
    let data_range =
        GuestPhysicalRange::new(GuestPhysicalAddress::new(DATA_PHYSICAL + 7), 1).unwrap();
    assert!(
        !writes.contains(&data_range),
        "TEST must not write the data byte"
    );
    assert_eq!(
        bus.read_physical(
            GuestPhysicalAddress::new(DATA_PHYSICAL + 7),
            AccessWidth::Byte,
        )
        .unwrap(),
        0x03
    );
    for address in [ROOT, ROOT + 0x1000, ROOT + 0x2020, ROOT + 0x4000] {
        assert_ne!(
            qword(&mut bus, address) & (1 << 5),
            0,
            "A bit missing at {address:#x}"
        );
    }
    assert_eq!(qword(&mut bus, ROOT + 0x2020) & (1 << 6), 0);
    assert_eq!(qword(&mut bus, ROOT + 0x4000) & (1 << 6), 0);
}

#[test]
fn paged_probe_test_rsp_disp8_07_bpl_data_page_fault_is_atomic() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x40, 0x84, 0x6c, 0x24, 0x07]);
    put(&mut bus, ROOT + 0x4000, 0); // absent leaf for the data page
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = DATA_LINEAR;
    state.gpr[Register::Rbp as usize] = 0xaabb_ccdd_eeff_0003;
    state.rflags = 0x8d5;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    let expected_fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0),
        linear_address: Some(DATA_LINEAR + 7),
    };
    assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    let data_range =
        GuestPhysicalRange::new(GuestPhysicalAddress::new(DATA_PHYSICAL + 7), 1).unwrap();
    assert!(
        !writes.contains(&data_range),
        "read fault must not write the data byte"
    );
    let record = result.page_fault.as_ref().expect("data read #PF");
    assert_eq!(record.fault(), &expected_fault);
    assert_eq!(record.saved_rip(), CODE_LINEAR);
    assert_eq!(record.access(), AccessKind::Read);
    assert_eq!(qword(&mut bus, ROOT + 0x4000), 0);
}

#[test]
fn paged_probe_mov_rsp_disp8_rdx_store_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x48, 0x89, 0x54, 0x24, 0x30]);
    let old_value = 0xfeed_face_cafe_beefu64;
    let value = 0x0123_4567_89ab_cdefu64;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x30, &old_value.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = DATA_LINEAR;
    state.gpr[Register::Rdx as usize] = value;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    let mut expected = before;
    expected.rip += 5;
    assert_eq!(state, expected);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x30), value);
    assert!(writes.contains(&range(0x1000)));
    assert!(writes.contains(&range(0x2000)));
    assert!(writes.contains(&range(0x3010)));
    assert!(writes.contains(&range(0x4000)));
    assert!(writes.contains(&range(0x3020)));
    assert!(writes.contains(&range(0x5000)));
    assert!(writes.contains(&range(DATA_PHYSICAL + 0x30)));
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    let data_pde = qword(&mut bus, ROOT + 0x2020);
    assert_ne!(data_pde & (1 << 5), 0);
    assert_eq!(data_pde & (1 << 6), 0);
    let data_pte = qword(&mut bus, ROOT + 0x4000);
    assert_ne!(data_pte & (1 << 5), 0);
    assert_ne!(data_pte & (1 << 6), 0);
}

#[test]
fn paged_probe_call_rel32_0x579_pushes_return_rip_with_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0xe8, 0x79, 0x05, 0, 0]);
    let mut state = initial_state(0);
    state.rip = CODE_LINEAR;
    state.gpr[Register::Rsp as usize] = DATA_LINEAR + 0x100;
    state.rflags = 0xd5a;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    let return_rip = CODE_LINEAR + 5;
    let mut expected = before;
    expected.gpr[Register::Rsp as usize] = DATA_LINEAR + 0xf8;
    expected.rip = CODE_LINEAR + 5 + 0x579;
    assert_eq!(state, expected);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0xf8), return_rip);
    assert!(writes.contains(&range(0x1000)));
    assert!(writes.contains(&range(0x2000)));
    assert!(writes.contains(&range(0x3010)));
    assert!(writes.contains(&range(0x4000)));
    assert!(writes.contains(&range(0x3020)));
    assert!(writes.contains(&range(0x5000)));
    assert!(writes.contains(&range(DATA_PHYSICAL + 0xf8)));
    assert_ne!(qword(&mut bus, ROOT) & (1 << 5), 0);
    assert_ne!(qword(&mut bus, ROOT + 0x1000) & (1 << 5), 0);
    assert_ne!(qword(&mut bus, ROOT + 0x2020) & (1 << 5), 0);
    assert_eq!(qword(&mut bus, ROOT + 0x2020) & (1 << 6), 0);
    let data_pte = qword(&mut bus, ROOT + 0x4000);
    assert_ne!(data_pte & (1 << 5), 0);
    assert_ne!(data_pte & (1 << 6), 0);
}

#[test]
fn paged_probe_call_rel32_0x579_write_protection_fault_is_atomic() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0xe8, 0x79, 0x05, 0, 0]);
    let old_stack = 0xfeed_face_cafe_beefu64;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0xf8, &old_stack.to_le_bytes());
    put(&mut bus, 0x5000, DATA_PHYSICAL | 5);
    let mut state = initial_state(0);
    state.rip = CODE_LINEAR;
    state.gpr[Register::Rsp as usize] = DATA_LINEAR + 0x100;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    let expected_fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x3),
        linear_address: Some(DATA_LINEAR + 0xf8),
    };
    assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.memory_error, None);
    assert_eq!(state, before);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0xf8), old_stack);
    let record = result
        .page_fault
        .as_ref()
        .expect("stack write-protection fault");
    assert_eq!(record.fault(), &expected_fault);
    assert_eq!(record.saved_rip(), CODE_LINEAR);
    assert_eq!(record.access(), AccessKind::Write);
    let data_pte = qword(&mut bus, ROOT + 0x4000);
    assert_ne!(data_pte & (1 << 5), 0);
    assert_eq!(data_pte & (1 << 6), 0);
    assert!(!writes.contains(&range(DATA_PHYSICAL + 0xf8)));
}

#[test]
fn paged_probe_push_rax_uses_tracked_four_level_stack_write() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x50]);
    let value = 0xf123_4567_89ab_cdefu64;
    let mut state = initial_state(0);
    state.gpr[Register::Rax as usize] = value;
    state.gpr[Register::Rsp as usize] = DATA_LINEAR + 0x100;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    let mut expected = before;
    expected.gpr[Register::Rsp as usize] = DATA_LINEAR + 0xf8;
    expected.rip += 1;
    assert_eq!(state, expected);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0xf8), value);
    for address in [0x1000, 0x2000, 0x3010, 0x4000, 0x3020, 0x5000] {
        assert!(writes.contains(&range(address)));
    }
    assert!(writes.contains(&range(DATA_PHYSICAL + 0xf8)));
    for address in [ROOT, ROOT + 0x1000, ROOT + 0x2020] {
        let entry = qword(&mut bus, address);
        assert_ne!(entry & (1 << 5), 0, "A bit missing at {address:#x}");
        assert_eq!(entry & (1 << 6), 0, "unexpected D bit at {address:#x}");
    }
    let leaf = qword(&mut bus, ROOT + 0x4000);
    assert_ne!(leaf & (1 << 5), 0);
    assert_ne!(leaf & (1 << 6), 0);
}

#[test]
fn paged_probe_push_rax_write_protection_fault_is_atomic() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x50]);
    let old_stack = 0xfeed_face_cafe_beefu64;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0xf8, &old_stack.to_le_bytes());
    put(&mut bus, 0x5000, DATA_PHYSICAL | 5);
    let mut state = initial_state(0);
    state.gpr[Register::Rax as usize] = 0xf123_4567_89ab_cdef;
    state.gpr[Register::Rsp as usize] = DATA_LINEAR + 0x100;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    let expected_fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x3),
        linear_address: Some(DATA_LINEAR + 0xf8),
    };
    assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.memory_error, None);
    assert_eq!(state, before);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0xf8), old_stack);
    let record = result
        .page_fault
        .as_ref()
        .expect("stack write-protection fault");
    assert_eq!(record.fault(), &expected_fault);
    assert_eq!(record.saved_rip(), CODE_LINEAR);
    assert_eq!(record.access(), AccessKind::Write);
    let leaf = qword(&mut bus, ROOT + 0x4000);
    assert_ne!(leaf & (1 << 5), 0);
    assert_eq!(leaf & (1 << 6), 0);
    assert!(!writes.contains(&range(DATA_PHYSICAL + 0xf8)));
}

#[test]
fn paged_probe_pop_rbx_uses_tracked_four_level_stack_read() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x5b]);
    let value = 0xfedc_ba98_7654_3210u64;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x100, &value.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = DATA_LINEAR + 0x100;
    state.gpr[Register::Rbx as usize] = 0xaabb_ccdd_eeff_0011;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    let mut expected = before;
    expected.rip += 1;
    expected.gpr[Register::Rsp as usize] = DATA_LINEAR + 0x108;
    expected.gpr[Register::Rbx as usize] = value;
    assert_eq!(state, expected);
    let data_range = range(DATA_PHYSICAL + 0x100);
    assert!(
        !writes.contains(&data_range),
        "POP must not write stack data"
    );
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x100), value);
    for address in [ROOT, ROOT + 0x1000, ROOT + 0x2020, ROOT + 0x4000] {
        let entry = qword(&mut bus, address);
        assert_ne!(entry & (1 << 5), 0, "A bit missing at {address:#x}");
        assert_eq!(entry & (1 << 6), 0, "unexpected D bit at {address:#x}");
    }
}

#[test]
fn paged_probe_pop_rbx_stack_page_fault_is_atomic() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x5b]);
    put(&mut bus, ROOT + 0x4000, 0); // Data leaf is not present.
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = DATA_LINEAR + 0x100;
    state.gpr[Register::Rbx as usize] = 0xaabb_ccdd_eeff_0011;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    let expected_fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0),
        linear_address: Some(DATA_LINEAR + 0x100),
    };
    assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.memory_error, None);
    assert_eq!(state, before);
    let record = result
        .page_fault
        .as_ref()
        .expect("POP stack-read page fault");
    assert_eq!(record.fault(), &expected_fault);
    assert_eq!(record.saved_rip(), CODE_LINEAR);
    assert_eq!(record.access(), AccessKind::Read);
    assert!(!writes.contains(&range(DATA_PHYSICAL + 0x100)));
    assert_eq!(qword(&mut bus, ROOT + 0x4000), 0);
}

#[test]
fn paged_probe_pop_rbx_noncanonical_rsp_raises_ss_without_retiring() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x5b]);
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0000;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(ArchitecturalFault {
            vector: 12,
            error_code: Some(0),
            linear_address: None,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state, before);
}

#[test]
fn paged_probe_pop_r12_uses_tracked_four_level_stack_read() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x41, 0x5c]);
    let value = 0xfedc_ba98_7654_3210u64;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x100, &value.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = DATA_LINEAR + 0x100;
    state.gpr[Register::R12 as usize] = 0xaabb_ccdd_eeff_0011;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    let mut expected = before;
    expected.rip += 2;
    expected.gpr[Register::Rsp as usize] = DATA_LINEAR + 0x108;
    expected.gpr[Register::R12 as usize] = value;
    assert_eq!(state, expected);
    let data_range = range(DATA_PHYSICAL + 0x100);
    assert!(
        !writes.contains(&data_range),
        "POP must not write stack data"
    );
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x100), value);
    for address in [ROOT, ROOT + 0x1000, ROOT + 0x2020, ROOT + 0x4000] {
        let entry = qword(&mut bus, address);
        assert_ne!(entry & (1 << 5), 0, "A bit missing at {address:#x}");
        assert_eq!(entry & (1 << 6), 0, "unexpected D bit at {address:#x}");
    }
}

#[test]
fn paged_probe_pop_r12_stack_page_fault_is_atomic() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x41, 0x5c]);
    put(&mut bus, ROOT + 0x4000, 0); // Data leaf is not present.
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = DATA_LINEAR + 0x100;
    state.gpr[Register::R12 as usize] = 0xaabb_ccdd_eeff_0011;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    let expected_fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0),
        linear_address: Some(DATA_LINEAR + 0x100),
    };
    assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.memory_error, None);
    assert_eq!(state, before);
    let record = result
        .page_fault
        .as_ref()
        .expect("POP stack-read page fault");
    assert_eq!(record.fault(), &expected_fault);
    assert_eq!(record.saved_rip(), CODE_LINEAR);
    assert_eq!(record.access(), AccessKind::Read);
    assert!(!writes.contains(&range(DATA_PHYSICAL + 0x100)));
    assert_eq!(qword(&mut bus, ROOT + 0x4000), 0);
}

#[test]
fn paged_probe_pop_r12_noncanonical_rsp_raises_ss_without_retiring() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x41, 0x5c]);
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0000;
    state.gpr[Register::R12 as usize] = 0xaabb_ccdd_eeff_0011;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(ArchitecturalFault {
            vector: 12,
            error_code: Some(0),
            linear_address: None,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state, before);
}

#[test]
fn paged_probe_pop_r13_uses_tracked_four_level_stack_read() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x41, 0x5d]);
    let value = 0xfedc_ba98_7654_3210u64;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x100, &value.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = DATA_LINEAR + 0x100;
    state.gpr[Register::R13 as usize] = 0xaabb_ccdd_eeff_0011;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    let mut expected = before;
    expected.rip += 2;
    expected.gpr[Register::Rsp as usize] = DATA_LINEAR + 0x108;
    expected.gpr[Register::R13 as usize] = value;
    assert_eq!(state, expected);
    let data_range = range(DATA_PHYSICAL + 0x100);
    assert!(
        !writes.contains(&data_range),
        "POP must not write stack data"
    );
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x100), value);
    for address in [ROOT, ROOT + 0x1000, ROOT + 0x2020, ROOT + 0x4000] {
        let entry = qword(&mut bus, address);
        assert_ne!(entry & (1 << 5), 0, "A bit missing at {address:#x}");
        assert_eq!(entry & (1 << 6), 0, "unexpected D bit at {address:#x}");
    }
}

#[test]
fn paged_probe_pop_r13_stack_page_fault_is_atomic() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x41, 0x5d]);
    put(&mut bus, ROOT + 0x4000, 0); // Data leaf is not present.
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = DATA_LINEAR + 0x100;
    state.gpr[Register::R13 as usize] = 0xaabb_ccdd_eeff_0011;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    let expected_fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0),
        linear_address: Some(DATA_LINEAR + 0x100),
    };
    assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.memory_error, None);
    assert_eq!(state, before);
    let record = result
        .page_fault
        .as_ref()
        .expect("POP stack-read page fault");
    assert_eq!(record.fault(), &expected_fault);
    assert_eq!(record.saved_rip(), CODE_LINEAR);
    assert_eq!(record.access(), AccessKind::Read);
    assert!(!writes.contains(&range(DATA_PHYSICAL + 0x100)));
    assert_eq!(qword(&mut bus, ROOT + 0x4000), 0);
}

#[test]
fn paged_probe_pop_r13_noncanonical_rsp_raises_ss_without_retiring() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x41, 0x5d]);
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0000;
    state.gpr[Register::R13 as usize] = 0xaabb_ccdd_eeff_0011;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(ArchitecturalFault {
            vector: 12,
            error_code: Some(0),
            linear_address: None,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state, before);
}

#[test]
fn paged_probe_pop_r14_uses_tracked_four_level_stack_read() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x41, 0x5e]);
    let value = 0xfedc_ba98_7654_3210u64;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x100, &value.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = DATA_LINEAR + 0x100;
    state.gpr[Register::R14 as usize] = 0xaabb_ccdd_eeff_0011;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    let mut expected = before;
    expected.rip += 2;
    expected.gpr[Register::Rsp as usize] = DATA_LINEAR + 0x108;
    expected.gpr[Register::R14 as usize] = value;
    assert_eq!(state, expected);
    let data_range = range(DATA_PHYSICAL + 0x100);
    assert!(
        !writes.contains(&data_range),
        "POP must not write stack data"
    );
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x100), value);
    for address in [ROOT, ROOT + 0x1000, ROOT + 0x2020, ROOT + 0x4000] {
        let entry = qword(&mut bus, address);
        assert_ne!(entry & (1 << 5), 0, "A bit missing at {address:#x}");
        assert_eq!(entry & (1 << 6), 0, "unexpected D bit at {address:#x}");
    }
}

#[test]
fn paged_probe_pop_r14_stack_page_fault_is_atomic() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x41, 0x5e]);
    put(&mut bus, ROOT + 0x4000, 0); // Data leaf is not present.
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = DATA_LINEAR + 0x100;
    state.gpr[Register::R14 as usize] = 0xaabb_ccdd_eeff_0011;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    let expected_fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0),
        linear_address: Some(DATA_LINEAR + 0x100),
    };
    assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.memory_error, None);
    assert_eq!(state, before);
    let record = result
        .page_fault
        .as_ref()
        .expect("POP stack-read page fault");
    assert_eq!(record.fault(), &expected_fault);
    assert_eq!(record.saved_rip(), CODE_LINEAR);
    assert_eq!(record.access(), AccessKind::Read);
    assert!(!writes.contains(&range(DATA_PHYSICAL + 0x100)));
    assert_eq!(qword(&mut bus, ROOT + 0x4000), 0);
}

#[test]
fn paged_probe_pop_r14_noncanonical_rsp_raises_ss_without_retiring() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x41, 0x5e]);
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0000;
    state.gpr[Register::R14 as usize] = 0xaabb_ccdd_eeff_0011;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(ArchitecturalFault {
            vector: 12,
            error_code: Some(0),
            linear_address: None,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state, before);
}

#[test]
fn paged_probe_mov_rsp_disp8_rdx_write_protection_fault_is_atomic() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x48, 0x89, 0x54, 0x24, 0x30]);
    let original_value = 0xfeed_face_cafe_beefu64;
    put_bytes_at(
        &mut bus,
        DATA_PHYSICAL + 0x30,
        &original_value.to_le_bytes(),
    );
    // Present, readable and supervisor read-only with CR0.WP enabled.
    put(&mut bus, 0x5000, DATA_PHYSICAL | 5);
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = DATA_LINEAR;
    state.gpr[Register::Rdx as usize] = 0xaabb_ccdd_eeff_0011;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    let expected_fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x3),
        linear_address: Some(DATA_LINEAR + 0x30),
    };
    assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.memory_error, None);
    assert_eq!(state, before);
    let record = result.page_fault.as_ref().expect("write-protection fault");
    assert_eq!(record.fault(), &expected_fault);
    assert_eq!(record.access(), AccessKind::Write);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x30), original_value);
    let data_pte = qword(&mut bus, ROOT + 0x4000);
    assert_ne!(data_pte & (1 << 5), 0, "data translation is accessed");
    assert_eq!(data_pte & (1 << 6), 0, "faulting store does not set dirty");
    assert!(!writes.contains(&range(DATA_PHYSICAL + 0x30)));
}

#[test]
fn paged_probe_mov_rsp_disp8_rsi_store_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x48, 0x89, 0x74, 0x24, 0x10]);
    let old_value = 0xfeed_face_cafe_beefu64;
    let value = 0x0123_4567_89ab_cdefu64;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x10, &old_value.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = DATA_LINEAR;
    state.gpr[Register::Rsi as usize] = value;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    let mut expected = before;
    expected.rip += 5;
    assert_eq!(state, expected);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x10), value);
    assert!(writes.contains(&range(0x1000)));
    assert!(writes.contains(&range(0x2000)));
    assert!(writes.contains(&range(0x3010)));
    assert!(writes.contains(&range(0x4000)));
    assert!(writes.contains(&range(0x3020)));
    assert!(writes.contains(&range(0x5000)));
    assert!(writes.contains(&range(DATA_PHYSICAL + 0x10)));
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    let data_pde = qword(&mut bus, ROOT + 0x2020);
    assert_ne!(data_pde & (1 << 5), 0);
    assert_eq!(data_pde & (1 << 6), 0);
    let data_pte = qword(&mut bus, ROOT + 0x4000);
    assert_ne!(data_pte & (1 << 5), 0);
    assert_ne!(data_pte & (1 << 6), 0);
}

#[test]
fn paged_probe_mov_rsi_rsp_disp8_load_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x48, 0x8b, 0x74, 0x24, 0x10]);
    let value = 0x0123_4567_89ab_cdefu64;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x10, &value.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = DATA_LINEAR;
    state.gpr[Register::Rsi as usize] = 0xfeed_face_cafe_beef;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    let mut expected = before;
    expected.gpr[Register::Rsi as usize] = value;
    expected.rip += 5;
    assert_eq!(state, expected);
    assert!(!writes.contains(&range(DATA_PHYSICAL + 0x10)));
    assert!(writes.contains(&range(0x1000)));
    assert!(writes.contains(&range(0x2000)));
    assert!(writes.contains(&range(0x3010)));
    assert!(writes.contains(&range(0x4000)));
    assert!(writes.contains(&range(0x3020)));
    assert!(writes.contains(&range(0x5000)));
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    let data_pde = qword(&mut bus, ROOT + 0x2020);
    assert_ne!(data_pde & (1 << 5), 0);
    assert_eq!(data_pde & (1 << 6), 0, "read does not set dirty");
    let data_pte = qword(&mut bus, ROOT + 0x4000);
    assert_ne!(data_pte & (1 << 5), 0);
    assert_eq!(data_pte & (1 << 6), 0, "read does not set dirty");
}

#[test]
fn paged_probe_mov_rdx_rsp_disp8_load_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x48, 0x8b, 0x54, 0x24, 0x30]);
    let value = 0xfedc_ba98_7654_3210u64;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x30, &value.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = DATA_LINEAR;
    state.gpr[Register::Rdx as usize] = 0x0123_4567_89ab_cdef;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    let mut expected = before;
    expected.gpr[Register::Rdx as usize] = value;
    expected.rip += 5;
    assert_eq!(state, expected);
    assert!(!writes.contains(&range(DATA_PHYSICAL + 0x30)));
    assert!(writes.contains(&range(0x1000)));
    assert!(writes.contains(&range(0x2000)));
    assert!(writes.contains(&range(0x3010)));
    assert!(writes.contains(&range(0x4000)));
    assert!(writes.contains(&range(0x3020)));
    assert!(writes.contains(&range(0x5000)));
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    let data_pde = qword(&mut bus, ROOT + 0x2020);
    assert_ne!(data_pde & (1 << 5), 0);
    assert_eq!(data_pde & (1 << 6), 0, "read does not set dirty");
    let data_pte = qword(&mut bus, ROOT + 0x4000);
    assert_ne!(data_pte & (1 << 5), 0);
    assert_eq!(data_pte & (1 << 6), 0, "read does not set dirty");
}

#[test]
fn paged_probe_mov_r14_rax_rcx_disp8_load_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x4c, 0x8b, 0x74, 0x08, 0x10]);
    let value = 0xfedc_ba98_7654_3210u64;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x60, &value.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::Rax as usize] = DATA_LINEAR + 0x20;
    state.gpr[Register::Rcx as usize] = 0x30;
    state.gpr[Register::R14 as usize] = 0x0123_4567_89ab_cdef;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    let mut expected = before;
    expected.gpr[Register::R14 as usize] = value;
    expected.rip += 5;
    assert_eq!(state, expected);
    assert!(!writes.contains(&range(DATA_PHYSICAL + 0x60)));
    assert!(writes.contains(&range(0x1000)));
    assert!(writes.contains(&range(0x2000)));
    assert!(writes.contains(&range(0x3010)));
    assert!(writes.contains(&range(0x4000)));
    assert!(writes.contains(&range(0x3020)));
    assert!(writes.contains(&range(0x5000)));
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    let data_pde = qword(&mut bus, ROOT + 0x2020);
    assert_ne!(data_pde & (1 << 5), 0);
    assert_eq!(data_pde & (1 << 6), 0, "read does not set dirty");
    let data_pte = qword(&mut bus, ROOT + 0x4000);
    assert_ne!(data_pte & (1 << 5), 0);
    assert_eq!(data_pte & (1 << 6), 0, "read does not set dirty");
}

#[test]
fn paged_probe_mov_r10_rax_rcx_plus_eight_load_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x4c, 0x8b, 0x54, 0x08, 0x08]);
    let value = 0xfedc_ba98_7654_3210u64;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x58, &value.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::Rax as usize] = DATA_LINEAR + 0x20;
    state.gpr[Register::Rcx as usize] = 0x30;
    state.gpr[Register::R10 as usize] = 0x0123_4567_89ab_cdef;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    let mut expected = before;
    expected.gpr[Register::R10 as usize] = value;
    expected.rip += 5;
    assert_eq!(state, expected);
    assert!(!writes.contains(&range(DATA_PHYSICAL + 0x58)));
    assert!(writes.contains(&range(0x1000)));
    assert!(writes.contains(&range(0x2000)));
    assert!(writes.contains(&range(0x3010)));
    assert!(writes.contains(&range(0x4000)));
    assert!(writes.contains(&range(0x3020)));
    assert!(writes.contains(&range(0x5000)));
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    let data_pde = qword(&mut bus, ROOT + 0x2020);
    assert_ne!(data_pde & (1 << 5), 0);
    assert_eq!(data_pde & (1 << 6), 0, "read does not set dirty");
    let data_pte = qword(&mut bus, ROOT + 0x4000);
    assert_ne!(data_pte & (1 << 5), 0);
    assert_eq!(data_pte & (1 << 6), 0, "read does not set dirty");
}

#[test]
fn paged_probe_mov_r10_rax_rcx_plus_eight_fault_does_not_commit_destination() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x4c, 0x8b, 0x54, 0x08, 0x08]);
    let mut state = initial_state(0);
    state.gpr[Register::Rax as usize] = DATA_LINEAR + 0x1000;
    state.gpr[Register::Rcx as usize] = 0;
    state.gpr[Register::R10 as usize] = 0x0123_4567_89ab_cdef;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert!(matches!(result.exit, PagedProbeExit::Fault(_)));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    let record = result.page_fault.as_ref().expect("unmapped data read #PF");
    assert_eq!(record.access(), AccessKind::Read);
    assert_eq!(record.saved_rip(), CODE_LINEAR);
    assert_eq!(record.fault().linear_address, Some(DATA_LINEAR + 0x1008));
}

#[test]
fn paged_probe_mov_rsi_r12_rax_minus_twelve_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x49, 0x8b, 0x74, 0x04, 0xf4]);
    let value = 0xfedc_ba98_7654_3210u64;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x44, &value.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::R12 as usize] = DATA_LINEAR + 0x40;
    state.gpr[Register::Rax as usize] = 0x10;
    state.gpr[Register::Rsi as usize] = 0x0123_4567_89ab_cdef;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    let mut expected = before;
    expected.gpr[Register::Rsi as usize] = value;
    expected.rip += 5;
    assert_eq!(state, expected);
    assert!(!writes.contains(&range(DATA_PHYSICAL + 0x44)));
    assert!(writes.contains(&range(0x1000)));
    assert!(writes.contains(&range(0x2000)));
    assert!(writes.contains(&range(0x3010)));
    assert!(writes.contains(&range(0x4000)));
    assert!(writes.contains(&range(0x3020)));
    assert!(writes.contains(&range(0x5000)));
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    let data_pde = qword(&mut bus, ROOT + 0x2020);
    assert_ne!(data_pde & (1 << 5), 0);
    assert_eq!(data_pde & (1 << 6), 0, "read does not set dirty");
    let data_pte = qword(&mut bus, ROOT + 0x4000);
    assert_ne!(data_pte & (1 << 5), 0);
    assert_eq!(data_pte & (1 << 6), 0, "read does not set dirty");
}

#[test]
fn paged_probe_mov_rsi_r12_rax_minus_twelve_fault_does_not_commit_destination() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x49, 0x8b, 0x74, 0x04, 0xf4]);
    let mut state = initial_state(0);
    state.gpr[Register::R12 as usize] = DATA_LINEAR + 0x1000;
    state.gpr[Register::Rax as usize] = 0x0c;
    state.gpr[Register::Rsi as usize] = 0x0123_4567_89ab_cdef;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert!(matches!(result.exit, PagedProbeExit::Fault(_)));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    let record = result.page_fault.as_ref().expect("unmapped data read #PF");
    assert_eq!(record.access(), AccessKind::Read);
    assert_eq!(record.saved_rip(), CODE_LINEAR);
    assert_eq!(record.fault().linear_address, Some(DATA_LINEAR + 0x1000));
}

#[test]
fn paged_probe_mov_r10_r12_rax_minus_twenty_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x4d, 0x8b, 0x54, 0x04, 0xec]);
    let value = 0xfedc_ba98_7654_3210u64;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 0x3c, &value.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::R12 as usize] = DATA_LINEAR + 0x40;
    state.gpr[Register::Rax as usize] = 0x10;
    state.gpr[Register::R10 as usize] = 0x0123_4567_89ab_cdef;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    let mut expected = before;
    expected.gpr[Register::R10 as usize] = value;
    expected.rip += 5;
    assert_eq!(state, expected);
    assert!(!writes.contains(&range(DATA_PHYSICAL + 0x3c)));
    assert!(writes.contains(&range(0x1000)));
    assert!(writes.contains(&range(0x2000)));
    assert!(writes.contains(&range(0x3010)));
    assert!(writes.contains(&range(0x4000)));
    assert!(writes.contains(&range(0x3020)));
    assert!(writes.contains(&range(0x5000)));
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    let data_pde = qword(&mut bus, ROOT + 0x2020);
    assert_ne!(data_pde & (1 << 5), 0);
    assert_eq!(data_pde & (1 << 6), 0, "read does not set dirty");
    let data_pte = qword(&mut bus, ROOT + 0x4000);
    assert_ne!(data_pte & (1 << 5), 0);
    assert_eq!(data_pte & (1 << 6), 0, "read does not set dirty");
}

#[test]
fn paged_probe_mov_r10_r12_rax_minus_twenty_fault_does_not_commit_destination() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x4d, 0x8b, 0x54, 0x04, 0xec]);
    let mut state = initial_state(0);
    state.gpr[Register::R12 as usize] = DATA_LINEAR + 0x1014;
    state.gpr[Register::Rax as usize] = 0;
    state.gpr[Register::R10 as usize] = 0x0123_4567_89ab_cdef;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert!(matches!(result.exit, PagedProbeExit::Fault(_)));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    let record = result.page_fault.as_ref().expect("unmapped data read #PF");
    assert_eq!(record.access(), AccessKind::Read);
    assert_eq!(record.saved_rip(), CODE_LINEAR);
    assert_eq!(record.fault().linear_address, Some(DATA_LINEAR + 0x1000));
}

#[test]
fn paged_probe_mov_rsp_disp8_rsi_write_protection_fault_is_atomic() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x48, 0x89, 0x74, 0x24, 0x10]);
    let original_value = 0xfeed_face_cafe_beefu64;
    put_bytes_at(
        &mut bus,
        DATA_PHYSICAL + 0x10,
        &original_value.to_le_bytes(),
    );
    // Present, readable and supervisor read-only with CR0.WP enabled.
    put(&mut bus, 0x5000, DATA_PHYSICAL | 5);
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = DATA_LINEAR;
    state.gpr[Register::Rsi as usize] = 0xaabb_ccdd_eeff_0011;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    let expected_fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x3),
        linear_address: Some(DATA_LINEAR + 0x10),
    };
    assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.memory_error, None);
    assert_eq!(state, before);
    let record = result.page_fault.as_ref().expect("write-protection fault");
    assert_eq!(record.fault(), &expected_fault);
    assert_eq!(record.access(), AccessKind::Write);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 0x10), original_value);
    let data_pte = qword(&mut bus, ROOT + 0x4000);
    assert_ne!(data_pte & (1 << 5), 0, "data translation is accessed");
    assert_eq!(data_pte & (1 << 6), 0, "faulting store does not set dirty");
    assert!(!writes.contains(&range(DATA_PHYSICAL + 0x10)));
}

#[test]
fn paged_probe_mov_rsp_disp8_rax_store_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x48, 0x89, 0x44, 0x24, 0x08]);
    let old_value = 0xfeed_face_cafe_beefu64;
    let value = 0x0123_4567_89ab_cdefu64;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 8, &old_value.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = DATA_LINEAR;
    state.gpr[Register::Rax as usize] = value;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    let mut expected = before;
    expected.rip += 5;
    assert_eq!(state, expected);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 8), value);
    assert!(writes.contains(&range(0x1000)));
    assert!(writes.contains(&range(0x2000)));
    assert!(writes.contains(&range(0x3010)));
    assert!(writes.contains(&range(0x4000)));
    assert!(writes.contains(&range(0x3020)));
    assert!(writes.contains(&range(0x5000)));
    assert!(writes.contains(&range(DATA_PHYSICAL + 8)));
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    let data_pde = qword(&mut bus, ROOT + 0x2020);
    assert_ne!(data_pde & (1 << 5), 0);
    assert_eq!(data_pde & (1 << 6), 0);
    let data_pte = qword(&mut bus, ROOT + 0x4000);
    assert_ne!(data_pte & (1 << 5), 0);
    assert_ne!(data_pte & (1 << 6), 0);
}

#[test]
fn paged_probe_mov_rsp_disp8_rax_write_protection_fault_is_atomic() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x48, 0x89, 0x44, 0x24, 0x08]);
    let original_value = 0xfeed_face_cafe_beefu64;
    put_bytes_at(&mut bus, DATA_PHYSICAL + 8, &original_value.to_le_bytes());
    // Present, readable and supervisor read-only with CR0.WP enabled.
    put(&mut bus, 0x5000, DATA_PHYSICAL | 5);
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = DATA_LINEAR;
    state.gpr[Register::Rax as usize] = 0xaabb_ccdd_eeff_0011;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    let expected_fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x3),
        linear_address: Some(DATA_LINEAR + 8),
    };
    assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.memory_error, None);
    assert_eq!(state, before);
    let record = result.page_fault.as_ref().expect("write-protection fault");
    assert_eq!(record.fault(), &expected_fault);
    assert_eq!(record.access(), AccessKind::Write);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL + 8), original_value);
    let data_pte = qword(&mut bus, ROOT + 0x4000);
    assert_ne!(data_pte & (1 << 5), 0, "data translation is accessed");
    assert_eq!(data_pte & (1 << 6), 0, "faulting store does not set dirty");
    assert!(!writes.contains(&range(DATA_PHYSICAL + 8)));
}

#[test]
fn paged_probe_mov_rsp_ebx_dword_store_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x89, 0x1c, 0x24]);
    let old_value = 0xfeed_face_cafe_beefu64;
    put_bytes_at(&mut bus, DATA_PHYSICAL, &old_value.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = DATA_LINEAR;
    state.gpr[Register::Rbx as usize] = 0xdead_beef_89ab_cdef;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    let mut expected = before;
    expected.rip += 3;
    assert_eq!(state, expected);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL), 0xfeed_face_89ab_cdef);
    assert!(writes.contains(&range(0x1000)));
    assert!(writes.contains(&range(0x2000)));
    assert!(writes.contains(&range(0x3010)));
    assert!(writes.contains(&range(0x4000)));
    assert!(writes.contains(&range(0x3020)));
    assert!(writes.contains(&range(0x5000)));
    assert!(
        writes.contains(
            &GuestPhysicalRange::new(GuestPhysicalAddress::new(DATA_PHYSICAL), 4).unwrap()
        )
    );
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    let data_pde = qword(&mut bus, ROOT + 0x2020);
    assert_ne!(data_pde & (1 << 5), 0);
    assert_eq!(data_pde & (1 << 6), 0);
    let data_pte = qword(&mut bus, ROOT + 0x4000);
    assert_ne!(data_pte & (1 << 5), 0);
    assert_ne!(data_pte & (1 << 6), 0);
}

#[test]
fn paged_probe_mov_word_ptr_rdi_ffff_uses_tracked_four_level_store() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x66, 0xc7, 0x07, 0xff, 0xff]);
    put_bytes_at(
        &mut bus,
        DATA_PHYSICAL + 4,
        &[0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6],
    );
    let mut state = initial_state(0);
    state.gpr[Register::Rdi as usize] = DATA_LINEAR + 6;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    let mut expected = before;
    expected.rip += 5;
    assert_eq!(state, expected);
    let bytes = (4..10)
        .map(|offset| {
            bus.read_physical(
                GuestPhysicalAddress::new(DATA_PHYSICAL + offset),
                AccessWidth::Byte,
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(bytes, [0xa1, 0xb2, 0xff, 0xff, 0xe5, 0xf6]);
    let store_range =
        GuestPhysicalRange::new(GuestPhysicalAddress::new(DATA_PHYSICAL + 6), 2).unwrap();
    assert!(writes.contains(&store_range));
    for address in [ROOT, ROOT + 0x1000, ROOT + 0x2020, ROOT + 0x4000] {
        assert_ne!(
            qword(&mut bus, address) & (1 << 5),
            0,
            "A bit missing at {address:#x}"
        );
    }
    let data_pde = qword(&mut bus, ROOT + 0x2020);
    assert_eq!(data_pde & (1 << 6), 0);
    let data_pte = qword(&mut bus, ROOT + 0x4000);
    assert_ne!(data_pte & (1 << 6), 0, "successful word store sets D");
}

#[test]
fn paged_probe_mov_word_ptr_rdi_ffff_write_protection_fault_is_atomic() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x66, 0xc7, 0x07, 0xff, 0xff]);
    put_bytes_at(
        &mut bus,
        DATA_PHYSICAL + 4,
        &[0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6],
    );
    put(&mut bus, 0x5000, DATA_PHYSICAL | 5); // supervisor read-only data page
    let mut state = initial_state(0);
    state.gpr[Register::Rdi as usize] = DATA_LINEAR + 6;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    let expected_fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x3),
        linear_address: Some(DATA_LINEAR + 6),
    };
    assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.memory_error, None);
    assert_eq!(state, before);
    let record = result.page_fault.as_ref().expect("word store WP fault");
    assert_eq!(record.fault(), &expected_fault);
    assert_eq!(record.saved_rip(), CODE_LINEAR);
    assert_eq!(record.access(), AccessKind::Write);
    let bytes = (4..10)
        .map(|offset| {
            bus.read_physical(
                GuestPhysicalAddress::new(DATA_PHYSICAL + offset),
                AccessWidth::Byte,
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(bytes, [0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6]);
    let data_pte = qword(&mut bus, ROOT + 0x4000);
    assert_ne!(data_pte & (1 << 5), 0, "faulting store sets A");
    assert_eq!(data_pte & (1 << 6), 0, "faulting store does not set D");
    let store_range =
        GuestPhysicalRange::new(GuestPhysicalAddress::new(DATA_PHYSICAL + 6), 2).unwrap();
    assert!(!writes.contains(&store_range));
}

#[test]
fn paged_probe_mov_rsp_ebx_dword_write_protection_fault_is_atomic() {
    let (mut bus, _) = fixture();
    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x89, 0x1c, 0x24]);
    let original_value = 0xfeed_face_cafe_beefu64;
    put_bytes_at(&mut bus, DATA_PHYSICAL, &original_value.to_le_bytes());
    // Present, readable and supervisor read-only with CR0.WP enabled.
    put(&mut bus, 0x5000, DATA_PHYSICAL | 5);
    let mut state = initial_state(0);
    state.gpr[Register::Rsp as usize] = DATA_LINEAR;
    state.gpr[Register::Rbx as usize] = 0xaabb_ccdd_89ab_cdef;
    let before = state.clone();
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    let expected_fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x3),
        linear_address: Some(DATA_LINEAR),
    };
    assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.memory_error, None);
    assert_eq!(state, before);
    let record = result.page_fault.as_ref().expect("write-protection fault");
    assert_eq!(record.fault(), &expected_fault);
    assert_eq!(record.access(), AccessKind::Write);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL), original_value);
    let data_pte = qword(&mut bus, ROOT + 0x4000);
    assert_ne!(data_pte & (1 << 5), 0, "data translation is accessed");
    assert_eq!(data_pte & (1 << 6), 0, "faulting store does not set dirty");
    assert!(
        !writes.contains(
            &GuestPhysicalRange::new(GuestPhysicalAddress::new(DATA_PHYSICAL), 4).unwrap()
        )
    );
}

#[test]
fn paged_probe_xor_r12d_sib_dword_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(
        &mut bus,
        CODE_PHYSICAL,
        &[0x46, 0x33, 0x24, 0x8d, 0x80, 0x26, 0x20, 0x00],
    );
    put_bytes_at(&mut bus, DATA_PHYSICAL, &0xa5a5_5a5au32.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::R9 as usize] = 0x17f660; // R9*4 + 0x202680 == DATA_LINEAR.
    state.gpr[Register::R12 as usize] = 0xdead_beef_0000_0001;
    let before_flags = state.rflags;
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state.rip, CODE_LINEAR + 8);
    assert_eq!(state.gpr[Register::R12 as usize], 0xa5a5_5a5b);
    assert_eq!(state.gpr[Register::R9 as usize], 0x17f660);
    assert_eq!(state.rflags & (1 << 7), 1 << 7); // Result bit 31 sets SF.
    assert_eq!(
        state.rflags & ((1 << 0) | (1 << 2) | (1 << 6) | (1 << 11)),
        0
    );
    assert_eq!(state.rflags & (1 << 4), before_flags & (1 << 4)); // AF is undefined, bit retained.
    assert_eq!(qword(&mut bus, DATA_PHYSICAL) & 0xffff_ffff, 0xa5a5_5a5a);
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x2020), ROOT + 0x4007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x3000), CODE_PHYSICAL | 7 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x4000), DATA_PHYSICAL | 7 | (1 << 5));
}

#[test]
fn paged_probe_xor_r9d_r14_sib_dword_uses_tracked_four_level_walk() {
    let (mut bus, _) = fixture();
    put_bytes_at(
        &mut bus,
        CODE_PHYSICAL,
        &[0x46, 0x33, 0x0c, 0xb5, 0x80, 0x26, 0x20, 0x00],
    );
    put_bytes_at(&mut bus, DATA_PHYSICAL, &0xa5a5_5a5au32.to_le_bytes());
    let mut state = initial_state(0);
    state.gpr[Register::R14 as usize] = 0x17f660; // R14*4 + 0x202680 == DATA_LINEAR.
    state.gpr[Register::R9 as usize] = 0xdead_beef_0000_0001;
    let before_flags = state.rflags;
    let mut writes = Vec::new();

    let result = run(&mut state, &mut bus, 1, || false, &mut writes);

    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.page_fault, None);
    assert_eq!(result.memory_error, None);
    assert_eq!(state.rip, CODE_LINEAR + 8);
    assert_eq!(state.gpr[Register::R9 as usize], 0xa5a5_5a5b);
    assert_eq!(state.gpr[Register::R14 as usize], 0x17f660);
    assert_eq!(state.rflags & (1 << 7), 1 << 7); // Result bit 31 sets SF.
    assert_eq!(
        state.rflags & ((1 << 0) | (1 << 2) | (1 << 6) | (1 << 11)),
        0
    );
    assert_eq!(state.rflags & (1 << 4), before_flags & (1 << 4)); // AF is undefined, bit retained.
    assert_eq!(qword(&mut bus, DATA_PHYSICAL) & 0xffff_ffff, 0xa5a5_5a5a);
    assert_eq!(qword(&mut bus, ROOT), ROOT + 0x1007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x1000), ROOT + 0x2007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x2020), ROOT + 0x4007 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x3000), CODE_PHYSICAL | 7 | (1 << 5));
    assert_eq!(qword(&mut bus, ROOT + 0x4000), DATA_PHYSICAL | 7 | (1 << 5));
}

#[test]
fn tracked_ram_backend_failure_is_distinct_from_guest_page_fault() {
    let (mut bus, _) = fixture();
    // This page is present, but its physical target is outside configured RAM.
    put(&mut bus, ROOT + 0x4000, 0x30_007);
    put_byte(&mut bus, CODE_PHYSICAL, 0x48);
    put_byte(&mut bus, CODE_PHYSICAL + 1, 0x8b);
    put_byte(&mut bus, CODE_PHYSICAL + 2, 0x03);
    let mut state = initial_state(0);
    let before = state.clone();
    let mut writes = Vec::new();
    let result = run(&mut state, &mut bus, 1, || false, &mut writes);
    assert!(matches!(result.exit, PagedProbeExit::Backend(_)));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert!(result.memory_error.is_some());
    assert!(result.page_fault.is_none());
}

#[test]
fn data_write_fault_after_successful_load_preserves_prefix_and_tracked_effects() {
    let (mut bus, _) = fixture();
    let payload = 0xfeed_face_cafe_be41;
    let original_data = 0x0123_4567_89ab_cdef;
    put_bytes_at(
        &mut bus,
        CODE_PHYSICAL,
        &[0x48, 0x8b, 0x03, 0x48, 0x89, 0x0b],
    );
    put(&mut bus, DATA_PHYSICAL, original_data);
    // Keep the data page present and readable, but reject supervisor writes
    // because CR0.WP is enabled in the fixture state.
    put(&mut bus, 0x5000, DATA_PHYSICAL | 5);

    let mut state = initial_state(payload);
    let before_flags = state.rflags;
    let mut callbacks = Vec::new();
    let result = run(&mut state, &mut bus, 2, || false, &mut callbacks);

    let expected_fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(0x3),
        linear_address: Some(DATA_LINEAR),
    };
    assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, FLAGS_DEFINED);
    assert_eq!(result.memory_error, None);
    assert_eq!(state.rip, CODE_LINEAR + 3);
    assert_eq!(state.rflags, before_flags);
    assert_eq!(state.gpr[Register::Rax as usize], original_data);
    assert_eq!(state.gpr[Register::Rcx as usize], payload);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL), original_data);

    let record = result.page_fault.as_ref().expect("precise data page fault");
    assert_eq!(record.fault(), &expected_fault);
    assert_eq!(record.saved_rip(), CODE_LINEAR + 3);
    assert_eq!(record.access(), AccessKind::Write);
    assert_eq!(record.context(), explicit_context());

    assert_eq!(
        callbacks,
        [
            range(0x1000),
            range(0x2000),
            range(0x3010),
            range(0x4000),
            range(0x3020),
            range(0x5000),
        ]
    );
    for address in [0x1000, 0x2000, 0x3010, 0x4000, 0x3020, 0x5000] {
        let entry = qword(&mut bus, address);
        assert_ne!(
            entry & (1 << 5),
            0,
            "prefix access must set A at {address:#x}"
        );
        assert_eq!(
            entry & (1 << 6),
            0,
            "faulting write must keep D clear at {address:#x}"
        );
    }
}

#[test]
fn halted_state_is_observed_after_poll_and_before_zero_budget_exit() {
    let (mut bus, _) = fixture();
    let mut state = initial_state(0);
    state.activity = Some(CpuActivity::Halted);
    let mut writes = Vec::new();
    let result = run(&mut state, &mut bus, 0, || false, &mut writes);
    assert_eq!(result.exit, PagedProbeExit::BudgetExhausted);
    let result = run(&mut state, &mut bus, 1, || false, &mut writes);
    assert_eq!(result.exit, PagedProbeExit::Halted);
    assert_eq!(result.instructions_executed, 0);
}

#[test]
fn next_page_fetch_fault_preserves_committed_store_for_not_present_and_nx() {
    for (next_pte, expected_pfec) in [(0, 0x10), (0xa000 | 7 | (1 << 63), 0x11)] {
        let (mut bus, _) = fixture();
        let payload = 0x0102_0304_0506_0741;
        let store_rip = CODE_LINEAR + 0xffd;
        let next_rip = CODE_LINEAR + 0x1000;
        put_bytes_at(&mut bus, CODE_PHYSICAL + 0xffd, &[0x48, 0x89, 0x0b]);
        map_next_code_page(&mut bus, next_pte);
        let mut state = initial_state(payload);
        state.rip = store_rip;
        let before_flags = state.rflags;
        let mut callbacks = Vec::new();

        let result = run(&mut state, &mut bus, 2, || false, &mut callbacks);

        let expected_fault = ArchitecturalFault {
            vector: 14,
            error_code: Some(expected_pfec),
            linear_address: Some(next_rip),
        };
        assert_eq!(result.exit, PagedProbeExit::Fault(expected_fault.clone()));
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, FLAGS_DEFINED);
        assert_eq!(state.rip, next_rip);
        assert_eq!(state.rflags, before_flags);
        assert_eq!(qword(&mut bus, DATA_PHYSICAL), payload);

        let record = result.page_fault.as_ref().expect("precise page fault");
        assert_eq!(record.fault(), &expected_fault);
        assert_eq!(record.saved_rip(), next_rip);
        assert_eq!(record.access(), AccessKind::Fetch);
        assert_eq!(record.context(), explicit_context());
        assert_eq!(
            callbacks[..8],
            [
                range(0x1000),
                range(0x2000),
                range(0x3010),
                range(0x4000),
                range(0x3020),
                range(0x5000),
                range(0x5000),
                range(DATA_PHYSICAL),
            ]
        );
        assert_eq!(callbacks.len(), if expected_pfec == 0x11 { 9 } else { 8 });
        if expected_pfec == 0x11 {
            assert_eq!(callbacks[8], range(0x4008));
        }
    }
}

#[test]
fn quantum_reports_exact_code_and_store_ad_callbacks_and_keeps_them_after_pio_error() {
    let (mut bus, _) = fixture();
    let payload = 0xfeed_face_cafe_be41;
    let mut state = initial_state(payload);
    let mut callbacks = Vec::new();

    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0xee]);
    state.gpr[Register::Rdx as usize] = 0x1234;
    let code_fault = run(&mut state, &mut bus, 1, || false, &mut callbacks);
    assert!(matches!(code_fault.exit, PagedProbeExit::PortIo(_)));
    assert_eq!(code_fault.instructions_executed, 0);
    assert_eq!(
        callbacks,
        [range(0x1000), range(0x2000), range(0x3010), range(0x4000)]
    );
    for address in [0x1000, 0x2000, 0x3010, 0x4000] {
        let entry = qword(&mut bus, address);
        assert_ne!(entry & (1 << 5), 0, "code fetch must set A at {address:#x}");
        assert_eq!(
            entry & (1 << 6),
            0,
            "code fetch must keep D clear at {address:#x}"
        );
    }

    put_bytes_at(&mut bus, CODE_PHYSICAL, &[0x48, 0x89, 0x0b, 0xee]);
    state.rip = CODE_LINEAR;
    state.gpr[Register::Rdx as usize] = u64::from(COM1);
    let store = run(&mut state, &mut bus, 1, || false, &mut callbacks);
    assert_eq!(store.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(store.instructions_executed, 1);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL), payload);
    assert_eq!(
        callbacks,
        [
            range(0x1000),
            range(0x2000),
            range(0x3010),
            range(0x4000),
            range(0x3020),
            range(0x5000),
            range(0x5000),
            range(DATA_PHYSICAL),
        ]
    );
    for (address, dirty) in [(0x3020, false), (0x5000, true)] {
        let entry = qword(&mut bus, address);
        assert_ne!(entry & (1 << 5), 0, "store must set A at {address:#x}");
        assert_eq!(entry & (1 << 6) != 0, dirty, "unexpected D at {address:#x}");
    }

    state.gpr[Register::Rdx as usize] = 0x1234;
    let before_callbacks = callbacks.clone();
    let pio_fault = run(&mut state, &mut bus, 1, || false, &mut callbacks);
    assert!(matches!(pio_fault.exit, PagedProbeExit::PortIo(_)));
    assert_eq!(pio_fault.instructions_executed, 0);
    assert_eq!(state.rip, CODE_LINEAR + 3);
    assert_eq!(callbacks, before_callbacks);
}

#[test]
fn stop_after_store_and_out_resume_on_same_cpu_and_bus_without_repeating_effects() {
    let payload = 0xaabb_ccdd_eeff_0041;
    let (mut bus, handle) = fixture();
    let mut state = initial_state(payload);
    let mut callbacks = Vec::new();
    let mut polls = 0;
    let stopped_after_store = run(
        &mut state,
        &mut bus,
        4,
        || {
            polls += 1;
            polls == 2
        },
        &mut callbacks,
    );
    assert_eq!(stopped_after_store.exit, PagedProbeExit::Stopped);
    assert_eq!(stopped_after_store.instructions_executed, 1);
    assert_eq!(state.rip, CODE_LINEAR + 3);
    let store_callbacks = callbacks.clone();

    let resumed = run(&mut state, &mut bus, 3, || false, &mut callbacks);
    assert_eq!(resumed.exit, PagedProbeExit::Halted);
    assert_eq!(resumed.instructions_executed, 3);
    assert_eq!(
        stopped_after_store.instructions_executed + resumed.instructions_executed,
        4
    );
    assert_eq!(state.rip, CODE_LINEAR + 8);
    assert_eq!(state.activity, Some(CpuActivity::Halted));
    assert_eq!(qword(&mut bus, DATA_PHYSICAL), payload);
    assert_eq!(
        callbacks
            .iter()
            .filter(|item| **item == range(DATA_PHYSICAL))
            .count(),
        1
    );
    assert_eq!(&callbacks[..store_callbacks.len()], &store_callbacks[..]);
    assert!(handle.transfer_holding_to_shift());
    assert_eq!(handle.complete_transmission(), Some(payload as u8));

    let (mut bus, handle) = fixture();
    let mut state = initial_state(payload);
    let mut callbacks = Vec::new();
    let mut polls = 0;
    let stopped_after_out = run(
        &mut state,
        &mut bus,
        4,
        || {
            polls += 1;
            polls == 4
        },
        &mut callbacks,
    );
    assert_eq!(stopped_after_out.exit, PagedProbeExit::Stopped);
    assert_eq!(stopped_after_out.instructions_executed, 3);
    assert_eq!(state.rip, CODE_LINEAR + 7);
    let callbacks_after_out = callbacks.clone();

    let resumed = run(&mut state, &mut bus, 1, || false, &mut callbacks);
    assert_eq!(resumed.exit, PagedProbeExit::Halted);
    assert_eq!(resumed.instructions_executed, 1);
    assert_eq!(
        stopped_after_out.instructions_executed + resumed.instructions_executed,
        4
    );
    assert_eq!(callbacks, callbacks_after_out);
    assert!(handle.transfer_holding_to_shift());
    assert_eq!(handle.complete_transmission(), Some(payload as u8));
    assert!(!handle.transfer_holding_to_shift());
}

#[test]
fn cr0_wp_clear_applies_before_store_and_matches_split_quantum() {
    let payload = 0x8877_6655_4433_2211;
    let mut whole_bus = cr0_wp_fixture();
    let mut whole_state = initial_cr0_wp_state(CR0_WP_CLEAR, payload);
    let old_flags = whole_state.rflags;
    let mut whole_writes = Vec::new();
    let whole = run_paged_cr0_wp_probe(
        &mut whole_state,
        &mut whole_bus,
        EFER,
        context(),
        2,
        FLAGS_DEFINED,
        || false,
        |write| whole_writes.push(write),
    );
    assert_eq!(whole.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(whole.instructions_executed, 2);
    assert_eq!(whole_state.cr0, Some(CR0_WP_CLEAR));
    assert_eq!(whole_state.rip, CODE_LINEAR + 6);
    assert_eq!(whole_state.rflags, old_flags);
    assert_eq!(whole.rflags_defined, FLAGS_DEFINED & !0x8d5);
    assert_eq!(qword(&mut whole_bus, DATA_PHYSICAL), payload);
    assert!(whole_writes.contains(&range(DATA_PHYSICAL)));
    assert_eq!(
        whole.translation_context_changes,
        [od_isa::TranslationContextChange::Cr0Write {
            old_cr0: CR0,
            new_cr0: CR0_WP_CLEAR,
        }]
    );

    let mut split_bus = cr0_wp_fixture();
    let mut split_state = initial_cr0_wp_state(CR0_WP_CLEAR, payload);
    let mut split_writes = Vec::new();
    let first = run_paged_cr0_wp_probe(
        &mut split_state,
        &mut split_bus,
        EFER,
        context(),
        1,
        FLAGS_DEFINED,
        || false,
        |write| split_writes.push(write),
    );
    assert_eq!(first.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(first.instructions_executed, 1);
    assert_eq!(split_state.rip, CODE_LINEAR + 3);
    assert_eq!(first.translation_context_changes.len(), 1);

    let second = run_paged_cr0_wp_probe(
        &mut split_state,
        &mut split_bus,
        EFER,
        context(),
        1,
        first.rflags_defined,
        || false,
        |write| split_writes.push(write),
    );
    assert_eq!(second.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(second.instructions_executed, 1);
    assert!(second.translation_context_changes.is_empty());
    assert_eq!(split_state, whole_state);
    assert_eq!(qword(&mut split_bus, DATA_PHYSICAL), payload);
    assert_eq!(split_writes, whole_writes);
}

#[test]
fn cr0_wp_set_faults_readonly_store_without_data_or_dirty_commit() {
    let original = 0x0123_4567_89ab_cdef;
    let payload = 0xfedc_ba98_7654_3210;
    let mut bus = cr0_wp_fixture();
    put(&mut bus, DATA_PHYSICAL, original);
    let mut state = initial_cr0_wp_state(CR0, payload);
    state.cr0 = Some(CR0_WP_CLEAR);
    let mut writes = Vec::new();

    let result = run_paged_cr0_wp_probe(
        &mut state,
        &mut bus,
        EFER,
        context(),
        2,
        FLAGS_DEFINED,
        || false,
        |write| writes.push(write),
    );
    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(3),
            linear_address: Some(DATA_LINEAR),
        })
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.cr0, Some(CR0));
    assert_eq!(state.rip, CODE_LINEAR + 3);
    assert_eq!(
        result.translation_context_changes,
        [od_isa::TranslationContextChange::Cr0Write {
            old_cr0: CR0_WP_CLEAR,
            new_cr0: CR0,
        }]
    );
    assert_eq!(qword(&mut bus, DATA_PHYSICAL), original);
    let data_pte = qword(&mut bus, 0x5000);
    assert_eq!(data_pte & (1 << 6), 0, "faulted store must not set D");
    assert!(!writes.contains(&range(DATA_PHYSICAL)));
}

#[test]
fn stop_after_mov_cr0_precedes_operand_walk_and_profile_rejects_invalid_state() {
    let mut bus = cr0_wp_fixture();
    let mut state = initial_cr0_wp_state(CR0_WP_CLEAR, 0xfeed_face_cafe_beef);
    let mut polls = 0;
    let stopped = run_paged_cr0_wp_probe(
        &mut state,
        &mut bus,
        EFER,
        context(),
        2,
        FLAGS_DEFINED,
        || {
            polls += 1;
            polls == 2
        },
        |_| {},
    );
    assert_eq!(stopped.exit, PagedProbeExit::Stopped);
    assert_eq!(stopped.instructions_executed, 1);
    assert_eq!(state.cr0, Some(CR0_WP_CLEAR));
    assert_eq!(state.rip, CODE_LINEAR + 3);
    let data_pte = qword(&mut bus, 0x5000);
    assert_eq!(data_pte & ((1 << 5) | (1 << 6)), 0);

    state.cr0 = Some(CR0 | (1 << 6));
    let invalid_before = state.clone();
    let rejected = run_paged_cr0_wp_probe(
        &mut state,
        &mut bus,
        EFER,
        context(),
        1,
        FLAGS_DEFINED,
        || false,
        |_| {},
    );
    assert!(matches!(rejected.exit, PagedProbeExit::Rejected(_)));
    assert_eq!(rejected.instructions_executed, 0);
    assert_eq!(state, invalid_before);

    state.cr0 = Some(CR0);
    state.cr3 = None;
    let missing_cr3 = run_paged_cr0_wp_probe(
        &mut state,
        &mut bus,
        EFER,
        context(),
        1,
        FLAGS_DEFINED,
        || false,
        |_| {},
    );
    assert_eq!(
        missing_cr3.exit,
        PagedProbeExit::Rejected(PagedProbeRejection::CpuCr3Required)
    );
    assert_eq!(missing_cr3.instructions_executed, 0);
}

#[test]
fn cr0_wp_profile_rejects_qword_store_crossing_a_guest_page() {
    let mut bus = cr0_wp_fixture();
    let mut state = initial_cr0_wp_state(CR0_WP_CLEAR, 0xfeed_face_cafe_beef);
    state.gpr[Register::Rbx as usize] = DATA_LINEAR + 0xff9;
    let original = qword(&mut bus, DATA_PHYSICAL);
    let mut writes = Vec::new();

    let result = run_paged_cr0_wp_probe(
        &mut state,
        &mut bus,
        EFER,
        context(),
        2,
        FLAGS_DEFINED,
        || false,
        |write| writes.push(write),
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.exit, PagedProbeExit::Unsupported { opcode: 0x89 });
    assert!(result.page_fault.is_none());
    assert!(result.memory_error.is_none());
    assert_eq!(qword(&mut bus, DATA_PHYSICAL), original);
    assert!(!writes.contains(&range(DATA_PHYSICAL)));
}

const MIXED_ROOT_B: u64 = 0x11_000;
const MIXED_PD_B: u64 = 0x13_000;
const MIXED_DATA_OFFSET: u64 = 0x17_000;

/// Root A: 4 KiB mappings. Root B: 4 KiB code page, but the whole data region
/// (PD index 4) is one 2 MiB leaf at physical 0. Guest program:
/// A: MOV CR3,RAX -> B: MOV RAX,[RBX] (loads root A from the 2 MiB page) ->
/// B: MOV CR3,RAX -> A.
fn mixed_cr3_fixture() -> (MachineBus, LongModeState) {
    let mut bus = cr3_switch_fixture();
    put(&mut bus, MIXED_PD_B + 0x20, 0x87);
    put(&mut bus, MIXED_DATA_OFFSET, ROOT);
    put_bytes_at(&mut bus, 0x16_000 + 6, &[0x0f, 0x22, 0xd8]);
    let mut state = initial_cr3_state();
    state.gpr[Register::Rbx as usize] = DATA_LINEAR + MIXED_DATA_OFFSET;
    (bus, state)
}

fn run_mixed(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    budget: u64,
    defined: u64,
    writes: &mut Vec<GuestPhysicalRange>,
) -> od_machine::PagedProbeQuantumReport {
    run_paged_cr3_probe_2m_ram(
        state,
        bus,
        EFER,
        context(),
        budget,
        defined,
        || false,
        |range| writes.push(range),
    )
}

#[test]
fn mixed_cr3_roots_whole_and_split_quantum_match() {
    let run_all = |split: bool| {
        let (mut bus, mut state) = mixed_cr3_fixture();
        let (mut writes, mut changes) = (Vec::new(), Vec::new());
        let (mut defined, mut retired) = (FLAGS_DEFINED, 0);
        loop {
            let report = run_mixed(
                &mut state,
                &mut bus,
                if split { 1 } else { 3 },
                defined,
                &mut writes,
            );
            assert_eq!(report.exit, PagedProbeExit::BudgetExhausted);
            retired += report.instructions_executed;
            defined = report.rflags_defined;
            changes.extend(report.translation_context_changes);
            if retired == 3 {
                break;
            }
        }
        (bus, state, defined, writes, changes)
    };
    let (mut whole_bus, whole, whole_defined, whole_writes, whole_changes) = run_all(false);
    let (mut split_bus, split, split_defined, split_writes, split_changes) = run_all(true);
    assert_eq!(whole, split);
    assert_eq!(whole_defined, split_defined);
    assert_eq!(whole_writes, split_writes);
    assert_eq!(whole_changes, split_changes);
    assert_eq!(
        whole_changes,
        [
            od_isa::execute::TranslationContextChange::Cr3Write {
                old_cr3: ROOT,
                new_cr3: MIXED_ROOT_B,
            },
            od_isa::execute::TranslationContextChange::Cr3Write {
                old_cr3: MIXED_ROOT_B,
                new_cr3: ROOT,
            },
        ]
    );
    assert_eq!(whole.cr3, Some(ROOT));
    assert_eq!(whole.gpr[Register::Rax as usize], ROOT);
    assert_eq!(whole.rip, CODE_LINEAR + 9);
    // Reading through the 2 MiB data PDE sets only its accessed bit (no stores, so no dirty bit).
    assert_eq!(qword(&mut whole_bus, MIXED_PD_B + 0x20) & 0x60, 0x20);
    for address in (0..RAM_SIZE).step_by(8) {
        assert_eq!(
            qword(&mut whole_bus, address),
            qword(&mut split_bus, address),
            "{address:#x}"
        );
    }
}

#[test]
fn mixed_cr3_quantum_keeps_legacy_quantum_rejecting_two_mib_pages() {
    let (mut bus, mut state) = mixed_cr3_fixture();
    let report = run_paged_cr3_probe(
        &mut state,
        &mut bus,
        EFER,
        context(),
        3,
        FLAGS_DEFINED,
        || false,
        |_| {},
    );
    assert!(matches!(report.exit, PagedProbeExit::Backend(_)));
    assert!(matches!(
        report.memory_error,
        Some(od_machine::TrackedAccessError::UnsupportedPageSize { .. })
    ));
    assert_eq!(report.instructions_executed, 1);
    assert_eq!(state.cr3, Some(MIXED_ROOT_B));
}

#[test]
fn mixed_cr3_large_leaf_faults_are_precise_after_the_root_switch() {
    // (PDE, expected #PF error code): not present; reserved bit 13 of a 2 MiB PDE.
    for (pde, error_code) in [(0u64, 0u32), (0x87 | (1 << 13), 9)] {
        let (mut bus, mut state) = mixed_cr3_fixture();
        put(&mut bus, MIXED_PD_B + 0x20, pde);
        let mut writes = Vec::new();
        let report = run_mixed(&mut state, &mut bus, 3, FLAGS_DEFINED, &mut writes);
        let PagedProbeExit::Fault(fault) = &report.exit else {
            panic!("pde {pde:#x}: {:?}", report.exit)
        };
        assert_eq!(fault.vector, 14, "pde {pde:#x}");
        assert_eq!(fault.error_code, Some(error_code), "pde {pde:#x}");
        assert_eq!(fault.linear_address, Some(DATA_LINEAR + MIXED_DATA_OFFSET));
        assert_eq!(report.instructions_executed, 1);
        assert_eq!(state.cr3, Some(MIXED_ROOT_B));
        assert_eq!(state.rip, CODE_LINEAR + 3);
        assert_eq!(report.translation_context_changes.len(), 1);
        assert!(!writes.is_empty());
    }

    // Data reads ignore NX on the PDE, so the whole program still completes.
    let (mut bus, mut state) = mixed_cr3_fixture();
    put(&mut bus, MIXED_PD_B + 0x20, 0x87 | (1 << 63));
    let mut writes = Vec::new();
    let report = run_mixed(&mut state, &mut bus, 3, FLAGS_DEFINED, &mut writes);
    assert_eq!(report.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(report.instructions_executed, 3);
}

#[test]
fn mixed_cr3_quantum_never_walks_the_next_code_page_and_honours_stop_and_budget() {
    // An RIP near the page end is Unsupported before any fetch; the next page's PTE
    // is never walked (its accessed bit stays clear) and no guest fault is raised.
    for offset in [0xff2u64, 0xfff] {
        let (mut bus, mut state) = mixed_cr3_fixture();
        map_next_code_page(&mut bus, 0x8000 | 7);
        put_byte(&mut bus, CODE_PHYSICAL + offset, 0x0f);
        put_byte(&mut bus, 0x8000, 0x22);
        state.rip = CODE_LINEAR + offset;
        let before = state.clone();
        let mut writes = Vec::new();
        let report = run_mixed(&mut state, &mut bus, 1, FLAGS_DEFINED, &mut writes);
        assert!(
            matches!(report.exit, PagedProbeExit::Unsupported { .. }),
            "{offset:#x}: {:?}",
            report.exit
        );
        assert!(report.page_fault.is_none());
        assert_eq!(report.instructions_executed, 0);
        assert_eq!(qword(&mut bus, 0x4008) & 0x20, 0);
        assert!(!writes.contains(&range(0x4008)));
        assert_eq!(state, before);
    }

    // A qword operand crossing a 4 KiB boundary inside a 2 MiB leaf is a backend
    // CrossesPage error (retired MOV CR3 stays retired).
    let (mut bus, mut state) = mixed_cr3_fixture();
    state.gpr[Register::Rbx as usize] = DATA_LINEAR + MIXED_DATA_OFFSET + 0xffc;
    let mut writes = Vec::new();
    let report = run_mixed(&mut state, &mut bus, 3, FLAGS_DEFINED, &mut writes);
    assert!(matches!(report.exit, PagedProbeExit::Backend(_)));
    assert!(matches!(
        report.memory_error,
        Some(od_machine::TrackedAccessError::CrossesPage { .. })
    ));
    assert_eq!(report.instructions_executed, 1);
    assert_eq!(state.cr3, Some(MIXED_ROOT_B));

    // STOP and a zero budget are honoured before any fetch.
    for stop in [true, false] {
        let (mut bus, mut state) = mixed_cr3_fixture();
        let before = state.clone();
        let mut writes = Vec::new();
        let report = run_paged_cr3_probe_2m_ram(
            &mut state,
            &mut bus,
            EFER,
            context(),
            0,
            FLAGS_DEFINED,
            || stop,
            |range| writes.push(range),
        );
        assert_eq!(
            report.exit,
            if stop {
                PagedProbeExit::Stopped
            } else {
                PagedProbeExit::BudgetExhausted
            }
        );
        assert_eq!(state, before);
        assert!(writes.is_empty());
    }
}

/// Like `mixed_cr3_fixture`, but root B's code region is a 2 MiB leaf too (PD index 2, base 0),
/// so the instructions under root B live at physical offsets 3 and 6 of that leaf.
fn mixed_cr3_large_code_fixture() -> (MachineBus, LongModeState) {
    let (mut bus, state) = mixed_cr3_fixture();
    put(&mut bus, MIXED_PD_B + 0x10, 0x87);
    put_bytes_at(&mut bus, 3, &[0x48, 0x8b, 0x03, 0x0f, 0x22, 0xd8]);
    (bus, state)
}

#[test]
fn mixed_cr3_code_in_a_two_mib_leaf_runs_whole_and_split() {
    let run_all = |split: bool| {
        let (mut bus, mut state) = mixed_cr3_large_code_fixture();
        let (mut writes, mut changes) = (Vec::new(), Vec::new());
        let (mut defined, mut retired) = (FLAGS_DEFINED, 0);
        while retired < 3 {
            let before = writes.len();
            let report = run_mixed(
                &mut state,
                &mut bus,
                if split { 1 } else { 3 },
                defined,
                &mut writes,
            );
            assert_eq!(report.exit, PagedProbeExit::BudgetExhausted);
            if split && retired == 0 {
                // The context change is consumed before any walk under root B.
                assert_eq!(report.translation_context_changes.len(), 1);
                assert!(!writes[before..].contains(&range(MIXED_PD_B + 0x10)));
            }
            retired += report.instructions_executed;
            defined = report.rflags_defined;
            changes.extend(report.translation_context_changes);
        }
        (bus, state, defined, writes, changes)
    };
    let (mut whole_bus, whole, whole_defined, whole_writes, whole_changes) = run_all(false);
    let (mut split_bus, split, split_defined, split_writes, split_changes) = run_all(true);
    assert_eq!(whole, split);
    assert_eq!(whole_defined, split_defined);
    assert_eq!(whole_writes, split_writes);
    assert_eq!(whole_changes, split_changes);
    assert_eq!(whole.cr3, Some(ROOT));
    assert_eq!(whole.rip, CODE_LINEAR + 9);
    // Fetching through the code PDE sets its accessed bit; there are no stores.
    assert_eq!(qword(&mut whole_bus, MIXED_PD_B + 0x10) & 0x60, 0x20);
    assert!(whole_writes.contains(&range(MIXED_PD_B + 0x10)));
    for address in (0..RAM_SIZE).step_by(8) {
        assert_eq!(
            qword(&mut whole_bus, address),
            qword(&mut split_bus, address),
            "{address:#x}"
        );
    }
}

#[test]
fn mixed_cr3_two_mib_code_leaf_faults_and_page_end_guard() {
    // NX on the code PDE: the fetch after the retired CR3 write raises #PF(P|I).
    let (mut bus, mut state) = mixed_cr3_large_code_fixture();
    put(&mut bus, MIXED_PD_B + 0x10, 0x87 | (1 << 63));
    let mut writes = Vec::new();
    let report = run_mixed(&mut state, &mut bus, 3, FLAGS_DEFINED, &mut writes);
    let PagedProbeExit::Fault(fault) = &report.exit else {
        panic!("{:?}", report.exit)
    };
    assert_eq!(fault.vector, 14);
    assert_eq!(fault.error_code, Some(0x11));
    assert_eq!(fault.linear_address, Some(CODE_LINEAR + 3));
    assert_eq!(report.instructions_executed, 1);
    assert_eq!(state.cr3, Some(MIXED_ROOT_B));
    assert_eq!(report.translation_context_changes.len(), 1);

    // Near the end of a 4 KiB page inside the 2 MiB leaf the CPU profile refuses to fetch,
    // so decode never walks into the next 4 KiB page of the leaf.
    for offset in [0xff2u64, 0xfff] {
        let (mut bus, mut state) = mixed_cr3_large_code_fixture();
        state.cr3 = Some(MIXED_ROOT_B);
        state.rip = CODE_LINEAR + offset;
        let before = state.clone();
        let mut writes = Vec::new();
        let report = run_mixed(&mut state, &mut bus, 1, FLAGS_DEFINED, &mut writes);
        assert!(
            matches!(report.exit, PagedProbeExit::Unsupported { .. }),
            "{offset:#x}: {:?}",
            report.exit
        );
        assert_eq!(report.instructions_executed, 0);
        assert!(writes.is_empty(), "{offset:#x}: no walk before the guard");
        assert_eq!(state, before);
    }
}

#[test]
fn mixed_cr3_table_pointer_outside_ram_and_same_root_write() {
    // A present table pointer outside RAM under root B is a backend error after the retired
    // MOV CR3 and the walk's committed A updates.
    let (mut bus, mut state) = mixed_cr3_fixture();
    put(&mut bus, MIXED_PD_B + 0x20, 0x4000_0007);
    let mut writes = Vec::new();
    let report = run_mixed(&mut state, &mut bus, 3, FLAGS_DEFINED, &mut writes);
    assert!(matches!(report.exit, PagedProbeExit::Backend(_)));
    assert!(matches!(
        report.memory_error,
        Some(od_machine::TrackedAccessError::TableRead { .. })
    ));
    assert_eq!(report.instructions_executed, 1);
    assert_eq!(state.cr3, Some(MIXED_ROOT_B));
    assert_eq!(report.translation_context_changes.len(), 1);
    assert!(!writes.is_empty());

    // Writing the current root back still reports a context change.
    let (mut bus, mut state) = mixed_cr3_fixture();
    state.gpr[Register::Rax as usize] = ROOT;
    let mut writes = Vec::new();
    let report = run_mixed(&mut state, &mut bus, 1, FLAGS_DEFINED, &mut writes);
    assert_eq!(report.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(
        report.translation_context_changes,
        [od_isa::execute::TranslationContextChange::Cr3Write {
            old_cr3: ROOT,
            new_cr3: ROOT,
        }]
    );
}

#[test]
fn paged_probe_mov_immediate_fetch_fault_on_immediate_commits_nothing() {
    let (mut bus, _) = fixture();
    // `66 BA` occupies the last two bytes of the mapped code page; the
    // immediate bytes live on the next page, which is not mapped.
    put_bytes_at(&mut bus, CODE_PHYSICAL + 0xffe, &[0x66, 0xba]);
    map_next_code_page(&mut bus, 0);
    let mut state = initial_state(0);
    state.rip = CODE_LINEAR + 0xffe;
    let before = state.clone();
    let mut callbacks = Vec::new();

    let result = run(&mut state, &mut bus, 2, || false, &mut callbacks);

    let next_page = CODE_LINEAR + 0x1000;
    assert_eq!(
        result.exit,
        PagedProbeExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(0x10),
            linear_address: Some(next_page),
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state.gpr[Register::Rdx as usize], u64::from(COM1));
    assert_eq!(state.rip, CODE_LINEAR + 0xffe);
    assert_eq!(state, before);
    assert!(result.page_fault.is_some());
}
