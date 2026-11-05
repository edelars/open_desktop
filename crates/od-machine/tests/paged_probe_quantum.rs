use od_isa::memory::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, InstructionMode, PrivilegeLevel,
};
use od_isa::{CpuActivity, INTEGER_FLAGS, LongModeState, Register};
use od_machine::{
    AccessWidth, GuestPhysicalAddress, GuestPhysicalRange, MachineBus, MachineMemoryMap,
    MemoryRegionKind, MemoryRegionSpec, PagedProbeExit, PagedProbeRejection, PortAddress,
    PortRange, PortRegionBinding, RamLimits, Uart16550Handle, Uart16550PollingTx, run_paged_probe,
};

const CR0: u64 = 0x8001_0033;
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
