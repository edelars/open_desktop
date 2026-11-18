//! S6-33: the bus and the devices from several host threads (MACHINE_CONTRACT.md, "Порядок
//! блокировок"). No vCPU runs in a thread here (that is S6-34): the threads are host threads that
//! do what vCPU threads will do, and every wait has a time limit, so a deadlock fails the test
//! instead of hanging it.
use super::*;
use crate::irq_source::BoardIrqSource;
use crate::pit_ioapic_irq_source::PitIoapicIrqSource;
use crate::pit_pic_irq_source::PitPicIrqSource;
use crate::uart_ioapic_irq_source::UartIoapicIrqSource;
use crate::uart_pic_irq_source::UartPicIrqSource;
use crate::{
    IoApic, IoApicHandle, LOCAL_APIC_MMIO_BASE, LOCAL_APIC_MMIO_SIZE, LocalApic, LocalApicHandle,
    MemoryRegionSpec, Pic8259, Pic8259Handle, Pic8259Role, Pit8254Devices, Pit8254Handle,
    Uart16550Handle, Uart16550PollingTx, VirtualClockHandle,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(30);
const ROUNDS: u64 = 3000;
const RAM_SIZE: u64 = 0x1_0000;
/// The port window of the plain latch device (device 1).
const LATCH_PORT: u16 = 0x80;
const MMIO: u64 = LOCAL_APIC_MMIO_BASE;

/// Runs `work` on a new thread; the test fails when it does not return in `TIMEOUT` (a deadlock
/// leaves the thread behind, which is the price of not hanging) or panics.
fn finishes_in_time<R: Send + 'static>(what: &str, work: impl FnOnce() -> R + Send + 'static) -> R {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    match rx.recv_timeout(TIMEOUT) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!("{what}: no result in {TIMEOUT:?}: deadlock")
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => panic!("{what}: the thread panicked"),
    }
}

/// A device with one register that remembers the last written value; a device that is not a
/// Local APIC, behind the same device lock.
struct Latch(u64);

impl BusDevice for Latch {
    fn read(&mut self, _: DeviceAccess) -> Result<u64, DeviceError> {
        Ok(self.0)
    }
    fn write(&mut self, _: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        self.0 = value;
        Ok(())
    }
}

/// A bus with 64 KiB of RAM, the Local APIC window of a two-processor board (device 0) and a
/// latch on ports `0x80..0x88` (device 1); both Local APICs software-enabled.
struct Rig {
    bus: Arc<MachineBus>,
    apics: [LocalApicHandle; 2],
}

fn rig() -> Rig {
    let lapic = LocalApic::with_cpus(2).unwrap();
    let apics = [lapic.handle(), lapic.handle_for(1).unwrap()];
    let base = GuestPhysicalAddress::new(MMIO);
    let map = MachineMemoryMap::new(
        36,
        vec![
            MemoryRegionSpec::new(
                GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM_SIZE).unwrap(),
                MemoryRegionKind::Ram,
            ),
            MemoryRegionSpec::new(
                GuestPhysicalRange::new(base, LOCAL_APIC_MMIO_SIZE).unwrap(),
                MemoryRegionKind::Mmio,
            ),
        ],
    )
    .unwrap();
    let bus = MachineBus::new(
        map,
        RamLimits::new(RAM_SIZE),
        vec![Box::new(lapic), Box::new(Latch(0))],
        vec![MmioBinding {
            region_start: base,
            device_index: 0,
        }],
        vec![PortRegionBinding {
            range: PortRange::new(PortAddress::new(LATCH_PORT), 8).unwrap(),
            device_index: 1,
        }],
    )
    .unwrap();
    let bus = Arc::new(bus);
    for cpu in 0..2 {
        bus.write_mmio_shared(
            cpu,
            GuestPhysicalAddress::new(MMIO + 0xf0),
            AccessWidth::Dword,
            0x1ff,
        )
        .unwrap();
    }
    Rig { bus, apics }
}

/// vCPU `from` sends the Fixed vector `vector` to `to` through the bus: ICR high, then ICR low.
fn send_ipi(bus: &MachineBus, from: usize, to: usize, vector: u8) -> Result<(), BusAccessError> {
    bus.write_mmio_shared(
        from,
        GuestPhysicalAddress::new(MMIO + 0x310),
        AccessWidth::Dword,
        (to as u64) << 24,
    )?;
    bus.write_mmio_shared(
        from,
        GuestPhysicalAddress::new(MMIO + 0x300),
        AccessWidth::Dword,
        u64::from(vector),
    )
}

/// The vector sent in round `round`: 208 distinct vectors (`0x30..=0xff`) repeat across the
/// rounds of a run, and equal vectors coalesce in one IRR bit, so a lost IPI shows as a missing
/// IRR bit only for a vector no other round delivered; the counters (6000 writes) count the
/// accepted writes.
fn vector_of(round: u64) -> u8 {
    0x30 + (round % 0xd0) as u8
}

const ROUNDS_DISTINCT: u64 = 0xd0;

fn assert_every_vector_arrived(apic: &LocalApicHandle, who: &str) {
    for round in 0..ROUNDS_DISTINCT {
        assert_eq!(
            apic.vector_state(vector_of(round)),
            (true, false),
            "{who}: vector {:#x} did not arrive",
            vector_of(round)
        );
    }
}

fn assert_send_sync<T: Send + Sync>() {}
fn assert_send<T: Send>() {}

#[test]
fn everything_a_vcpu_thread_reaches_is_send_and_sync() {
    assert_send_sync::<MachineBus>();
    assert_send_sync::<PhysicalMemory>();
    assert_send_sync::<LocalApic>();
    assert_send_sync::<LocalApicHandle>();
    assert_send_sync::<IoApicHandle>();
    assert_send_sync::<Pit8254Handle>();
    assert_send_sync::<Pic8259Handle>();
    assert_send_sync::<Uart16550Handle>();
    assert_send_sync::<VirtualClockHandle>();
    assert_send_sync::<PitIoapicIrqSource>();
    assert_send_sync::<UartIoapicIrqSource>();
    assert_send_sync::<PitPicIrqSource>();
    assert_send_sync::<UartPicIrqSource>();
    // The device table and the source of a board are `Send` by their traits.
    assert_send::<Box<dyn BusDevice>>();
    assert_send::<Box<dyn BoardIrqSource>>();
    // The board owns the bus and the source; it moves to a thread whole.
    assert_send::<crate::firmware_board::LoadedFirmwareBoard>();
}

/// The ICR writes of two vCPUs aimed at each other, through the bus (device lock, then APIC
/// lock), in lockstep rounds and then free-running: no deadlock, every IPI delivered.
#[test]
fn icr_writes_aimed_at_each_other_through_the_bus_do_not_deadlock_and_deliver_both() {
    let rig = rig();
    let delivered = Arc::new([AtomicU64::new(0), AtomicU64::new(0)]);
    let barrier = Arc::new(Barrier::new(2));
    let (done_tx, done_rx) = mpsc::channel();
    for me in 0..2usize {
        let (bus, delivered, barrier, done_tx) = (
            Arc::clone(&rig.bus),
            Arc::clone(&delivered),
            Arc::clone(&barrier),
            done_tx.clone(),
        );
        std::thread::spawn(move || {
            for round in 0..ROUNDS {
                // Lockstep: both sides write at the same moment, the worst case for a cycle.
                barrier.wait();
                if send_ipi(&bus, me, 1 - me, vector_of(round)).is_ok() {
                    delivered[me].fetch_add(1, Ordering::Relaxed);
                }
            }
            for round in 0..ROUNDS {
                if send_ipi(&bus, me, 1 - me, vector_of(round)).is_ok() {
                    delivered[me].fetch_add(1, Ordering::Relaxed);
                }
            }
            let _ = done_tx.send(me);
        });
    }
    for _ in 0..2 {
        done_rx
            .recv_timeout(TIMEOUT)
            .expect("the vCPU threads did not finish: deadlock");
    }
    assert_eq!(delivered[0].load(Ordering::Relaxed), 2 * ROUNDS);
    assert_eq!(delivered[1].load(Ordering::Relaxed), 2 * ROUNDS);
    assert_every_vector_arrived(&rig.apics[0], "vCPU 0 received from vCPU 1");
    assert_every_vector_arrived(&rig.apics[1], "vCPU 1 received from vCPU 0");
}

/// The same crossing through the handles of the two Local APICs, without the bus (the APIC lock
/// alone): the path a vCPU thread takes when it holds no `&MachineBus`.
#[test]
fn icr_writes_aimed_at_each_other_through_the_handles_do_not_deadlock_and_deliver_both() {
    let rig = rig();
    let barrier = Arc::new(Barrier::new(2));
    let delivered = Arc::new([AtomicU64::new(0), AtomicU64::new(0)]);
    let (done_tx, done_rx) = mpsc::channel();
    for me in 0..2usize {
        let apic = rig.apics[me].clone();
        let (barrier, delivered, done_tx) = (
            Arc::clone(&barrier),
            Arc::clone(&delivered),
            done_tx.clone(),
        );
        std::thread::spawn(move || {
            for round in 0..ROUNDS {
                barrier.wait();
                let vector = u32::from(vector_of(round));
                if apic.write_icr(me, (1 - me) as u8, vector).is_ok() {
                    delivered[me].fetch_add(1, Ordering::Relaxed);
                }
            }
            let _ = done_tx.send(me);
        });
    }
    for _ in 0..2 {
        done_rx
            .recv_timeout(TIMEOUT)
            .expect("the vCPU threads did not finish: deadlock");
    }
    assert_eq!(delivered[0].load(Ordering::Relaxed), ROUNDS);
    assert_eq!(delivered[1].load(Ordering::Relaxed), ROUNDS);
    assert_every_vector_arrived(&rig.apics[0], "vCPU 0 received from vCPU 1");
    assert_every_vector_arrived(&rig.apics[1], "vCPU 1 received from vCPU 0");
}

/// One thread sends through the bus, the other works on its own Local APIC through the handle
/// (what a vCPU does at an instruction boundary: look at the pending vector, take it) and a third
/// reads and writes RAM; the bus-side sender and the handle-side consumer never wait on each other
/// in a cycle.
#[test]
fn the_bus_the_handles_and_ram_used_at_once_finish() {
    let rig = rig();
    let bus = Arc::clone(&rig.bus);
    let apic1 = rig.apics[1].clone();
    let apic0 = rig.apics[0].clone();
    finishes_in_time("mixed use", move || {
        let sender = {
            let bus = Arc::clone(&bus);
            std::thread::spawn(move || {
                for round in 0..ROUNDS {
                    send_ipi(&bus, 0, 1, vector_of(round)).unwrap();
                }
            })
        };
        let consumer = std::thread::spawn(move || {
            let mut taken = 0u64;
            for _ in 0..ROUNDS * 4 {
                if apic1.acknowledge().is_some() {
                    taken += 1;
                }
                let _ = apic1.eligible();
                let _ = apic0.pending();
            }
            taken
        });
        let ram = {
            let bus = Arc::clone(&bus);
            std::thread::spawn(move || {
                for i in 0..ROUNDS * 4 {
                    let at = GuestPhysicalAddress::new((i % 0x1ff) * 8);
                    bus.physical_memory().store_u64(at, i, |_| {}).unwrap();
                    assert_eq!(bus.physical_memory().load_u64(at).unwrap(), i);
                }
            })
        };
        sender.join().unwrap();
        let _taken = consumer.join().unwrap();
        ram.join().unwrap();
    });
}

/// (a) The lock order of the contract: the device lock comes first, then the APIC lock. While one
/// thread holds the device lock, every operation that reaches the APIC without the bus (a
/// `LocalApicHandle`, the IOAPIC handle, every interrupt source) still completes: none of them
/// takes the device lock, so there is no path from an APIC (or a source) back to the bus.
#[test]
fn no_apic_or_source_operation_takes_the_device_lock() {
    let rig = rig();
    let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
    let pit = Pit8254Devices::new(clock.clone());
    let lapic = rig.apics[0].clone();
    let io_device = IoApic::new(lapic.clone());
    let (_pic_device, pic) = Pic8259::new_with_handle(Pic8259Role::Master);
    let (_uart_device, uart) = Uart16550PollingTx::new_with_handle();
    let pit_ioapic = PitIoapicIrqSource::new(
        pit.handle.clone(),
        io_device.handle(),
        lapic.clone(),
        clock.clone(),
    );
    let uart_ioapic =
        UartIoapicIrqSource::new(pit_ioapic.clone(), uart.clone(), io_device.handle());
    let pit_pic = PitPicIrqSource::new(pit.handle.clone(), pic.clone(), clock);
    let uart_pic = UartPicIrqSource::new(pit_pic.clone(), uart.clone(), pic.clone());
    let io = io_device.handle();

    let held = rig.bus.lock_devices();
    let operations = finishes_in_time("operations under a held device lock", move || {
        let mut sources: Vec<Box<dyn BoardIrqSource>> = vec![
            Box::new(pit_ioapic),
            Box::new(uart_ioapic),
            Box::new(pit_pic),
            Box::new(uart_pic),
        ];
        let mut count = 0u64;
        for source in &mut sources {
            let _ = source.pending_vector();
            let _ = source.next_deadline_ns();
            let _ = source.wake_ns();
            let _ = source.irq0_edges();
            let _ = source.pit_write_ends_quantum();
            source.acknowledge(0x30);
            count += 6;
        }
        for apic in [&lapic, &lapic.for_cpu(1).unwrap()] {
            let _ = apic.eligible();
            let _ = apic.pending();
            let _ = apic.acknowledge();
            let _ = apic.acknowledge_or_spurious();
            let _ = apic.take_startup();
            let _ = apic.take_nmi();
            let _ = apic.has_timer();
            let _ = apic.next_timer_deadline();
            let _ = apic.dispatch_timer();
            let _ = apic.request_fixed_edge(0x41);
            let _ = apic.snapshot_pair();
            let _ = apic.cpu_count();
            count += 12;
        }
        let _ = io.set_intin4(true);
        let _ = io.set_intin4(false);
        let _ = uart.interrupt_line();
        let _ = pit.handle.snapshot();
        pic.raise_irq(4);
        count + 4
    });
    assert!(operations > 0);
    drop(held);
}

/// (a) The allowed direction: a thread that holds the device lock reaches a Local APIC through
/// the device (device lock, then APIC lock), also several times in a row.
#[test]
fn the_device_lock_then_the_apic_lock_is_the_order_and_it_works() {
    let rig = rig();
    let bus = Arc::clone(&rig.bus);
    finishes_in_time("device lock then APIC lock", move || {
        let mut devices = bus.lock_devices();
        let access = |offset| DeviceAccess {
            space: IoSpace::Mmio,
            offset,
            width: AccessWidth::Dword,
        };
        for round in 0..100u64 {
            devices[0]
                .write_from_cpu(0, access(0x310), 1 << 24)
                .unwrap();
            devices[0]
                .write_from_cpu(0, access(0x300), u64::from(vector_of(round)))
                .unwrap();
            assert_eq!(devices[0].read_from_cpu(1, access(0x20)), Ok(1 << 24));
        }
    });
    assert_eq!(rig.apics[1].vector_state(vector_of(0)), (true, false));
}

/// (a) The direction the order forbids is not needed either: while the APIC lock is held by
/// another thread, a Local APIC access through the bus waits (holding the device lock, as the
/// order says) and completes as soon as the APIC lock is released; every other device stays
/// unreachable only for as long as that waiter holds the device lock, and is reachable again after.
#[test]
fn an_apic_access_waits_for_the_apic_lock_while_holding_the_device_lock() {
    let rig = rig();
    let apic_lock = rig.apics[0].0.lock().unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let bus = Arc::clone(&rig.bus);
    std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let result = bus.read_mmio_shared(
            0,
            GuestPhysicalAddress::new(MMIO + 0x20),
            AccessWidth::Dword,
        );
        done_tx.send(result).unwrap();
    });
    started_rx.recv_timeout(TIMEOUT).unwrap();
    // The waiter takes the device lock first, then blocks on the APIC lock.
    let began = Instant::now();
    while rig.bus.devices.try_lock().is_ok() {
        assert!(
            began.elapsed() < TIMEOUT,
            "the waiter never took the device lock"
        );
        std::thread::yield_now();
    }
    assert!(
        done_rx.try_recv().is_err(),
        "the access finished while the APIC lock was held"
    );
    drop(apic_lock);
    assert_eq!(done_rx.recv_timeout(TIMEOUT).unwrap(), Ok(0));
    assert_eq!(
        rig.bus
            .read_port_shared(PortAddress::new(LATCH_PORT), AccessWidth::Byte),
        Ok(0)
    );
}

/// (c) RAM is not behind the device lock: a thread that holds it does not stop another thread's
/// access to RAM through the `&PhysicalMemory` of the same bus, and a thread that waits for the
/// device lock does not either.
#[test]
fn ram_is_not_behind_the_device_lock() {
    let rig = rig();
    let held = rig.bus.lock_devices();
    let bus = Arc::clone(&rig.bus);
    let waiter_bus = Arc::clone(&rig.bus);
    // A second thread waits for the device lock the whole time.
    let waiter = std::thread::spawn(move || {
        waiter_bus.write_port_shared(PortAddress::new(LATCH_PORT), AccessWidth::Byte, 7)
    });
    finishes_in_time("RAM access under a held device lock", move || {
        let memory = bus.physical_memory();
        for i in 0..20_000u64 {
            let at = GuestPhysicalAddress::new((i % 0x1ff) * 8);
            memory.store_u64(at, i ^ 0x5a5a, |_| {}).unwrap();
            assert_eq!(memory.load_u64(at).unwrap(), i ^ 0x5a5a);
        }
        let mut bytes = [0u8; 16];
        memory
            .load_bytes(GuestPhysicalAddress::new(0x100), &mut bytes)
            .unwrap();
    });
    drop(held);
    assert_eq!(waiter.join().unwrap(), Ok(()));
    assert_eq!(
        rig.bus
            .read_port_shared(PortAddress::new(LATCH_PORT), AccessWidth::Byte),
        Ok(7)
    );
}

/// The shared access (device lock) and the exclusive access (`&mut`, no lock) reach the same
/// devices with the same results and errors.
#[test]
fn shared_and_exclusive_access_agree() {
    let mut rig = rig();
    let port = PortAddress::new(LATCH_PORT);
    rig.bus
        .write_port_shared(port, AccessWidth::Byte, 0x42)
        .unwrap();
    {
        let bus = Arc::get_mut(&mut rig.bus).expect("no other reference");
        assert_eq!(bus.read_port(port, AccessWidth::Byte), Ok(0x42));
        bus.write_port(port, AccessWidth::Byte, 0x17).unwrap();
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(MMIO + 0x20), AccessWidth::Dword),
            Ok(0)
        );
    }
    assert_eq!(rig.bus.read_port_shared(port, AccessWidth::Byte), Ok(0x17));
    assert_eq!(
        rig.bus.read_mmio_shared(
            1,
            GuestPhysicalAddress::new(MMIO + 0x20),
            AccessWidth::Dword
        ),
        Ok(1 << 24)
    );
    // Unmapped ports and a window of no device give the errors of the exclusive path.
    assert_eq!(
        rig.bus
            .read_port_shared(PortAddress::new(0x1234), AccessWidth::Byte),
        Err(BusAccessError::UnmappedPort(PortAddress::new(0x1234)))
    );
    assert_eq!(
        rig.bus.read_port_shared(port, AccessWidth::Qword),
        Err(BusAccessError::UnsupportedWidth)
    );
    assert!(matches!(
        rig.bus
            .read_mmio_shared(0, GuestPhysicalAddress::new(0x10), AccessWidth::Dword),
        Err(BusAccessError::CrossesRegion)
    ));
}

/// A device that panics under the device lock does not lock the bus for good: the panic is the
/// host's problem and is seen in the thread that panicked, the next access still works.
#[test]
fn a_panic_under_the_device_lock_does_not_wedge_the_bus() {
    let rig = rig();
    let bus = Arc::clone(&rig.bus);
    let outcome = std::thread::spawn(move || {
        let _held = bus.lock_devices();
        panic!("a device failed");
    })
    .join();
    assert!(outcome.is_err());
    assert_eq!(
        rig.bus
            .read_port_shared(PortAddress::new(LATCH_PORT), AccessWidth::Byte),
        Ok(0)
    );
}

/// The interrupt sources used by several threads at once together with the device writes that
/// move their lines: the source lock is taken before the device locks and is not taken by them,
/// so the mix finishes.
#[test]
fn sources_and_device_writes_from_several_threads_finish() {
    let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
    let pit = Pit8254Devices::new(clock.clone());
    let lapic_device = LocalApic::with_cpus(2).unwrap();
    let lapic = lapic_device.handle();
    let io_device = IoApic::new(lapic.clone());
    let (_pic_device, pic) = Pic8259::new_with_handle(Pic8259Role::Master);
    let (_uart_device, uart) = Uart16550PollingTx::new_with_handle();
    let pit_ioapic = PitIoapicIrqSource::new(
        pit.handle.clone(),
        io_device.handle(),
        lapic.clone(),
        clock.clone(),
    );
    let uart_ioapic = UartIoapicIrqSource::new(pit_ioapic, uart.clone(), io_device.handle());
    let pit_pic = PitPicIrqSource::new(pit.handle.clone(), pic.clone(), clock.clone());
    let uart_pic = UartPicIrqSource::new(pit_pic, uart, pic);
    let io = io_device.handle();
    finishes_in_time("sources and device writes", move || {
        let askers: Vec<_> = (0..4)
            .map(|i| {
                let (a, b) = (uart_ioapic.clone(), uart_pic.clone());
                std::thread::spawn(move || {
                    for _ in 0..ROUNDS {
                        if i % 2 == 0 {
                            let _ = a.pending_vector();
                            let _ = a.wake_ns();
                        } else {
                            let _ = b.pending_vector();
                            let _ = b.wake_ns();
                        }
                    }
                })
            })
            .collect();
        let writer = std::thread::spawn(move || {
            for round in 0..ROUNDS {
                let _ = io.set_intin4(round % 2 == 0);
                let _ = lapic.write_icr(0, 1, u32::from(vector_of(round)));
                let _ = lapic.take_startup();
                let _ = lapic.acknowledge();
            }
        });
        for asker in askers {
            asker.join().unwrap();
        }
        writer.join().unwrap();
    });
}
