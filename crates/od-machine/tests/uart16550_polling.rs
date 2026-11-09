use od_machine::{
    AccessWidth, BusAccessError, DeviceError, GuestPhysicalAddress, GuestPhysicalRange, MachineBus,
    MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, PortAddress, PortRange,
    PortRegionBinding, RamLimits, Uart16550Handle, Uart16550PollingTx,
};

const COM1: u16 = 0x3f8;

fn make_bus() -> (MachineBus, Uart16550Handle) {
    let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 0x100).unwrap();
    let map =
        MachineMemoryMap::new(36, vec![MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)]).unwrap();
    let (uart, handle) = Uart16550PollingTx::new_with_handle();
    let bus = MachineBus::new(
        map,
        RamLimits::new(0x100),
        vec![Box::new(uart)],
        vec![],
        vec![PortRegionBinding {
            range: PortRange::new(PortAddress::new(COM1), 8).unwrap(),
            device_index: 0,
        }],
    )
    .unwrap();
    (bus, handle)
}

fn write(bus: &mut MachineBus, offset: u16, value: u8) -> Result<(), BusAccessError> {
    bus.write_port(
        PortAddress::new(COM1 + offset),
        AccessWidth::Byte,
        u32::from(value),
    )
}

fn read(bus: &mut MachineBus, offset: u16) -> Result<u32, BusAccessError> {
    bus.read_port(PortAddress::new(COM1 + offset), AccessWidth::Byte)
}

#[test]
fn polling_pio_and_external_handle_share_one_uart_state() {
    let (mut bus, handle) = make_bus();
    let alias = handle.clone();

    assert_eq!(read(&mut bus, 5), Ok(0x60));

    write(&mut bus, 3, 0x83).unwrap();
    write(&mut bus, 0, 0x34).unwrap();
    write(&mut bus, 1, 0x12).unwrap();
    write(&mut bus, 3, 0x03).unwrap();
    assert_eq!(read(&mut bus, 5), Ok(0x60));

    write(&mut bus, 0, 0x00).unwrap();
    assert_eq!(read(&mut bus, 5), Ok(0x00));
    assert_eq!(
        write(&mut bus, 0, 0xff),
        Err(BusAccessError::Device {
            device_index: 0,
            source: DeviceError::Busy,
        })
    );
    assert_eq!(read(&mut bus, 5), Ok(0x00));

    assert!(alias.transfer_holding_to_shift());
    assert_eq!(read(&mut bus, 5), Ok(0x20));
    assert_eq!(handle.complete_transmission(), Some(0x00));
    assert_eq!(handle.complete_transmission(), None);
    assert_eq!(read(&mut bus, 5), Ok(0x60));

    write(&mut bus, 0, 0x0a).unwrap();
    assert!(handle.transfer_holding_to_shift());
    assert_eq!(handle.complete_transmission(), Some(0x0a));
    assert_eq!(handle.complete_transmission(), None);
    assert_eq!(read(&mut bus, 5), Ok(0x60));
}

#[test]
fn polling_errors_and_reset_leave_shared_state_atomic() {
    let (mut bus, handle) = make_bus();

    assert_eq!(
        write(&mut bus, 0, 0x41),
        Err(BusAccessError::Device {
            device_index: 0,
            source: DeviceError::UnsupportedConfiguration,
        })
    );
    write(&mut bus, 3, 0x83).unwrap();
    write(&mut bus, 0, 0x34).unwrap();
    write(&mut bus, 1, 0x12).unwrap();
    write(&mut bus, 3, 0x03).unwrap();

    // S4-24e: IER bits 0 and 1 are accepted; line/modem-status enables are not.
    assert_eq!(
        write(&mut bus, 1, 0x04),
        Err(BusAccessError::Device {
            device_index: 0,
            source: DeviceError::UnsupportedConfiguration,
        })
    );
    assert_eq!(
        write(&mut bus, 2, 0x01),
        Err(BusAccessError::Device {
            device_index: 0,
            source: DeviceError::UnsupportedConfiguration,
        })
    );
    assert_eq!(read(&mut bus, 5), Ok(0x60));

    write(&mut bus, 0, 0xff).unwrap();
    assert_eq!(read(&mut bus, 5), Ok(0x00));
    handle.reset();
    assert_eq!(read(&mut bus, 5), Ok(0x60));
    assert_eq!(read(&mut bus, 3), Ok(0));
    assert_eq!(handle.complete_transmission(), None);
    assert_eq!(
        write(&mut bus, 0, 0x00),
        Err(BusAccessError::Device {
            device_index: 0,
            source: DeviceError::UnsupportedConfiguration,
        })
    );
    assert_eq!(read(&mut bus, 5), Ok(0x60));
}
