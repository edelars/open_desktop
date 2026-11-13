use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};
use od_isa::{CpuActivity, INTEGER_FLAGS, LongModeState, Register, StiShadow};
use od_machine::{
    AccessWidth, BusDevice, DeviceAccess, DeviceError, EventQueueError, GuestPhysicalAddress,
    GuestPhysicalRange, IoSpace, MachineBus, MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec,
    PagedProbeExit, PortAddress, PortRange, PortRegionBinding, RamLimits, Uart16550Handle,
    Uart16550PollingTx, UartRxDispatchExit, UartRxEventPump, VirtualClock, run_paged_probe,
};

const COM1: u16 = 0x3f8;
const RAM_SIZE: u64 = 0x20_000;
const ROOT: u64 = 0x1000;
const CODE_LINEAR: u64 = 0x40_0000;
const CODE_PHYSICAL: u64 = 0x7000;
const CR0: u64 = 0x8001_0033;
const CR4: u64 = 1 << 5;
const EFER: u64 = (1 << 8) | (1 << 10) | (1 << 11);
const FLAGS_DEFINED: u64 = INTEGER_FLAGS & !(1 << 4);

fn uart() -> (Uart16550PollingTx, Uart16550Handle) {
    Uart16550PollingTx::new_with_handle()
}

fn read_rx(uart: &mut Uart16550PollingTx) -> Result<u64, DeviceError> {
    uart.read(DeviceAccess {
        space: IoSpace::Pio,
        offset: 0,
        width: AccessWidth::Byte,
    })
}

#[test]
fn deadline_readiness_and_equal_deadline_reschedule_keep_queue_order() {
    let (mut device, handle) = uart();
    let mut pump = UartRxEventPump::new(3, handle);
    let generation = pump.generation();
    pump.schedule(generation, 1, 10, 0x11).unwrap();
    pump.schedule(generation, 2, 10, 0x22).unwrap();
    pump.schedule(generation, 3, 20, 0x33).unwrap();
    pump.reschedule(generation, 3, 10).unwrap();

    assert_eq!(pump.next_deadline(), Some(10));
    assert_eq!(
        pump.dispatch_due(generation, 9, 3, || false).unwrap().exit,
        UartRxDispatchExit::DeadlinePending
    );
    assert_eq!(
        pump.dispatch_due(generation, 10, 3, || false).unwrap(),
        od_machine::UartRxDispatchReport {
            delivered: 1,
            exit: UartRxDispatchExit::ReceiverBusy,
        }
    );
    assert_eq!(read_rx(&mut device), Ok(0x11));
    assert_eq!(
        pump.dispatch_due(generation, 10, 3, || false).unwrap().exit,
        UartRxDispatchExit::ReceiverBusy
    );
    assert_eq!(read_rx(&mut device), Ok(0x22));
    assert_eq!(
        pump.dispatch_due(generation, 10, 3, || false).unwrap(),
        od_machine::UartRxDispatchReport {
            delivered: 1,
            exit: UartRxDispatchExit::NoEvent,
        }
    );
    assert_eq!(read_rx(&mut device), Ok(0x33));
    assert_eq!(
        pump.dispatch_due(generation, 10, 3, || false).unwrap().exit,
        UartRxDispatchExit::NoEvent
    );
}

#[test]
fn busy_retry_preserves_payload_deadline_and_prevents_later_event_bypass() {
    let (mut device, handle) = uart();
    let mut pump = UartRxEventPump::new(2, handle);
    let generation = pump.generation();
    pump.schedule(generation, 1, 5, 0xa1).unwrap();
    pump.schedule(generation, 2, 5, 0xb2).unwrap();
    device.inject_received_byte(0x7f).unwrap();

    assert_eq!(
        pump.dispatch_due(generation, 5, 2, || false).unwrap(),
        od_machine::UartRxDispatchReport {
            delivered: 0,
            exit: UartRxDispatchExit::ReceiverBusy,
        }
    );
    assert_eq!(pump.len(), 2);
    assert_eq!(pump.next_deadline(), Some(5));
    assert_eq!(read_rx(&mut device), Ok(0x7f));
    assert_eq!(
        pump.dispatch_due(generation, 5, 1, || false).unwrap(),
        od_machine::UartRxDispatchReport {
            delivered: 1,
            exit: UartRxDispatchExit::BudgetExhausted,
        }
    );
    assert_eq!(pump.len(), 1);
    assert_eq!(read_rx(&mut device), Ok(0xa1));
    assert_eq!(
        pump.dispatch_due(generation, 5, 1, || false)
            .unwrap()
            .delivered,
        1
    );
    assert_eq!(read_rx(&mut device), Ok(0xb2));
    assert!(pump.is_empty());
}

#[test]
fn stop_and_zero_budget_do_not_inject_and_stop_precedes_budget() {
    let (mut device, handle) = uart();
    let mut pump = UartRxEventPump::new(1, handle);
    let generation = pump.generation();
    pump.schedule(generation, 1, 0, 0x41).unwrap();

    assert_eq!(
        pump.dispatch_due(generation, 0, 0, || true).unwrap().exit,
        UartRxDispatchExit::Stopped
    );
    assert_eq!(
        pump.dispatch_due(generation, 0, 0, || false).unwrap().exit,
        UartRxDispatchExit::BudgetExhausted
    );
    // S4-38L9z: an empty receiver reads as the last received byte (0 here).
    assert_eq!(read_rx(&mut device), Ok(0));
    assert_eq!(pump.len(), 1);
}

#[test]
fn cancel_duplicate_full_and_zero_capacity_are_atomic() {
    let (mut device, handle) = uart();
    let mut pump = UartRxEventPump::new(1, handle);
    let generation = pump.generation();
    assert_eq!(pump.cancel(generation, 7), Err(EventQueueError::NotFound));
    pump.schedule(generation, 7, 4, 0x77).unwrap();
    assert_eq!(
        pump.schedule(generation, 7, 5, 0x88),
        Err(EventQueueError::DuplicateKey)
    );
    assert_eq!(
        pump.schedule(generation, 8, 5, 0x88),
        Err(EventQueueError::Full)
    );
    pump.cancel(generation, 7).unwrap();
    assert!(pump.is_empty());
    // S4-38L9z: an empty receiver reads as the last received byte (0 here).
    assert_eq!(read_rx(&mut device), Ok(0));

    let (_, zero_capacity_handle) = uart();
    let mut zero_capacity = UartRxEventPump::new(0, zero_capacity_handle);
    let zero_generation = zero_capacity.generation();
    assert_eq!(
        zero_capacity.schedule(zero_generation, 1, 0, 1),
        Err(EventQueueError::Full)
    );
    assert_eq!(zero_capacity.len(), 0);
}

#[test]
fn reset_discards_pending_payload_invalidates_old_generation_and_preserves_rx_and_clock() {
    let (mut device, handle) = uart();
    let mut pump = UartRxEventPump::new(2, handle);
    let mut clock = VirtualClock::new(1_000_000_000).unwrap();
    clock.advance_time_ns(12).unwrap();
    let old = pump.generation();
    pump.schedule(old, 1, 0, 0xa1).unwrap();
    pump.schedule(old, 2, 50, 0xb2).unwrap();
    assert_eq!(
        pump.dispatch_due(old, clock.now_ns(), 1, || false)
            .unwrap()
            .delivered,
        1
    );
    let new = pump.reset(old).unwrap();
    assert_ne!(new, old);
    assert_eq!(clock.now_ns(), 12);
    assert_eq!(pump.len(), 0);
    assert_eq!(read_rx(&mut device), Ok(0xa1));
    assert_eq!(
        pump.schedule(old, 1, 12, 0xff),
        Err(EventQueueError::StaleGeneration)
    );
    assert_eq!(
        pump.dispatch_due(old, 12, 1, || false),
        Err(od_machine::UartRxPumpError::Queue(
            EventQueueError::StaleGeneration
        ))
    );
    // S4-38L9z: an empty receiver reads as the last received byte (0xa1 here).
    assert_eq!(read_rx(&mut device), Ok(0xa1));

    pump.schedule(new, 1, 12, 0xc3).unwrap();
    assert_eq!(
        pump.dispatch_due(new, 12, 1, || false).unwrap().delivered,
        1
    );
    assert_eq!(read_rx(&mut device), Ok(0xc3));
}

#[test]
fn repeated_fill_reset_and_drain_never_exceed_capacity() {
    let (_, handle) = uart();
    let mut pump = UartRxEventPump::new(4, handle);
    for _ in 0..64 {
        let generation = pump.generation();
        for key in 0..4 {
            pump.schedule(generation, key, key, key as u8).unwrap();
            assert!(pump.len() <= pump.capacity());
        }
        assert_eq!(pump.len(), 4);
        let next = pump.reset(generation).unwrap();
        assert_ne!(next, generation);
        assert_eq!(pump.len(), 0);
        assert!(pump.capacity() == 4);
    }
}

fn machine_fixture() -> (MachineBus, Uart16550Handle) {
    let range = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM_SIZE).unwrap();
    let map = MachineMemoryMap::new(
        36,
        vec![MemoryRegionSpec::new(range, MemoryRegionKind::Ram)],
    )
    .unwrap();
    let (uart, handle) = uart();
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
    put_qword(&mut bus, ROOT, ROOT + 0x1007);
    put_qword(&mut bus, ROOT + 0x1000, ROOT + 0x2007);
    put_qword(&mut bus, ROOT + 0x2010, ROOT + 0x3007);
    put_qword(&mut bus, ROOT + 0x3000, CODE_PHYSICAL | 7);
    for (index, byte) in [0xec_u8, 0xec, 0xf4].into_iter().enumerate() {
        bus.write_physical(
            GuestPhysicalAddress::new(CODE_PHYSICAL + index as u64),
            AccessWidth::Byte,
            u64::from(byte),
            |_| {},
        )
        .unwrap();
    }
    (bus, handle)
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

#[test]
fn scheduled_rx_bytes_feed_paged_in_in_in_and_hlt_execution() {
    let (mut bus, handle) = machine_fixture();
    let mut pump = UartRxEventPump::new(2, handle);
    let mut clock = VirtualClock::new(1_000_000_000).unwrap();
    let generation = pump.generation();
    pump.schedule(generation, 1, 10, 0x5a).unwrap();
    pump.schedule(generation, 2, 20, 0xc3).unwrap();

    let mut cpu = LongModeState {
        cr3: None,
        rip: CODE_LINEAR,
        cr0: Some(CR0),
        cr4: Some(CR4),
        ..LongModeState::default()
    };
    cpu.activity = Some(CpuActivity::Running);
    cpu.sti_shadow = Some(StiShadow::Clear);
    cpu.gpr[Register::Rdx as usize] = u64::from(COM1);
    cpu.gpr[Register::Rax as usize] = 0x1122_3344_5566_7700;
    cpu.rflags = 0x2d7;

    assert_eq!(
        pump.dispatch_due(generation, clock.now_ns(), 2, || false)
            .unwrap()
            .exit,
        UartRxDispatchExit::DeadlinePending
    );
    clock.advance_time_ns(10).unwrap();
    assert_eq!(
        pump.dispatch_due(generation, clock.now_ns(), 2, || false)
            .unwrap()
            .delivered,
        1
    );
    let first = run_paged_probe(
        &mut cpu,
        &mut bus,
        ROOT,
        EFER,
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::DescriptorTable,
        },
        1,
        FLAGS_DEFINED,
        || false,
        |_| {},
    );
    assert_eq!(first.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(cpu.gpr[Register::Rax as usize], 0x1122_3344_5566_775a);

    clock.advance_time_ns(10).unwrap();
    assert_eq!(
        pump.dispatch_due(generation, clock.now_ns(), 2, || false)
            .unwrap()
            .delivered,
        1
    );
    let rest = run_paged_probe(
        &mut cpu,
        &mut bus,
        ROOT,
        EFER,
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::DescriptorTable,
        },
        2,
        FLAGS_DEFINED,
        || false,
        |_| {},
    );
    assert_eq!(rest.exit, PagedProbeExit::Halted);
    assert_eq!(rest.instructions_executed, 2);
    assert_eq!(cpu.gpr[Register::Rax as usize], 0x1122_3344_5566_77c3);
    assert_eq!(cpu.rip, CODE_LINEAR + 3);
    assert_eq!(cpu.activity, Some(CpuActivity::Halted));
    assert_eq!(cpu.rflags, 0x2d7);
    assert_eq!(clock.now_ns(), 20);
}
