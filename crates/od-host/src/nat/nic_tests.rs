//! The NIC model of `od-machine` (S11-32) connected to [`NatBackend`] (S11-31a) with synthetic
//! frames: the guest side is driven through the PCI function, the registers and descriptor rings
//! in guest RAM, exactly as a driver would; the backend side is the real NAT backend.
//!
//! The backend is shared with the test through a mutex (`SharedNat`): the NIC owns a box of the
//! `NetBackend` trait, while the test plays the I/O thread of the contract and calls
//! [`NatBackend::process_pending`] between the guest's steps.

use std::sync::{Arc, Mutex, MutexGuard};

use od_machine::{
    AccessWidth, Dropped, GuestPhysicalAddress, GuestPhysicalRange, MachineBus, MachineMemoryMap,
    MemoryRegionKind, MemoryRegionSpec, MmioBinding, NET_MAX_FRAME, NIC_GATEWAY_MAC,
    NIC_PCI_BAR_BASE, NIC_PCI_BAR_BYTES, NetBackend, NetCounters, NicConfig, NicConfigError,
    NicDevice, NicHandle, PciFunction, RamLimits,
};

use super::NatBackend;
use super::frame::{GATEWAY_MAC, GUEST_IP, build_arp_reply};
use super::test_frames::{GUEST_MAC, arp_request_for_gateway, echo_frame};
use std::net::Ipv4Addr;

const RAM_BYTES: u64 = 16 << 20;
const TX_RING: u64 = 0x1_0000;
const TX_BUFFERS: u64 = 0x10_0000;
const RX_RING: u64 = 0x2_0000;
const RX_BUFFERS: u64 = 0x20_0000;
const TX_DESCRIPTORS: u32 = 512;

// Register offsets (NIC_DEVICE.md section 4).
const CTRL: u64 = 0x0000;
const RCTL: u64 = 0x0100;
const TCTL: u64 = 0x0400;
const RDBAL: u64 = 0x2800;
const RDLEN: u64 = 0x2808;
const RDH: u64 = 0x2810;
const RDT: u64 = 0x2818;
const TDBAL: u64 = 0x3800;
const TDLEN: u64 = 0x3808;
const TDH: u64 = 0x3810;
const TDT: u64 = 0x3818;

#[derive(Clone)]
struct SharedNat(Arc<Mutex<NatBackend>>);

impl SharedNat {
    fn lock(&self) -> MutexGuard<'_, NatBackend> {
        self.0.lock().unwrap()
    }
}

impl NetBackend for SharedNat {
    fn transmit(&mut self, frame: &[u8]) -> Result<(), Dropped> {
        self.lock().transmit(frame)
    }

    fn poll_receive(&mut self, buf: &mut [u8]) -> Option<usize> {
        self.lock().poll_receive(buf)
    }

    fn counters(&self) -> NetCounters {
        self.0.lock().unwrap().counters()
    }

    fn reset(&mut self) {
        self.lock().reset();
    }
}

struct Machine {
    bus: MachineBus,
    nic: NicHandle,
    nat: SharedNat,
}

fn machine() -> Machine {
    let (device, nic, mut pci) = NicDevice::new_pci(NicConfig { mac: GUEST_MAC }).unwrap();
    let range =
        |start, len| GuestPhysicalRange::new(GuestPhysicalAddress::new(start), len).unwrap();
    let map = MachineMemoryMap::new(
        36,
        vec![
            MemoryRegionSpec::new(range(0, RAM_BYTES), MemoryRegionKind::Ram),
            MemoryRegionSpec::new(
                range(NIC_PCI_BAR_BASE, NIC_PCI_BAR_BYTES),
                MemoryRegionKind::Mmio,
            ),
        ],
    )
    .unwrap();
    let bus = MachineBus::new(
        map,
        RamLimits::new(RAM_BYTES),
        vec![Box::new(device)],
        vec![MmioBinding {
            region_start: GuestPhysicalAddress::new(NIC_PCI_BAR_BASE),
            device_index: 0,
        }],
        vec![],
    )
    .unwrap();
    let nat = SharedNat(Arc::new(Mutex::new(NatBackend::new(GUEST_MAC).unwrap())));
    nic.attach_backend(Box::new(nat.clone())).unwrap();
    // The driver enables memory decoding and bus mastering.
    pci.config_write(0x04, AccessWidth::Word, 0b110);
    let mut machine = Machine { bus, nic, nat };
    machine.program_rings();
    machine
}

impl Machine {
    fn reg(&mut self, offset: u64) -> u32 {
        self.bus
            .read_physical(
                GuestPhysicalAddress::new(NIC_PCI_BAR_BASE + offset),
                AccessWidth::Dword,
            )
            .unwrap() as u32
    }

    fn set_reg(&mut self, offset: u64, value: u32) {
        self.bus
            .write_physical(
                GuestPhysicalAddress::new(NIC_PCI_BAR_BASE + offset),
                AccessWidth::Dword,
                u64::from(value),
                |_| {},
            )
            .unwrap();
    }

    /// Guest RAM through the public bus accesses, byte by byte.
    fn put(&mut self, address: u64, bytes: &[u8]) {
        for (offset, byte) in bytes.iter().enumerate() {
            self.bus
                .write_physical(
                    GuestPhysicalAddress::new(address + offset as u64),
                    AccessWidth::Byte,
                    u64::from(*byte),
                    |_| {},
                )
                .unwrap();
        }
    }

    fn get(&mut self, address: u64, len: usize) -> Vec<u8> {
        (0..len as u64)
            .map(|offset| {
                self.bus
                    .read_physical(
                        GuestPhysicalAddress::new(address + offset),
                        AccessWidth::Byte,
                    )
                    .unwrap() as u8
            })
            .collect()
    }

    /// Transmit ring of 512 descriptors, receive ring of 8 with 2 KiB buffers; reception takes
    /// frames for the guest MAC and broadcasts, with the FCS stripped.
    fn program_rings(&mut self) {
        self.set_reg(TDBAL, TX_RING as u32);
        self.set_reg(TDLEN, TX_DESCRIPTORS * 16);
        self.set_reg(TDH, 0);
        self.set_reg(TDT, 0);
        self.set_reg(TCTL, 1 << 1);
        for index in 0..8u64 {
            let mut descriptor = [0u8; 16];
            descriptor[0..8].copy_from_slice(&(RX_BUFFERS + index * 0x1000).to_le_bytes());
            self.put(RX_RING + index * 16, &descriptor);
        }
        self.set_reg(RDBAL, RX_RING as u32);
        self.set_reg(RDLEN, 8 * 16);
        self.set_reg(RDH, 0);
        self.set_reg(RDT, 7);
        self.set_reg(RCTL, (1 << 1) | (1 << 15) | (1 << 26));
    }

    /// Queues `frame` in transmit descriptor `index` (EOP and RS) and advances the tail.
    fn transmit(&mut self, index: u32, frame: &[u8]) {
        let buffer = TX_BUFFERS + u64::from(index) * 0x1000;
        self.put(buffer, frame);
        let mut descriptor = [0u8; 16];
        descriptor[0..8].copy_from_slice(&buffer.to_le_bytes());
        descriptor[8..10].copy_from_slice(&(frame.len() as u16).to_le_bytes());
        descriptor[11] = 0b1001;
        self.put(TX_RING + u64::from(index) * 16, &descriptor);
        self.set_reg(TDT, (index + 1) % TX_DESCRIPTORS);
    }

    fn service(&mut self) -> od_machine::NicServiceReport {
        self.nic.service(&mut self.bus, |_| {})
    }

    fn tx_done(&mut self, index: u32) -> bool {
        self.get(TX_RING + u64::from(index) * 16 + 12, 1)[0] & 1 == 1
    }

    fn rx_length(&mut self, index: u64) -> usize {
        let descriptor = self.get(RX_RING + index * 16, 16);
        usize::from(u16::from_le_bytes([descriptor[8], descriptor[9]]))
    }
}

#[test]
fn the_nic_and_the_backend_agree_on_the_gateway_mac() {
    assert_eq!(NIC_GATEWAY_MAC, GATEWAY_MAC);
    assert_eq!(
        NicDevice::new_pci(NicConfig { mac: GATEWAY_MAC }).err(),
        Some(NicConfigError::MacIsGateway)
    );
    assert!(NatBackend::new(GATEWAY_MAC).is_err());
}

#[test]
fn an_arp_request_for_the_gateway_goes_through_the_rings_and_is_answered_in_the_rx_ring() {
    let mut machine = machine();
    machine.transmit(0, &arp_request_for_gateway());
    let report = machine.service();
    assert_eq!(report.tx_descriptors, 1);
    assert!(
        machine.tx_done(0),
        "the guest sees a completed transmission"
    );
    // The backend has the frame in its L2 queue; the I/O thread of the contract handles it.
    assert_eq!(machine.nat.lock().process_pending(), 1);
    assert_eq!(machine.service().rx_frames, 1);
    let mut expected = [0u8; 60];
    let length = build_arp_reply(GUEST_MAC, GUEST_IP_GATEWAY, GUEST_IP, &mut expected[..42])
        .expect("the buffer holds an ARP reply");
    assert_eq!(length, 42);
    assert_eq!(machine.rx_length(0), 60, "padded to the minimum frame");
    assert_eq!(machine.get(RX_BUFFERS, 60), expected);
    assert_eq!(machine.reg(RDH), 1);
    let counters = machine.nic.backend_counters().unwrap();
    assert_eq!(
        (counters.tx_frames, counters.arp_replies, counters.rx_frames),
        (1, 1, 1)
    );
    assert_eq!(machine.nic.counters().tx_frames, 1);
}

const GUEST_IP_GATEWAY: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 2);

#[test]
fn frames_the_backend_drops_complete_for_the_guest_and_are_counted_by_reason() {
    let mut machine = machine();
    // A runt, an oversize frame and a frame from a foreign source MAC.
    machine.transmit(0, &[0xAA; 10]);
    machine.transmit(1, &vec![0xBB; NET_MAX_FRAME + 100]);
    let mut foreign = arp_request_for_gateway();
    foreign[6..12].copy_from_slice(&[0x02, 0x99, 0x99, 0x99, 0x99, 0x99]);
    machine.transmit(2, &foreign);
    assert_eq!(machine.service().tx_descriptors, 3);
    assert!(machine.tx_done(0) && machine.tx_done(1) && machine.tx_done(2));
    machine.nat.lock().process_pending();
    let counters = machine.nic.backend_counters().unwrap();
    assert_eq!(counters.drop_runt, 1);
    assert_eq!(counters.drop_oversize, 1);
    assert_eq!(counters.drop_bad_src_mac, 1);
    // The NIC saw one accepted frame (the foreign-MAC one is dropped later, inside the backend).
    let nic = machine.nic.counters();
    assert_eq!((nic.tx_frames, nic.tx_backend_dropped), (1, 2));
    assert_eq!(machine.service().rx_frames, 0, "nothing to deliver");
}

#[test]
fn a_full_backend_queue_refuses_frames_without_failing_the_descriptors() {
    let mut machine = machine();
    // 300 frames with no I/O thread running: the L2 queue holds 256.
    for index in 0..300u32 {
        machine.transmit(index, &echo_frame(Ipv4Addr::new(93, 184, 216, 34)));
    }
    let mut serviced = 0;
    while serviced < 300 {
        let report = machine.service();
        assert!(report.tx_descriptors > 0, "the ring drains");
        serviced += report.tx_descriptors;
    }
    assert!((0..300).all(|index| machine.tx_done(index)));
    let counters = machine.nic.backend_counters().unwrap();
    assert_eq!(counters.tx_frames, 256);
    assert_eq!(counters.drop_tx_queue_full, 44);
    assert_eq!(machine.reg(TDH), 300);
}

#[test]
fn a_nic_reset_empties_the_backend_queues_and_keeps_its_counters() {
    let mut machine = machine();
    machine.transmit(0, &arp_request_for_gateway());
    machine.service();
    machine.set_reg(CTRL, 1 << 26);
    assert_eq!(machine.nat.lock().process_pending(), 0, "L2 was emptied");
    assert_eq!(machine.nic.backend_counters().unwrap().resets, 1);
    assert_eq!(machine.nic.backend_counters().unwrap().tx_frames, 1);
    // The rings must be programmed again after a reset.
    assert_eq!(machine.reg(TDLEN), 0);
    assert_eq!(machine.reg(RCTL), 0);
}

#[test]
fn a_reply_waits_in_the_backend_while_the_guest_has_no_receive_descriptor() {
    let mut machine = machine();
    machine.transmit(0, &arp_request_for_gateway());
    machine.service();
    // The driver has handed back no descriptor: RDT = RDH.
    machine.set_reg(RDT, 0);
    machine.nat.lock().process_pending();
    assert_eq!(machine.service().rx_frames, 0);
    assert_eq!(
        machine.nic.backend_counters().unwrap().rx_frames,
        0,
        "not taken"
    );
    machine.set_reg(RDT, 7);
    assert_eq!(machine.service().rx_frames, 1);
    assert_eq!(machine.nic.backend_counters().unwrap().rx_frames, 1);
}
