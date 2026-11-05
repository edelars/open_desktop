use od_isa::execute::{PioU8Exit, RunExit};
use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};
use od_isa::{INTEGER_FLAGS, LongModeState, Register};
use od_machine::{
    AccessWidth, BusAccessError, BusDevice, DeviceAccess, DeviceError, GuestPhysicalAddress,
    GuestPhysicalRange, MachineBus, MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec,
    PortAddress, PortRange, PortRegionBinding, RamLimits, Uart16550Handle, Uart16550PollingTx,
    step_paged_pio_u8,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

const CR0: u64 = 0x8001_0033;
const CR4: u64 = 1 << 5;
const EFER: u64 = (1 << 8) | (1 << 10) | (1 << 11);
const ROOT_A: u64 = 0x1000;
const ROOT_B: u64 = 0x16000;
const CODE_LINEAR: u64 = 0x0040_0000;
const CODE_PHYSICAL_A: u64 = 0x7000;
const CODE_PHYSICAL_B: u64 = 0x8000;
const DATA_LINEAR: u64 = 0x0080_0000;
const DATA_PHYSICAL: u64 = 0x9000;
const COM1: u16 = 0x3f8;
const RAM_SIZE: u64 = 0x20_000;
const FLAGS_DEFINED: u64 = INTEGER_FLAGS & !(1 << 4);

struct CountingUart {
    uart: Uart16550PollingTx,
    accesses: Arc<AtomicUsize>,
}

impl BusDevice for CountingUart {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        self.accesses.fetch_add(1, Ordering::SeqCst);
        self.uart.read(access)
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        self.accesses.fetch_add(1, Ordering::SeqCst);
        self.uart.write(access, value)
    }
}

fn make_bus() -> (MachineBus, Uart16550Handle, Arc<AtomicUsize>) {
    let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM_SIZE).unwrap();
    let map =
        MachineMemoryMap::new(36, vec![MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)]).unwrap();
    let (uart, handle) = Uart16550PollingTx::new_with_handle();
    let accesses = Arc::new(AtomicUsize::new(0));
    let bus = MachineBus::new(
        map,
        RamLimits::new(RAM_SIZE),
        vec![Box::new(CountingUart {
            uart,
            accesses: Arc::clone(&accesses),
        })],
        vec![],
        vec![PortRegionBinding {
            range: PortRange::new(PortAddress::new(COM1), 8).unwrap(),
            device_index: 0,
        }],
    )
    .unwrap();
    (bus, handle, accesses)
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

fn put_bytes(bus: &mut MachineBus, address: u64, bytes: &[u8]) {
    for (offset, byte) in bytes.iter().copied().enumerate() {
        bus.write_physical(
            GuestPhysicalAddress::new(address + offset as u64),
            AccessWidth::Byte,
            u64::from(byte),
            |_| {},
        )
        .unwrap();
    }
}

fn map_code(bus: &mut MachineBus, root: u64, pte: u64) {
    put(bus, root, root + 0x1007);
    put(bus, root + 0x1000, root + 0x2007);
    put(bus, root + 0x2010, root + 0x3007); // PD index 2 for 4 MiB.
    put(bus, root + 0x3000, pte); // PT index 0.
}

fn map_data(bus: &mut MachineBus, root: u64, physical: u64) {
    put(bus, root + 0x2020, root + 0x4007); // PD index 4.
    put(bus, root + 0x4000, physical | 7); // PT index 0.
}

fn setup() -> (MachineBus, Uart16550Handle, Arc<AtomicUsize>) {
    let (mut bus, handle, accesses) = make_bus();
    map_code(&mut bus, ROOT_A, CODE_PHYSICAL_A | 7);
    (bus, handle, accesses)
}

fn context(privilege: PrivilegeLevel) -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege,
        origin: AccessOrigin::DescriptorTable,
    }
}

fn cpu(rip: u64) -> LongModeState {
    LongModeState {
        rip,
        cr0: Some(CR0),
        cr4: Some(CR4),
        ..LongModeState::default()
    }
}

fn step(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    root: u64,
) -> od_machine::PagedPioU8StepReport {
    step_with_efer(state, bus, root, EFER)
}

fn step_with_efer(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    root: u64,
    efer: u64,
) -> od_machine::PagedPioU8StepReport {
    step_paged_pio_u8(
        state,
        bus,
        root,
        efer,
        context(PrivilegeLevel::Ring0),
        FLAGS_DEFINED,
        |_| {},
    )
    .unwrap()
}

fn configure_uart(bus: &mut MachineBus) {
    for (offset, value) in [(3, 0x83), (0, 0x34), (1, 0x12), (3, 0x03)] {
        bus.write_port(PortAddress::new(COM1 + offset), AccessWidth::Byte, value)
            .unwrap();
    }
}

fn qword(bus: &mut MachineBus, address: u64) -> u64 {
    bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
        .unwrap()
}

fn range(address: u64) -> GuestPhysicalRange {
    GuestPhysicalRange::new(GuestPhysicalAddress::new(address), 8).unwrap()
}

#[test]
fn paged_mov_then_pio_uses_loaded_byte_and_preserves_boundary_state() {
    for data in [0xfeed_face_cafe_be41, 0x0123_4567_89ab_00a7] {
        let (mut bus, handle, accesses) = setup();
        map_data(&mut bus, ROOT_A, DATA_PHYSICAL);
        put_bytes(&mut bus, CODE_PHYSICAL_A, &[0x48, 0x8b, 0x03, 0xee]);
        put(&mut bus, DATA_PHYSICAL, data);
        configure_uart(&mut bus);
        accesses.store(0, Ordering::SeqCst);

        let mut state = cpu(CODE_LINEAR);
        state.gpr[Register::Rbx as usize] = DATA_LINEAR;
        state.gpr[Register::Rdx as usize] = u64::from(COM1);
        state.rflags = 0x2d7;
        let initial_flags = state.rflags;
        let initial_cpu = state.clone();
        let mut callbacks = Vec::new();

        let mov = od_machine::step_paged_mov(
            &mut state,
            &mut bus,
            ROOT_A,
            EFER,
            context(PrivilegeLevel::Ring0),
            FLAGS_DEFINED,
            |address| callbacks.push(address),
        )
        .unwrap();
        assert_eq!(mov.cpu.exit, RunExit::StopAddress);
        assert_eq!(mov.cpu.instructions_executed, 1);
        assert_eq!(mov.cpu.rflags_defined, FLAGS_DEFINED);
        assert_eq!(state.gpr[Register::Rax as usize], data);
        assert_eq!(state.rip, CODE_LINEAR + 3);
        assert_eq!(state.rflags, initial_flags);

        let defined_after_mov = mov.cpu.rflags_defined;
        let pio = step_paged_pio_u8(
            &mut state,
            &mut bus,
            ROOT_A,
            EFER,
            context(PrivilegeLevel::Ring0),
            defined_after_mov,
            |address| callbacks.push(address),
        )
        .unwrap();
        assert_eq!(pio.cpu.exit, PioU8Exit::Retired);
        assert_eq!(pio.cpu.instructions_executed, 1);
        assert_eq!(pio.cpu.rflags_defined, defined_after_mov);
        assert_eq!(state.rip, initial_cpu.rip + 4);
        assert_eq!(state.rflags, initial_flags);
        assert_eq!(accesses.load(Ordering::SeqCst), 1);
        assert!(handle.transfer_holding_to_shift());
        assert_eq!(handle.complete_transmission(), Some(data as u8));
    }
}

#[test]
fn paged_pio_error_keeps_fetch_ad_callbacks_and_does_not_set_dirty() {
    let (mut bus, _, accesses) = setup();
    put_bytes(&mut bus, CODE_PHYSICAL_A, &[0xee]);
    let mut state = cpu(CODE_LINEAR);
    state.gpr[Register::Rdx as usize] = 0x1234;
    state.rflags = 0x2d7;
    let before = state.clone();
    let mut callbacks = Vec::new();

    let result = step_paged_pio_u8(
        &mut state,
        &mut bus,
        ROOT_A,
        EFER,
        context(PrivilegeLevel::Ring0),
        FLAGS_DEFINED,
        |address| callbacks.push(address),
    )
    .unwrap();

    assert_eq!(
        result.cpu.exit,
        PioU8Exit::PortIoError(BusAccessError::UnmappedPort(PortAddress::new(0x1234)))
    );
    assert_eq!(result.cpu.instructions_executed, 0);
    assert_eq!(result.cpu.rflags_defined, FLAGS_DEFINED);
    assert_eq!(state, before);
    assert_eq!(accesses.load(Ordering::SeqCst), 0);
    assert_eq!(
        callbacks,
        [range(0x1000), range(0x2000), range(0x3010), range(0x4000)]
    );

    for address in [0x1000, 0x2000, 0x3010, 0x4000] {
        let entry = qword(&mut bus, address);
        assert_ne!(entry & 0x20, 0, "fetch must commit A at {address:#x}");
        assert_eq!(entry & 0x40, 0, "fetch must not commit D at {address:#x}");
    }
}

#[test]
fn nonidentity_fetch_out_reaches_uart_with_two_distinct_values() {
    let (mut bus, handle, accesses) = setup();
    put_bytes(&mut bus, CODE_PHYSICAL_A, &[0xee]);
    configure_uart(&mut bus);
    accesses.store(0, Ordering::SeqCst);
    let mut state = cpu(CODE_LINEAR);
    state.gpr[Register::Rdx as usize] = u64::from(COM1);

    for value in [0x41, 0xa7] {
        state.gpr[Register::Rax as usize] = 0xfeed_face_cafe_be00 | value;
        let result = step(&mut state, &mut bus, ROOT_A);
        assert_eq!(result.cpu.exit, PioU8Exit::Retired);
        assert_eq!(result.cpu.instructions_executed, 1);
        assert_eq!(state.rip, CODE_LINEAR + 1);
        assert!(handle.transfer_holding_to_shift());
        assert_eq!(handle.complete_transmission(), Some(value as u8));
        state.rip = CODE_LINEAR;
    }
    assert_eq!(accesses.load(Ordering::SeqCst), 2);
}

#[test]
fn paged_in_reads_uart_and_preserves_upper_rax_bits() {
    let (mut bus, handle, accesses) = setup();
    put_bytes(&mut bus, CODE_PHYSICAL_A, &[0xec]);
    handle.inject_received_byte(0xd3).unwrap();
    let mut state = cpu(CODE_LINEAR);
    state.gpr[Register::Rdx as usize] = u64::from(COM1);
    state.gpr[Register::Rax as usize] = 0x1122_3344_5566_7788;

    let result = step(&mut state, &mut bus, ROOT_A);
    assert_eq!(result.cpu.exit, PioU8Exit::Retired);
    assert_eq!(state.gpr[Register::Rax as usize], 0x1122_3344_5566_77d3);
    assert_eq!(state.rip, CODE_LINEAR + 1);
    assert_eq!(accesses.load(Ordering::SeqCst), 1);
}

#[test]
fn selected_cr3_root_selects_the_fetched_instruction() {
    let (mut bus, handle, accesses) = setup();
    map_code(&mut bus, ROOT_B, CODE_PHYSICAL_B | 7);
    put_bytes(&mut bus, CODE_PHYSICAL_A, &[0xee]);
    put_bytes(&mut bus, CODE_PHYSICAL_B, &[0xec]);
    handle.inject_received_byte(0x5c).unwrap();
    let mut state = cpu(CODE_LINEAR);
    state.gpr[Register::Rdx as usize] = u64::from(COM1);
    state.gpr[Register::Rax as usize] = 0x1122_3344_5566_7788;

    let result = step(&mut state, &mut bus, ROOT_B);
    assert_eq!(result.cpu.exit, PioU8Exit::Retired);
    assert_eq!(state.gpr[Register::Rax as usize], 0x1122_3344_5566_775c);
    assert_eq!(state.rip, CODE_LINEAR + 1);
    assert_eq!(accesses.load(Ordering::SeqCst), 1);
}

#[test]
fn pio_error_and_invalid_admission_preserve_cpu_and_skip_device_access() {
    let (mut bus, _, accesses) = setup();
    put_bytes(&mut bus, CODE_PHYSICAL_A, &[0xec]);
    let mut state = cpu(CODE_LINEAR);
    state.gpr[Register::Rdx as usize] = 0x1234;
    let before = state.clone();
    let failed = step(&mut state, &mut bus, ROOT_A);
    assert_eq!(
        failed.cpu.exit,
        PioU8Exit::PortIoError(BusAccessError::UnmappedPort(PortAddress::new(0x1234)))
    );
    assert_eq!(failed.cpu.instructions_executed, 0);
    assert_eq!(failed.cpu.rflags_defined, FLAGS_DEFINED);
    assert_eq!(state, before);
    assert_eq!(accesses.load(Ordering::SeqCst), 0);

    let mut rejected_state = cpu(CODE_LINEAR);
    let rejected = step_paged_pio_u8(
        &mut rejected_state,
        &mut bus,
        ROOT_A,
        EFER,
        context(PrivilegeLevel::Ring3),
        FLAGS_DEFINED,
        |_| {},
    )
    .unwrap();
    assert!(matches!(rejected.cpu.exit, PioU8Exit::Rejected(_)));
    assert_eq!(rejected.cpu.instructions_executed, 0);
    assert_eq!(rejected_state.rip, CODE_LINEAR);
    assert_eq!(accesses.load(Ordering::SeqCst), 0);

    assert!(
        step_paged_pio_u8(
            &mut rejected_state,
            &mut bus,
            1 << 40,
            EFER,
            context(PrivilegeLevel::Ring0),
            FLAGS_DEFINED,
            |_| {},
        )
        .is_err()
    );
}

#[test]
fn not_present_and_nx_fetch_faults_do_not_access_uart() {
    for (pte, efer, expected_error_bits) in [
        (0, EFER, 0x10),
        (CODE_PHYSICAL_A | 7 | (1 << 63), EFER, 0x11),
    ] {
        let (mut bus, _, accesses) = setup();
        if pte == 0 {
            // Clear P on the final PTE while retaining the page-table chain.
            put(&mut bus, ROOT_A + 0x3000, CODE_PHYSICAL_A | 6);
        } else {
            put(&mut bus, ROOT_A + 0x3000, pte);
        }
        put_bytes(&mut bus, CODE_PHYSICAL_A, &[0xee]);
        let mut state = cpu(CODE_LINEAR);
        state.gpr[Register::Rdx as usize] = u64::from(COM1);
        let before = state.clone();

        let result = step_with_efer(&mut state, &mut bus, ROOT_A, efer);
        assert!(matches!(result.cpu.exit, PioU8Exit::Fault(_)));
        let fault = result.page_fault.expect("walker retains the page fault");
        assert_eq!(fault.saved_rip(), CODE_LINEAR);
        assert_eq!(fault.cr2_value(), CODE_LINEAR);
        assert_eq!(
            fault.fault().error_code.unwrap() & 0x1f,
            expected_error_bits
        );
        assert_eq!(result.cpu.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(accesses.load(Ordering::SeqCst), 0);
    }
}
