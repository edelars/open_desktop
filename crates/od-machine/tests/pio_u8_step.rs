use od_isa::execute::PioU8Exit;
use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};
use od_isa::{LongModeState, Register};
use od_machine::{
    AccessWidth, BusAccessError, BusDevice, DeviceAccess, DeviceError, GuestPhysicalAddress,
    GuestPhysicalRange, MachineBus, MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec,
    PortAddress, PortRange, PortRegionBinding, RamLimits, Uart16550Handle, Uart16550PollingTx,
    step_diagnostic_pio_u8,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

const COM1: u16 = 0x3f8;
const RAM_SIZE: u64 = 0x100;
const FLAGS_DEFINED: u64 = od_isa::INTEGER_FLAGS & !(1 << 4);

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

fn put_code(bus: &mut MachineBus, code: &[u8]) {
    for (offset, byte) in code.iter().copied().enumerate() {
        bus.write_physical(
            GuestPhysicalAddress::new(offset as u64),
            AccessWidth::Byte,
            u64::from(byte),
            |_| {},
        )
        .unwrap();
    }
}

fn context(privilege: PrivilegeLevel) -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege,
        origin: AccessOrigin::DescriptorTable,
    }
}

fn cpu(code: &[u8]) -> (LongModeState, MachineBus, Uart16550Handle, Arc<AtomicUsize>) {
    let (mut bus, handle, accesses) = make_bus();
    put_code(&mut bus, code);
    let mut state = LongModeState {
        rip: 0,
        ..LongModeState::default()
    };
    state.gpr[Register::Rdx as usize] = u64::from(COM1);
    state.rflags = 0x202;
    (state, bus, handle, accesses)
}

#[test]
fn in_al_dx_reads_one_uart_receive_byte_and_retires_once() {
    let (mut state, mut bus, handle, accesses) = cpu(&[0xec]);
    handle.inject_received_byte(0xa5).unwrap();
    assert_eq!(handle.inject_received_byte(0x5a), Err(DeviceError::Busy));
    state.gpr[Register::Rax as usize] = 0x1122_3344_5566_7788;

    let result = step_diagnostic_pio_u8(
        &mut state,
        &mut bus,
        context(PrivilegeLevel::Ring0),
        FLAGS_DEFINED,
    );

    assert_eq!(result.exit, PioU8Exit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(result.rflags_defined, FLAGS_DEFINED);
    assert_eq!(state.gpr[Register::Rax as usize], 0x1122_3344_5566_77a5);
    assert_eq!(state.rip, 1);
    assert_eq!(accesses.load(Ordering::SeqCst), 1);

    assert_eq!(
        bus.read_port(PortAddress::new(COM1), AccessWidth::Byte),
        Err(BusAccessError::Device {
            device_index: 0,
            source: DeviceError::WriteOnly,
        })
    );
    assert_eq!(
        bus.read_port(PortAddress::new(COM1 + 5), AccessWidth::Byte),
        Ok(0x60)
    );
    assert_eq!(
        bus.read_port(PortAddress::new(COM1 + 5), AccessWidth::Byte),
        Ok(0x60)
    );

    handle.inject_received_byte(0x5b).unwrap();
    assert_eq!(
        bus.read_port(PortAddress::new(COM1 + 5), AccessWidth::Byte),
        Ok(0x61)
    );
    assert_eq!(
        bus.read_port(PortAddress::new(COM1), AccessWidth::Byte),
        Ok(0x5b)
    );
    assert_eq!(
        bus.read_port(PortAddress::new(COM1 + 5), AccessWidth::Byte),
        Ok(0x60)
    );
}

#[test]
fn out_dx_al_writes_one_uart_transmit_byte_and_retires_once() {
    let (mut state, mut bus, handle, accesses) = cpu(&[0xee]);
    for (port, value) in [(3, 0x83), (0, 0x34), (1, 0x12), (3, 0x03)] {
        bus.write_port(PortAddress::new(COM1 + port), AccessWidth::Byte, value)
            .unwrap();
    }
    accesses.store(0, Ordering::SeqCst);
    state.gpr[Register::Rax as usize] = 0xfeed_face_cafe_be41;

    let result = step_diagnostic_pio_u8(
        &mut state,
        &mut bus,
        context(PrivilegeLevel::Ring0),
        FLAGS_DEFINED,
    );

    assert_eq!(result.exit, PioU8Exit::Retired);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, 1);
    assert_eq!(accesses.load(Ordering::SeqCst), 1);
    assert!(handle.transfer_holding_to_shift());
    assert_eq!(handle.complete_transmission(), Some(0x41));
}

#[test]
fn pio_failure_preserves_cpu_state_and_definedness() {
    let (mut state, mut bus, _, accesses) = cpu(&[0xec]);
    state.gpr[Register::Rdx as usize] = 0x1234;
    let before = state.clone();

    let result = step_diagnostic_pio_u8(
        &mut state,
        &mut bus,
        context(PrivilegeLevel::Ring0),
        FLAGS_DEFINED,
    );

    assert_eq!(
        result.exit,
        PioU8Exit::PortIoError(BusAccessError::UnmappedPort(PortAddress::new(0x1234)))
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, FLAGS_DEFINED);
    assert_eq!(state, before);
    assert_eq!(accesses.load(Ordering::SeqCst), 0);
}

#[test]
fn fetch_backend_error_and_unsupported_or_cpl_rejection_do_not_retire() {
    let (mut state, mut bus, _, accesses) = cpu(&[0xec]);
    state.rip = RAM_SIZE;
    let before = state.clone();
    let fetch = step_diagnostic_pio_u8(
        &mut state,
        &mut bus,
        context(PrivilegeLevel::Ring0),
        FLAGS_DEFINED,
    );
    assert!(matches!(fetch.exit, PioU8Exit::MemoryError(_)));
    assert_eq!(fetch.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(accesses.load(Ordering::SeqCst), 0);

    state.rip = 0;
    put_code(&mut bus, &[0x90]);
    let unsupported = step_diagnostic_pio_u8(
        &mut state,
        &mut bus,
        context(PrivilegeLevel::Ring0),
        FLAGS_DEFINED,
    );
    assert_eq!(unsupported.exit, PioU8Exit::Unsupported { opcode: 0x90 });
    assert_eq!(unsupported.instructions_executed, 0);

    put_code(&mut bus, &[0xec]);
    let before = state.clone();
    let rejected = step_diagnostic_pio_u8(
        &mut state,
        &mut bus,
        context(PrivilegeLevel::Ring3),
        FLAGS_DEFINED,
    );
    assert!(matches!(rejected.exit, PioU8Exit::Rejected(_)));
    assert_eq!(rejected.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(accesses.load(Ordering::SeqCst), 0);
}
