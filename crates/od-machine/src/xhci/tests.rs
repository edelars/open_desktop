//! Tests of the xHCI model (S8-42, slice A) through the PCI function and `MachineBus`, the way
//! firmware or a driver uses it: registers, doorbells, command and event rings in guest RAM,
//! virtual time. Contract: `docs/specs/XHCI_DEVICE.md`.

use super::regs;
use super::trb::{
    CC_EVENT_RING_FULL_ERROR, CC_NO_SLOTS_AVAILABLE, CC_SLOT_NOT_ENABLED, CC_SUCCESS, CC_TRB_ERROR,
    TRB_TYPE_COMMAND_COMPLETION_EVENT, TRB_TYPE_DISABLE_SLOT, TRB_TYPE_ENABLE_SLOT,
    TRB_TYPE_HOST_CONTROLLER_EVENT, TRB_TYPE_LINK, TRB_TYPE_NO_OP_COMMAND,
    TRB_TYPE_PORT_STATUS_CHANGE_EVENT,
};
use super::*;
use crate::pci_ecam::PciFunction;
use crate::{
    AccessWidth, BusAccessError, DeviceError, GuestPhysicalAddress, GuestPhysicalRange, MachineBus,
    MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, MmioBinding, RamLimits,
    VirtualClockHandle,
};

mod hid;
mod irq;
mod slot_commands;
mod transfers;

const RAM_BYTES: u64 = 16 << 20;
const CMD_RING: u64 = 0x1_0000;
/// Command ring segment: 31 command slots and a Link TRB.
const CMD_RING_TRBS: u32 = 32;
const ERST: u64 = 0x2_0000;
const EVENT_RING: u64 = 0x3_0000;
const EVENT_TRBS: u32 = 16;
const DCBAA: u64 = 0x4_0000;
/// Not mapped at all.
const UNMAPPED: u64 = 0x8000_0000;

const USBCMD: u64 = regs::USBCMD;
const USBSTS: u64 = regs::USBSTS;
const CRCR: u64 = regs::CRCR;
const DCBAAP: u64 = regs::DCBAAP;
const CONFIG: u64 = regs::CONFIG;
const IMAN: u64 = regs::IMAN;
const ERSTSZ: u64 = regs::ERSTSZ;
const ERSTBA: u64 = regs::ERSTBA;
const ERDP: u64 = regs::ERDP;
const DOORBELL_0: u64 = regs::DOORBELLS;

const RS: u32 = regs::USBCMD_RS;
const HCRST: u32 = regs::USBCMD_HCRST;
const INTE: u32 = regs::USBCMD_INTE;
const HSEE: u32 = regs::USBCMD_HSEE;
const HCH: u32 = regs::USBSTS_HCH;
const HSE: u32 = regs::USBSTS_HSE;
const EINT: u32 = regs::USBSTS_EINT;
const PCD: u32 = regs::USBSTS_PCD;
const IP: u32 = regs::IMAN_IP;
const IE: u32 = regs::IMAN_IE;
const CRR: u32 = regs::CRCR_CRR;
const EHB: u32 = regs::ERDP_EHB;

const PORTSC_PED: u32 = 1 << 1;
const PORTSC_PR: u32 = 1 << 4;
const PORTSC_CSC: u32 = 1 << 17;
const PORTSC_PRC: u32 = 1 << 21;
/// Attached, not reset: CCS, PLS = Polling (7), PP, Full-speed, CSC.
const PORTSC_INITIAL: u32 = 0x0002_06E1;
/// After a completed reset: CCS, PED, PLS = U0, PP, Full-speed, CSC, PRC.
const PORTSC_AFTER_RESET: u32 = 0x0022_0603;

const COMMAND_MSE: u32 = 1 << 1;
const COMMAND_BME: u32 = 1 << 2;
const COMMAND_INTX_DISABLE: u32 = 1 << 10;

fn portsc(port: u64) -> u64 {
    regs::PORT_BASE + regs::PORT_STRIDE * (port - 1)
}

fn doorbell(index: u64) -> u64 {
    regs::DOORBELLS + 4 * index
}

/// One event TRB read back from the event ring.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Event {
    parameter: u64,
    code: u32,
    slot_id: u32,
    kind: u32,
    cycle: bool,
    dw0: u32,
    raw: [u32; 4],
}

impl Event {
    fn parse(dw: [u32; 4]) -> Self {
        Self {
            parameter: u64::from(dw[0]) | (u64::from(dw[1]) << 32),
            code: dw[2] >> 24,
            slot_id: dw[3] >> 24,
            kind: (dw[3] >> 10) & 0x3F,
            cycle: dw[3] & 1 != 0,
            dw0: dw[0],
            raw: dw,
        }
    }

    fn port_id(&self) -> u32 {
        self.dw0 >> 24
    }
}

fn command_trb(kind: u32, slot_id: u32) -> [u32; 4] {
    [0, 0, 0, (slot_id << 24) | (kind << 10)]
}

fn enable_slot() -> [u32; 4] {
    command_trb(TRB_TYPE_ENABLE_SLOT, 0)
}

fn disable_slot(slot_id: u32) -> [u32; 4] {
    command_trb(TRB_TYPE_DISABLE_SLOT, slot_id)
}

fn no_op() -> [u32; 4] {
    command_trb(TRB_TYPE_NO_OP_COMMAND, 0)
}

struct Rig {
    bus: MachineBus,
    pci: XhciPciFunction,
    xhci: XhciHandle,
    clock: VirtualClockHandle,
    cmd_index: u32,
    cmd_cycle: bool,
    ev_size: u32,
    ev_index: u32,
    ev_cycle: bool,
}

fn rig_with(config: XhciConfig) -> Rig {
    let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
    let (device, xhci, pci) = XhciDevice::new_pci(config, clock.clone());
    let range =
        |start, len| GuestPhysicalRange::new(GuestPhysicalAddress::new(start), len).unwrap();
    let map = MachineMemoryMap::new(
        36,
        vec![
            MemoryRegionSpec::new(range(0, RAM_BYTES), MemoryRegionKind::Ram),
            MemoryRegionSpec::new(
                range(XHCI_PCI_BAR_BASE, XHCI_PCI_BAR_BYTES),
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
            region_start: GuestPhysicalAddress::new(XHCI_PCI_BAR_BASE),
            device_index: 0,
        }],
        vec![],
    )
    .unwrap();
    Rig {
        bus,
        pci,
        xhci,
        clock,
        cmd_index: 0,
        cmd_cycle: true,
        ev_size: EVENT_TRBS,
        ev_index: 0,
        ev_cycle: true,
    }
}

/// PCI Command is still 0: the function does not decode memory or master the bus.
fn bare_rig() -> Rig {
    rig_with(XhciConfig::default())
}

/// Memory decoding and bus mastering enabled, controller stopped and unprogrammed.
fn rig() -> Rig {
    let mut rig = bare_rig();
    rig.cfg_write(0x04, AccessWidth::Word, COMMAND_MSE | COMMAND_BME);
    rig
}

/// Memory decoding and bus mastering enabled, controller running with rings programmed, port
/// events consumed and cleared.
fn running_rig() -> Rig {
    let mut rig = rig();
    rig.start();
    rig
}

impl Rig {
    fn address(offset: u64) -> GuestPhysicalAddress {
        GuestPhysicalAddress::new(XHCI_PCI_BAR_BASE + offset)
    }

    fn read(&mut self, offset: u64, width: AccessWidth) -> Result<u64, BusAccessError> {
        self.bus.read_physical(Self::address(offset), width)
    }

    fn write(&mut self, offset: u64, width: AccessWidth, value: u64) -> Result<(), BusAccessError> {
        self.bus
            .write_physical(Self::address(offset), width, value, |_| {})
    }

    fn device_error<T: std::fmt::Debug>(result: Result<T, BusAccessError>) -> DeviceError {
        match result {
            Err(BusAccessError::Device { source, .. }) => source,
            other => panic!("expected a device error, got {other:?}"),
        }
    }

    fn dword(&mut self, offset: u64) -> u32 {
        self.read(offset, AccessWidth::Dword).unwrap() as u32
    }

    fn set_dword(&mut self, offset: u64, value: u32) {
        self.write(offset, AccessWidth::Dword, u64::from(value))
            .unwrap();
    }

    fn qword(&mut self, offset: u64) -> u64 {
        self.read(offset, AccessWidth::Qword).unwrap()
    }

    fn set_qword(&mut self, offset: u64, value: u64) {
        self.write(offset, AccessWidth::Qword, value).unwrap();
    }

    fn cfg_read(&mut self, register: u16, width: AccessWidth) -> u32 {
        self.pci.config_read(register, width)
    }

    fn cfg_write(&mut self, register: u16, width: AccessWidth, value: u32) {
        self.pci.config_write(register, width, value);
    }

    fn mem_write(&mut self, address: u64, bytes: &[u8]) {
        self.bus
            .write_flat_bytes(GuestPhysicalAddress::new(address), bytes, |_| {})
            .unwrap();
    }

    fn mem_read(&mut self, address: u64, len: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; len];
        self.bus
            .read_flat_bytes(GuestPhysicalAddress::new(address), &mut bytes)
            .unwrap();
        bytes
    }

    fn put_trb(&mut self, address: u64, dw: [u32; 4]) {
        let mut raw = [0u8; 16];
        for (index, chunk) in raw.chunks_exact_mut(4).enumerate() {
            chunk.copy_from_slice(&dw[index].to_le_bytes());
        }
        self.mem_write(address, &raw);
    }

    fn read_trb(&mut self, address: u64) -> [u32; 4] {
        let raw = self.mem_read(address, 16);
        let mut dw = [0u32; 4];
        for (index, chunk) in raw.chunks_exact(4).enumerate() {
            dw[index] = u32::from_le_bytes(chunk.try_into().unwrap());
        }
        dw
    }

    fn service(&mut self) -> XhciServiceReport {
        self.xhci.service(&mut self.bus, |_| {})
    }

    fn advance_ns(&self, ns: u64) {
        self.clock.advance_standalone(ns).unwrap();
    }

    fn usbsts(&mut self) -> u32 {
        self.dword(USBSTS)
    }

    fn iman(&mut self) -> u32 {
        self.dword(IMAN)
    }

    /// One ERST entry at [`ERST`] describing the segment, programmed while stopped.
    fn program_event_ring_at(&mut self, erst: u64, base: u64, size: u32) {
        let mut entry = [0u8; 16];
        entry[0..8].copy_from_slice(&base.to_le_bytes());
        entry[8..12].copy_from_slice(&size.to_le_bytes());
        // A driver hands the controller a zeroed segment.
        if size > 0 && base < RAM_BYTES - u64::from(size) * 16 {
            self.mem_write(base, &vec![0u8; size as usize * 16]);
        }
        self.mem_write(erst, &entry);
        self.set_dword(ERSTSZ, 1);
        self.set_qword(ERSTBA, erst);
        self.set_qword(ERDP, base);
        self.ev_size = size;
        self.ev_index = 0;
        self.ev_cycle = true;
    }

    fn program_event_ring(&mut self) {
        self.program_event_ring_at(ERST, EVENT_RING, EVENT_TRBS);
    }

    fn program_command_ring(&mut self) {
        self.mem_write(CMD_RING, &vec![0u8; CMD_RING_TRBS as usize * 16]);
        self.set_qword(CRCR, CMD_RING | 1);
        self.cmd_index = 0;
        self.cmd_cycle = true;
    }

    /// Programs rings, DCBAA and CONFIG, sets IE and starts the controller; one service call
    /// loads the event ring and publishes the two Port Status Change Events.
    fn begin_with(&mut self, max_slots: u32) {
        self.program_event_ring();
        self.program_command_ring();
        self.set_qword(DCBAAP, DCBAA);
        self.set_dword(CONFIG, max_slots);
        self.set_dword(IMAN, IE);
        self.set_dword(USBCMD, RS | INTE);
        self.service();
    }

    fn begin(&mut self) {
        self.begin_with(8);
    }

    /// `begin`, then the two connect events are consumed, the change bits, IP and EINT cleared.
    fn start_with(&mut self, max_slots: u32) {
        self.begin_with(max_slots);
        assert_eq!(self.events().len(), 2);
        for port in 1..=2 {
            self.set_dword(portsc(port), PORTSC_CSC);
        }
        self.set_dword(IMAN, IP | IE);
        self.set_dword(USBSTS, EINT);
    }

    fn start(&mut self) {
        self.start_with(8);
    }

    /// Events written since the last call, in ring order; ERDP is moved past them (EHB cleared).
    fn events(&mut self) -> Vec<Event> {
        let events = self.peek_events();
        self.acknowledge_events();
        events
    }

    /// Events written since the last call; ERDP is not moved.
    fn peek_events(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        loop {
            let raw = self.read_trb(EVENT_RING + u64::from(self.ev_index) * 16);
            if (raw[3] & 1 != 0) != self.ev_cycle {
                return events;
            }
            events.push(Event::parse(raw));
            self.ev_index += 1;
            if self.ev_index == self.ev_size {
                self.ev_index = 0;
                self.ev_cycle = !self.ev_cycle;
            }
        }
    }

    fn acknowledge_events(&mut self) {
        let pointer = EVENT_RING + u64::from(self.ev_index) * 16;
        self.set_qword(ERDP, pointer | u64::from(EHB));
    }

    /// Writes a command TRB with the current producer cycle bit; a Link TRB with Toggle Cycle
    /// closes the segment. Returns the TRB address.
    fn push_command(&mut self, mut trb: [u32; 4]) -> u64 {
        if self.cmd_index == CMD_RING_TRBS - 1 {
            let link = [
                CMD_RING as u32,
                (CMD_RING >> 32) as u32,
                0,
                (TRB_TYPE_LINK << 10) | 2 | u32::from(self.cmd_cycle),
            ];
            self.put_trb(CMD_RING + u64::from(self.cmd_index) * 16, link);
            self.cmd_index = 0;
            self.cmd_cycle = !self.cmd_cycle;
        }
        let address = CMD_RING + u64::from(self.cmd_index) * 16;
        trb[3] = (trb[3] & !1) | u32::from(self.cmd_cycle);
        self.put_trb(address, trb);
        self.cmd_index += 1;
        address
    }

    fn ring_command_doorbell(&mut self) {
        self.set_dword(DOORBELL_0, 0);
    }

    /// Submits one command, rings doorbell 0, services and returns its single completion event.
    fn command(&mut self, trb: [u32; 4]) -> Event {
        let address = self.push_command(trb);
        self.ring_command_doorbell();
        self.service();
        let events = self.events();
        assert_eq!(events.len(), 1, "one command, one event: {events:?}");
        let event = events[0];
        assert_eq!(event.kind, TRB_TYPE_COMMAND_COMPLETION_EVENT);
        assert_eq!(event.parameter, address);
        event
    }

    fn enable_slot(&mut self) -> Event {
        self.command(enable_slot())
    }
}

// ---------------------------------------------------------------- capability registers

#[test]
fn capability_registers_have_contract_values() {
    let mut rig = rig();
    assert_eq!(
        rig.read(0x00, AccessWidth::Byte).unwrap(),
        0x20,
        "CAPLENGTH"
    );
    assert_eq!(
        rig.read(0x02, AccessWidth::Word).unwrap(),
        0x0100,
        "HCIVERSION"
    );
    assert_eq!(rig.dword(0x00), 0x0100_0020);
    assert_eq!(
        rig.dword(0x04),
        0x0200_0108,
        "HCSPARAMS1: 8 slots, 1 interrupter, 2 ports"
    );
    assert_eq!(rig.dword(0x08), 0, "HCSPARAMS2");
    assert_eq!(rig.dword(0x0C), 0, "HCSPARAMS3");
    assert_eq!(rig.dword(0x10), 0x0800_0001, "HCCPARAMS1: AC64, xECP 0x800");
    assert_eq!(rig.dword(0x14), 0x0000_0800, "DBOFF");
    assert_eq!(rig.dword(0x18), 0x0000_1000, "RTSOFF");
    assert_eq!(rig.dword(0x1C), 0, "HCCPARAMS2");
}

#[test]
fn capability_registers_read_in_bytes_words_and_dwords() {
    let mut rig = rig();
    assert_eq!(rig.read(0x01, AccessWidth::Byte).unwrap(), 0x00);
    assert_eq!(rig.read(0x03, AccessWidth::Byte).unwrap(), 0x01);
    assert_eq!(rig.read(0x00, AccessWidth::Word).unwrap(), 0x0020);
    assert_eq!(rig.read(0x04, AccessWidth::Byte).unwrap(), 0x08);
    assert_eq!(rig.read(0x07, AccessWidth::Byte).unwrap(), 0x02);
    assert_eq!(rig.read(0x06, AccessWidth::Word).unwrap(), 0x0200);
    assert_eq!(rig.read(0x12, AccessWidth::Word).unwrap(), 0x0800);
}

#[test]
fn capability_qword_read_is_an_unsupported_width() {
    let mut rig = rig();
    assert_eq!(
        Rig::device_error(rig.read(0x00, AccessWidth::Qword)),
        DeviceError::UnsupportedWidth
    );
}

#[test]
fn capability_unaligned_reads_are_misaligned() {
    let mut rig = rig();
    assert_eq!(
        Rig::device_error(rig.read(0x01, AccessWidth::Word)),
        DeviceError::Misaligned
    );
    assert_eq!(
        Rig::device_error(rig.read(0x06, AccessWidth::Dword)),
        DeviceError::Misaligned
    );
}

#[test]
fn capability_registers_ignore_writes() {
    let mut rig = rig();
    for offset in [0x00, 0x04, 0x08, 0x0C, 0x10, 0x14, 0x18, 0x1C] {
        let before = rig.dword(offset);
        rig.set_dword(offset, 0xFFFF_FFFF);
        assert_eq!(rig.dword(offset), before, "offset {offset:#x}");
    }
}

// ---------------------------------------------------------------- operational registers

#[test]
fn operational_registers_reset_to_contract_values() {
    let mut rig = rig();
    assert_eq!(rig.dword(USBCMD), 0);
    assert_eq!(rig.dword(USBSTS), 1, "HCH = 1");
    assert_eq!(rig.dword(regs::PAGESIZE), 1, "4 KiB");
    assert_eq!(rig.dword(regs::DNCTRL), 0);
    assert_eq!(rig.qword(CRCR), 0);
    assert_eq!(rig.qword(DCBAAP), 0);
    assert_eq!(rig.dword(CONFIG), 0);
    assert_eq!(rig.dword(IMAN), 0);
    assert_eq!(rig.dword(regs::IMOD), 0x0FA0);
    assert_eq!(rig.dword(ERSTSZ), 0);
    assert_eq!(rig.qword(ERSTBA), 0);
    assert_eq!(rig.qword(ERDP), 0);
    assert_eq!(rig.dword(regs::MFINDEX), 0);
}

#[test]
fn non_capability_registers_reject_byte_and_word_reads() {
    let mut rig = rig();
    for offset in [
        USBCMD,
        USBSTS,
        regs::PAGESIZE,
        portsc(1),
        doorbell(0),
        regs::MFINDEX,
        0x2000,
    ] {
        assert_eq!(
            Rig::device_error(rig.read(offset, AccessWidth::Byte)),
            DeviceError::UnsupportedWidth,
            "byte at {offset:#x}"
        );
        assert_eq!(
            Rig::device_error(rig.read(offset, AccessWidth::Word)),
            DeviceError::UnsupportedWidth,
            "word at {offset:#x}"
        );
    }
}

#[test]
fn writes_of_one_or_two_bytes_fail_without_changing_state() {
    let mut rig = rig();
    rig.set_dword(regs::DNCTRL, 0x1234);
    assert_eq!(
        Rig::device_error(rig.write(regs::DNCTRL, AccessWidth::Byte, 0xFF)),
        DeviceError::UnsupportedWidth
    );
    assert_eq!(
        Rig::device_error(rig.write(regs::DNCTRL, AccessWidth::Word, 0xFF)),
        DeviceError::UnsupportedWidth
    );
    assert_eq!(
        Rig::device_error(rig.write(0x00, AccessWidth::Byte, 0xFF)),
        DeviceError::UnsupportedWidth
    );
    assert_eq!(rig.dword(regs::DNCTRL), 0x1234);
}

#[test]
fn qword_write_to_a_dword_register_fails_without_changing_state() {
    let mut rig = rig();
    rig.set_dword(regs::DNCTRL, 0x1234);
    assert_eq!(
        Rig::device_error(rig.write(regs::DNCTRL, AccessWidth::Qword, u64::MAX)),
        DeviceError::UnsupportedWidth
    );
    assert_eq!(
        Rig::device_error(rig.write(CONFIG, AccessWidth::Qword, 8)),
        DeviceError::UnsupportedWidth
    );
    assert_eq!(rig.dword(regs::DNCTRL), 0x1234);
    assert_eq!(rig.dword(CONFIG), 0);
}

#[test]
fn qword_read_of_a_dword_register_is_an_unsupported_width() {
    let mut rig = rig();
    for offset in [USBCMD, regs::DNCTRL, CONFIG, portsc(1), IMAN, 0x2000] {
        assert_eq!(
            Rig::device_error(rig.read(offset, AccessWidth::Qword)),
            DeviceError::UnsupportedWidth,
            "offset {offset:#x}"
        );
    }
}

#[test]
fn unaligned_register_accesses_are_misaligned() {
    let mut rig = rig();
    assert_eq!(
        Rig::device_error(rig.read(USBCMD + 2, AccessWidth::Dword)),
        DeviceError::Misaligned
    );
    assert_eq!(
        Rig::device_error(rig.write(regs::DNCTRL + 1, AccessWidth::Dword, 1)),
        DeviceError::Misaligned
    );
    // The upper half of CRCR is not 8-byte aligned.
    assert_eq!(
        Rig::device_error(rig.read(CRCR + 4, AccessWidth::Qword)),
        DeviceError::Misaligned
    );
    assert_eq!(
        Rig::device_error(rig.write(DCBAAP + 4, AccessWidth::Qword, 1)),
        DeviceError::Misaligned
    );
}

#[test]
fn sixty_four_bit_registers_work_as_two_dwords_or_one_qword() {
    let mut rig = rig();
    rig.set_dword(DCBAAP, 0x1234_5678);
    rig.set_dword(DCBAAP + 4, 0x9);
    assert_eq!(rig.qword(DCBAAP), 0x9_1234_5640, "bits 5:0 read as zero");
    assert_eq!(rig.dword(DCBAAP), 0x1234_5640);
    assert_eq!(rig.dword(DCBAAP + 4), 0x9);
    rig.set_qword(DCBAAP, 0x0000_0002_0000_1FFF);
    assert_eq!(rig.qword(DCBAAP), 0x0000_0002_0000_1FC0);
}

#[test]
fn undefined_offsets_read_zero_and_ignore_writes() {
    let mut rig = rig();
    for offset in [
        0x2C, 0x40, 0x5C, 0x400, 0x440, 0x824, 0xFFC, 0x1040, 0x2010, 0x3FFC,
    ] {
        assert_eq!(rig.dword(offset), 0, "read {offset:#x}");
        rig.set_dword(offset, 0xFFFF_FFFF);
        assert_eq!(rig.dword(offset), 0, "written {offset:#x}");
    }
    assert_eq!(rig.dword(USBCMD), 0);
    assert_eq!(rig.dword(CONFIG), 0);
}

#[test]
fn read_only_registers_ignore_writes() {
    let mut rig = rig();
    rig.set_dword(regs::PAGESIZE, 0xFFFF_FFFF);
    assert_eq!(rig.dword(regs::PAGESIZE), 1);
    rig.set_dword(regs::MFINDEX, 0x1234);
    assert_eq!(rig.dword(regs::MFINDEX), 0);
}

#[test]
fn usbcmd_implements_only_run_stop_hcrst_inte_and_hsee() {
    let mut rig = rig();
    rig.set_dword(USBCMD, 0xFFFF_FFFD);
    assert_eq!(rig.dword(USBCMD), RS | INTE | HSEE);
    rig.set_dword(USBCMD, 0);
    assert_eq!(rig.dword(USBCMD), 0);
    rig.set_dword(USBCMD, INTE);
    assert_eq!(rig.dword(USBCMD), INTE);
}

#[test]
fn hcrst_bit_reads_back_as_zero() {
    let mut rig = rig();
    rig.set_dword(USBCMD, HCRST | RS | INTE);
    assert_eq!(
        rig.dword(USBCMD),
        0,
        "HCRST self-clears and takes the rest of the write"
    );
    assert_eq!(rig.usbsts(), HCH);
}

#[test]
fn run_stop_sets_and_clears_hch_in_the_same_access() {
    let mut rig = rig();
    assert_eq!(rig.usbsts() & HCH, HCH);
    rig.set_dword(USBCMD, RS);
    assert_eq!(rig.usbsts() & HCH, 0);
    rig.set_dword(USBCMD, 0);
    assert_eq!(rig.usbsts() & HCH, HCH);
}

#[test]
fn stopping_clears_crr_and_drops_a_pending_command_doorbell() {
    let mut rig = rig();
    rig.begin();
    rig.push_command(no_op());
    rig.ring_command_doorbell();
    assert_eq!(rig.dword(CRCR), CRR, "the first doorbell sets CRR");
    rig.set_dword(USBCMD, 0);
    assert_eq!(rig.dword(CRCR), 0);
    assert!(!rig.xhci.service_pending());
    assert_eq!(rig.peek_events().len(), 2, "the two port events");
    rig.set_dword(USBCMD, RS);
    rig.service();
    assert!(
        rig.peek_events().is_empty(),
        "the stopped doorbell is not replayed"
    );
}

#[test]
fn usbsts_hch_and_reserved_bits_ignore_writes() {
    let mut rig = rig();
    rig.set_dword(USBSTS, 0xFFFF_FFFF);
    assert_eq!(rig.usbsts(), HCH);
    rig.set_dword(USBCMD, RS);
    rig.set_dword(USBSTS, HCH);
    assert_eq!(rig.usbsts(), 0);
}

#[test]
fn crcr_reads_only_crr_and_ignores_cs_and_ca() {
    let mut rig = rig();
    rig.set_qword(CRCR, CMD_RING | 1 | 2 | 4);
    assert_eq!(rig.qword(CRCR), 0, "pointer, RCS, CS and CA read as zero");
    rig.begin();
    rig.push_command(no_op());
    rig.ring_command_doorbell();
    assert_eq!(rig.qword(CRCR), u64::from(CRR));
    rig.set_qword(CRCR, 4);
    assert_eq!(rig.qword(CRCR), u64::from(CRR), "CA does nothing");
}

#[test]
fn crcr_write_is_ignored_while_crr_is_set() {
    let mut rig = running_rig();
    rig.ring_command_doorbell();
    assert_eq!(rig.dword(CRCR), CRR);
    rig.set_qword(CRCR, 0x7_0000 | 1);
    let event = rig.command(no_op());
    assert_eq!(
        event.code, CC_SUCCESS,
        "the ring continues at the old pointer"
    );
}

#[test]
fn dcbaap_config_erstsz_and_erstba_are_ignored_while_running() {
    let mut rig = running_rig();
    rig.set_qword(DCBAAP, 0x7_0000);
    assert_eq!(rig.qword(DCBAAP), DCBAA);
    rig.set_dword(CONFIG, 2);
    assert_eq!(rig.dword(CONFIG), 8);
    rig.set_dword(ERSTSZ, 0);
    assert_eq!(rig.dword(ERSTSZ), 1);
    rig.set_qword(ERSTBA, 0x7_0000);
    assert_eq!(rig.qword(ERSTBA), ERST);
}

#[test]
fn erstsz_above_one_is_ignored() {
    let mut rig = rig();
    rig.set_dword(ERSTSZ, 2);
    assert_eq!(rig.dword(ERSTSZ), 0);
    rig.set_dword(ERSTSZ, 0xFFFF);
    assert_eq!(rig.dword(ERSTSZ), 0);
    rig.set_dword(ERSTSZ, 1);
    assert_eq!(rig.dword(ERSTSZ), 1);
    rig.set_dword(ERSTSZ, 0);
    assert_eq!(rig.dword(ERSTSZ), 0);
}

#[test]
fn erstba_keeps_bits_63_to_6() {
    let mut rig = rig();
    rig.set_qword(ERSTBA, 0xFFFF_FFFF_FFFF_FFFF);
    assert_eq!(rig.qword(ERSTBA), 0xFFFF_FFFF_FFFF_FFC0);
}

#[test]
fn config_keeps_the_low_byte_only() {
    let mut rig = rig();
    rig.set_dword(CONFIG, 0x3FF);
    assert_eq!(rig.dword(CONFIG), 0xFF, "U3E and CIE are read-only zero");
}

#[test]
fn dnctrl_stores_the_low_16_bits() {
    let mut rig = rig();
    rig.set_dword(regs::DNCTRL, 0xFFFF_FFFF);
    assert_eq!(rig.dword(regs::DNCTRL), 0xFFFF);
}

#[test]
fn iman_ie_is_read_write_and_imod_is_stored() {
    let mut rig = rig();
    rig.set_dword(IMAN, IE);
    assert_eq!(rig.iman(), IE);
    rig.set_dword(IMAN, 0);
    assert_eq!(rig.iman(), 0);
    rig.set_dword(regs::IMOD, 0x0123_4567);
    assert_eq!(rig.dword(regs::IMOD), 0x0123_4567);
}

#[test]
fn mfindex_counts_microframes_of_virtual_time() {
    let mut rig = rig();
    assert_eq!(rig.dword(regs::MFINDEX), 0);
    rig.advance_ns(124_999);
    assert_eq!(rig.dword(regs::MFINDEX), 0);
    rig.advance_ns(1);
    assert_eq!(rig.dword(regs::MFINDEX), 1);
    rig.advance_ns(250_000);
    assert_eq!(rig.dword(regs::MFINDEX), 3);
    rig.advance_ns(0x4000 * 125_000 - 3 * 125_000);
    assert_eq!(rig.dword(regs::MFINDEX), 0, "14 bits wrap");
}

#[test]
fn extended_capability_is_the_usb2_supported_protocol() {
    let mut rig = rig();
    assert_eq!(rig.dword(0x2000), 0x0200_0002);
    assert_eq!(rig.dword(0x2004), 0x2042_5355, "\"USB \"");
    assert_eq!(rig.dword(0x2008), 0x0000_0201);
    assert_eq!(rig.dword(0x200C), 0);
    for offset in (0x2010..0x4000).step_by(0x100) {
        assert_eq!(rig.dword(offset), 0, "offset {offset:#x}");
    }
}

// ---------------------------------------------------------------- doorbells

#[test]
fn doorbells_read_zero() {
    let mut rig = running_rig();
    for index in 0..=8 {
        assert_eq!(rig.dword(doorbell(index)), 0);
    }
}

#[test]
fn doorbell_while_halted_is_ignored_and_counted() {
    let mut rig = rig();
    rig.set_dword(doorbell(0), 0);
    assert_eq!(rig.xhci.ignored_doorbells(), 1);
    assert_eq!(rig.dword(CRCR), 0, "CRR stays clear");
    assert!(!rig.xhci.service_pending());
}

#[test]
fn doorbell_zero_with_a_target_is_ignored_and_counted() {
    let mut rig = running_rig();
    rig.set_dword(doorbell(0), 1);
    assert_eq!(rig.xhci.ignored_doorbells(), 1);
    assert_eq!(rig.dword(CRCR), 0);
}

#[test]
fn slot_doorbell_for_a_disabled_slot_is_ignored_and_counted() {
    let mut rig = running_rig();
    rig.set_dword(doorbell(1), 1);
    assert_eq!(rig.xhci.ignored_doorbells(), 1);
    assert!(!rig.xhci.service_pending());
}

#[test]
fn slot_doorbell_for_a_dci_other_than_1_and_3_is_ignored_and_counted() {
    let mut rig = running_rig();
    rig.enable_slot();
    for dci in [0, 2, 4, 31, 32] {
        rig.set_dword(doorbell(1), dci);
    }
    assert_eq!(rig.xhci.ignored_doorbells(), 5);
    assert!(!rig.xhci.service_pending());
}

#[test]
fn slot_doorbell_with_a_stream_id_is_ignored_and_counted() {
    let mut rig = running_rig();
    rig.enable_slot();
    rig.set_dword(doorbell(1), (1 << 16) | 1);
    assert_eq!(rig.xhci.ignored_doorbells(), 1);
}

#[test]
fn slot_doorbell_for_dci_1_and_3_of_a_slot_that_is_only_enabled_is_ignored() {
    // The endpoints are Disabled until Address Device: the doorbell is ignored.
    let mut rig = running_rig();
    rig.enable_slot();
    rig.set_dword(doorbell(1), 1);
    rig.set_dword(doorbell(1), 3);
    assert_eq!(rig.xhci.ignored_doorbells(), 2);
    assert!(!rig.xhci.service_pending());
}

#[test]
fn command_doorbell_is_remembered_without_bus_master_and_runs_after_it_is_enabled() {
    let mut rig = rig();
    rig.begin();
    rig.push_command(no_op());
    let before = rig.xhci.ram_accesses();
    rig.cfg_write(0x04, AccessWidth::Word, COMMAND_MSE);
    rig.ring_command_doorbell();
    assert!(!rig.xhci.service_pending(), "no BME: nothing to service");
    assert_eq!(rig.service(), XhciServiceReport::default());
    assert_eq!(
        rig.xhci.ram_accesses(),
        before,
        "no BME: no guest RAM access"
    );
    assert_eq!(rig.dword(CRCR), CRR);
    rig.cfg_write(0x04, AccessWidth::Word, COMMAND_MSE | COMMAND_BME);
    assert!(rig.xhci.service_pending());
    let report = rig.service();
    assert_eq!(report.commands, 1);
    let events = rig.events();
    assert_eq!(
        events.last().unwrap().kind,
        TRB_TYPE_COMMAND_COMPLETION_EVENT
    );
}

// ---------------------------------------------------------------- PCI function

#[test]
fn pci_identity_and_class_follow_the_contract() {
    let mut rig = bare_rig();
    assert_eq!(rig.cfg_read(0x00, AccessWidth::Dword), 0x0010_D1A6);
    assert_eq!(
        rig.cfg_read(0x08, AccessWidth::Dword),
        0x0C03_3001,
        "class 0C0330, revision 1"
    );
    assert_eq!(
        rig.cfg_read(0x0E, AccessWidth::Byte),
        0,
        "header type 0, single function"
    );
    assert_eq!(rig.cfg_read(0x2C, AccessWidth::Dword), 0, "subsystem IDs");
    assert_eq!(
        rig.cfg_read(0x04, AccessWidth::Dword),
        0,
        "Command and Status reset to 0"
    );
}

#[test]
fn pci_has_no_capability_list_and_uses_inta() {
    let mut rig = bare_rig();
    assert_eq!(
        rig.cfg_read(0x34, AccessWidth::Byte),
        0,
        "Capabilities Pointer"
    );
    assert_eq!(
        rig.cfg_read(0x06, AccessWidth::Word) & (1 << 4),
        0,
        "Status.Capabilities List"
    );
    assert_eq!(
        rig.cfg_read(0x3D, AccessWidth::Byte),
        1,
        "Interrupt Pin = INTA"
    );
}

#[test]
fn bar0_is_a_64_bit_16_kib_non_prefetchable_window_at_fc008000() {
    let mut rig = bare_rig();
    assert_eq!(rig.cfg_read(0x10, AccessWidth::Dword), 0xFC00_8004);
    assert_eq!(rig.cfg_read(0x14, AccessWidth::Dword), 0);
    rig.cfg_write(0x10, AccessWidth::Dword, 0xFFFF_FFFF);
    assert_eq!(
        rig.cfg_read(0x10, AccessWidth::Dword),
        0xFFFF_C004,
        "16 KiB sizing mask"
    );
    for register in [0x18, 0x1C, 0x20, 0x24] {
        assert_eq!(
            rig.cfg_read(register, AccessWidth::Dword),
            0,
            "other BARs are read-only 0"
        );
    }
    assert_eq!(XHCI_PCI_BAR_BASE, 0xFC00_8000);
    assert_eq!(XHCI_PCI_BAR_BYTES, 0x4000);
    assert_eq!(XHCI_PCI_SLOT, 5);
    assert_eq!(XHCI_INTX_GSI, 17);
}

#[test]
fn without_memory_space_enable_mmio_reads_all_ones_and_writes_are_dropped() {
    let mut rig = bare_rig();
    assert_eq!(rig.dword(0x00), 0xFFFF_FFFF);
    assert_eq!(rig.read(0x00, AccessWidth::Byte).unwrap(), 0xFF);
    assert_eq!(rig.qword(CRCR), u64::MAX);
    rig.set_dword(regs::DNCTRL, 0x55);
    rig.cfg_write(0x04, AccessWidth::Word, COMMAND_MSE);
    assert_eq!(
        rig.dword(regs::DNCTRL),
        0,
        "the write while MSE = 0 was dropped"
    );
    assert_eq!(rig.dword(0x00), 0x0100_0020);
}

#[test]
fn moving_bar0_turns_decoding_off_and_restoring_it_turns_it_on() {
    let mut rig = rig();
    assert_eq!(rig.dword(0x00), 0x0100_0020);
    rig.cfg_write(0x10, AccessWidth::Dword, 0xFC01_0000);
    assert_eq!(rig.dword(0x00), 0xFFFF_FFFF, "the model does not move");
    rig.cfg_write(0x10, AccessWidth::Dword, 0xFC00_8000);
    assert_eq!(rig.dword(0x00), 0x0100_0020);
}

#[test]
fn io_space_enable_is_not_implemented() {
    let mut rig = bare_rig();
    rig.cfg_write(0x04, AccessWidth::Word, 0x1);
    assert_eq!(rig.cfg_read(0x04, AccessWidth::Word) & 1, 0);
}

#[test]
fn intx_line_needs_ip_ie_and_inte() {
    let mut rig = running_rig();
    assert!(!rig.xhci.sample_pci_interrupts());
    rig.command(no_op());
    assert_eq!(rig.iman(), IP | IE);
    assert!(rig.xhci.sample_pci_interrupts());
    rig.set_dword(IMAN, IP);
    assert!(!rig.xhci.sample_pci_interrupts(), "IE cleared, IP cleared");
    rig.set_dword(IMAN, IE);
    rig.command(no_op());
    rig.set_dword(USBCMD, 0);
    rig.set_dword(USBCMD, RS);
    assert!(!rig.xhci.sample_pci_interrupts(), "INTE = 0 masks the line");
    rig.set_dword(USBCMD, RS | INTE);
    assert!(rig.xhci.sample_pci_interrupts());
}

#[test]
fn writing_one_to_iman_ip_drops_the_line_and_eint_clears_separately() {
    let mut rig = running_rig();
    rig.command(no_op());
    assert!(rig.xhci.sample_pci_interrupts());
    assert_eq!(rig.usbsts() & EINT, EINT);
    rig.set_dword(IMAN, IP | IE);
    assert!(!rig.xhci.sample_pci_interrupts());
    assert_eq!(rig.usbsts() & EINT, EINT, "EINT is independent of IP");
    rig.set_dword(USBSTS, EINT);
    assert_eq!(rig.usbsts() & EINT, 0);
}

#[test]
fn command_intx_disable_masks_the_line_and_status_interrupt_follows_it() {
    let mut rig = running_rig();
    rig.command(no_op());
    let status = |rig: &mut Rig| rig.cfg_read(0x06, AccessWidth::Word) & (1 << 3);
    assert_eq!(
        status(&mut rig),
        1 << 3,
        "Status.Interrupt Status mirrors the line"
    );
    rig.cfg_write(
        0x04,
        AccessWidth::Word,
        COMMAND_MSE | COMMAND_BME | COMMAND_INTX_DISABLE,
    );
    assert!(!rig.xhci.sample_pci_interrupts());
    assert_eq!(status(&mut rig), 0);
    rig.cfg_write(0x04, AccessWidth::Word, COMMAND_MSE | COMMAND_BME);
    assert!(rig.xhci.sample_pci_interrupts());
    assert_eq!(status(&mut rig), 1 << 3);
}

// ---------------------------------------------------------------- HCRST

/// Every register HCRST returns to its reset value, read the way a driver reads it.
fn assert_registers_reset(rig: &mut Rig) {
    assert_eq!(rig.dword(USBCMD), 0, "USBCMD");
    assert_eq!(rig.usbsts(), HCH, "USBSTS");
    assert_eq!(rig.qword(CRCR), 0, "CRCR");
    assert_eq!(rig.qword(DCBAAP), 0, "DCBAAP");
    assert_eq!(rig.dword(CONFIG), 0, "CONFIG");
    assert_eq!(rig.dword(regs::DNCTRL), 0, "DNCTRL");
    assert_eq!(rig.iman(), 0, "IMAN");
    assert_eq!(rig.dword(regs::IMOD), 0x0FA0, "IMOD");
    assert_eq!(rig.dword(ERSTSZ), 0, "ERSTSZ");
    assert_eq!(rig.qword(ERSTBA), 0, "ERSTBA");
    assert_eq!(rig.qword(ERDP), 0, "ERDP");
    for port in 1..=2 {
        assert_eq!(rig.dword(portsc(port)), PORTSC_INITIAL, "PORTSC{port}");
    }
}

#[test]
fn hcrst_from_a_running_controller_resets_registers_slots_and_ports() {
    let mut rig = running_rig();
    rig.set_dword(regs::DNCTRL, 0x55);
    rig.set_dword(regs::IMOD, 0x1234);
    rig.enable_slot();
    rig.enable_slot();
    rig.set_dword(portsc(1), PORTSC_PR);
    rig.advance_ns(XHCI_USB_PORT_RESET_NS);
    rig.set_dword(USBCMD, HCRST);
    assert_registers_reset(&mut rig);
    assert!(!rig.xhci.sample_pci_interrupts());
    // The slots are Disabled again: the next Enable Slot starts at slot 1.
    rig.start();
    assert_eq!(rig.enable_slot().slot_id, 1);
}

#[test]
fn hcrst_from_the_power_on_state_changes_nothing() {
    let mut rig = rig();
    rig.set_dword(USBCMD, HCRST);
    assert_registers_reset(&mut rig);
}

#[test]
fn hcrst_does_not_touch_guest_memory_or_pci_configuration() {
    let mut rig = running_rig();
    rig.mem_write(DCBAA + 8, &0x1234_5678_9ABC_DEF0u64.to_le_bytes());
    let erst = rig.mem_read(ERST, 16);
    let command = rig.cfg_read(0x04, AccessWidth::Word);
    let bar = rig.cfg_read(0x10, AccessWidth::Dword);
    rig.set_dword(USBCMD, HCRST);
    assert_eq!(
        rig.mem_read(DCBAA + 8, 8),
        0x1234_5678_9ABC_DEF0u64.to_le_bytes()
    );
    assert_eq!(rig.mem_read(ERST, 16), erst);
    assert_eq!(
        rig.cfg_read(0x04, AccessWidth::Word) & 0x406,
        command & 0x406
    );
    assert_eq!(rig.cfg_read(0x10, AccessWidth::Dword), bar);
    assert_eq!(rig.dword(0x00), 0x0100_0020, "BAR0 still decodes");
}

#[test]
fn hcrst_clears_host_system_error() {
    let mut rig = rig();
    rig.program_event_ring_at(ERST, EVENT_RING, 4);
    rig.program_command_ring();
    rig.set_dword(USBCMD, RS);
    rig.service();
    assert_eq!(rig.usbsts() & HSE, HSE);
    rig.set_dword(USBCMD, HCRST);
    assert_registers_reset(&mut rig);
}

#[test]
fn hcrst_during_a_port_reset_cancels_the_reset() {
    let mut rig = running_rig();
    rig.set_dword(portsc(1), PORTSC_PR);
    assert_eq!(rig.dword(portsc(1)) & PORTSC_PR, PORTSC_PR);
    rig.set_dword(USBCMD, HCRST);
    assert_eq!(rig.dword(portsc(1)), PORTSC_INITIAL);
    rig.advance_ns(XHCI_USB_PORT_RESET_NS * 2);
    assert_eq!(
        rig.dword(portsc(1)),
        PORTSC_INITIAL,
        "the cancelled reset never completes"
    );
    assert_eq!(rig.usbsts() & PCD, 0);
}

#[test]
fn hcrst_returns_a_reset_port_to_connected_and_not_reset() {
    let mut rig = running_rig();
    rig.set_dword(portsc(2), PORTSC_PR);
    rig.advance_ns(XHCI_USB_PORT_RESET_NS);
    assert_eq!(rig.dword(portsc(2)), PORTSC_AFTER_RESET & !PORTSC_CSC);
    rig.set_dword(USBCMD, HCRST);
    assert_eq!(rig.dword(portsc(2)), PORTSC_INITIAL);
}

#[test]
fn hcrst_with_a_full_event_ring_starts_over() {
    let mut rig = running_rig();
    for _ in 0..16 {
        rig.push_command(no_op());
    }
    rig.ring_command_doorbell();
    rig.service();
    let events = rig.peek_events();
    assert_eq!(events.last().unwrap().kind, TRB_TYPE_HOST_CONTROLLER_EVENT);
    rig.set_dword(USBCMD, HCRST);
    assert_registers_reset(&mut rig);
    rig.start();
    assert_eq!(rig.command(no_op()).code, CC_SUCCESS);
}

#[test]
fn hcrst_drops_a_pending_command_doorbell() {
    let mut rig = rig();
    rig.begin();
    rig.cfg_write(0x04, AccessWidth::Word, COMMAND_MSE);
    rig.push_command(no_op());
    rig.ring_command_doorbell();
    rig.set_dword(USBCMD, HCRST);
    rig.cfg_write(0x04, AccessWidth::Word, COMMAND_MSE | COMMAND_BME);
    assert!(!rig.xhci.service_pending());
    assert_eq!(rig.service(), XhciServiceReport::default());
}

#[test]
fn after_stop_and_hcrst_the_model_does_not_touch_guest_ram() {
    let mut rig = running_rig();
    rig.command(no_op());
    // The firmware handoff of the contract (section 12.3): stop, reset, mask.
    rig.set_dword(USBCMD, 0);
    assert_eq!(rig.usbsts() & HCH, HCH);
    rig.set_dword(USBCMD, HCRST);
    rig.set_dword(USBCMD, 0);
    let accesses = rig.xhci.ram_accesses();
    rig.set_dword(doorbell(0), 0);
    rig.set_dword(portsc(1), PORTSC_PR);
    rig.advance_ns(XHCI_USB_PORT_RESET_NS);
    assert_eq!(rig.service(), XhciServiceReport::default());
    assert_eq!(rig.xhci.ram_accesses(), accesses);
    assert!(!rig.xhci.sample_pci_interrupts());
}

// ---------------------------------------------------------------- ports

#[test]
fn ports_start_attached_enabled_off_and_in_polling() {
    let mut rig = rig();
    assert_eq!(rig.dword(portsc(1)), PORTSC_INITIAL);
    assert_eq!(rig.dword(portsc(2)), PORTSC_INITIAL);
    for port in 1..=2 {
        for register in [4, 8, 12] {
            assert_eq!(
                rig.dword(portsc(port) + register),
                0,
                "PORTPMSC/PORTLI/PORTHLPMC"
            );
        }
    }
    assert_eq!(rig.dword(portsc(2) + 0x10), 0, "no third port");
    assert_eq!(
        rig.usbsts() & PCD,
        0,
        "the connect after HCRST is not a change transition"
    );
}

#[test]
fn portsc_fixed_fields_follow_the_contract() {
    let mut rig = rig();
    let value = rig.dword(portsc(1));
    assert_eq!(value & 1, 1, "CCS");
    assert_eq!((value >> 5) & 0xF, 7, "PLS = Polling");
    assert_eq!((value >> 9) & 1, 1, "PP");
    assert_eq!((value >> 10) & 0xF, 1, "Port Speed = Full-speed");
    assert_eq!((value >> 14) & 3, 0, "PIC");
}

#[test]
fn port_reset_lasts_exactly_50_ms_of_virtual_time() {
    let mut rig = rig();
    rig.set_dword(portsc(1), PORTSC_PR);
    let during = rig.dword(portsc(1));
    assert_eq!(during & PORTSC_PR, PORTSC_PR);
    assert_eq!(during & PORTSC_PED, 0);
    assert_eq!((during >> 5) & 0xF, 7);
    rig.advance_ns(XHCI_USB_PORT_RESET_NS - 1);
    assert_eq!(rig.dword(portsc(1)) & PORTSC_PR, PORTSC_PR, "1 ns early");
    assert_eq!(rig.usbsts() & PCD, 0);
    rig.advance_ns(1);
    assert_eq!(rig.dword(portsc(1)), PORTSC_AFTER_RESET);
    assert_eq!(rig.usbsts() & PCD, PCD);
    assert_eq!(
        rig.dword(portsc(2)),
        PORTSC_INITIAL,
        "the other port is untouched"
    );
}

#[test]
fn writing_pr_during_a_reset_does_not_restart_it() {
    let mut rig = rig();
    rig.set_dword(portsc(1), PORTSC_PR);
    rig.advance_ns(30_000_000);
    rig.set_dword(portsc(1), PORTSC_PR);
    rig.advance_ns(20_000_000);
    assert_eq!(rig.dword(portsc(1)), PORTSC_AFTER_RESET);
}

#[test]
fn port_reset_can_be_configured_to_take_no_time() {
    let mut rig = rig_with(XhciConfig { port_reset_ns: 0 });
    rig.cfg_write(0x04, AccessWidth::Word, COMMAND_MSE | COMMAND_BME);
    rig.set_dword(portsc(1), PORTSC_PR);
    assert_eq!(rig.dword(portsc(1)), PORTSC_AFTER_RESET);
    assert_eq!(rig.usbsts() & PCD, PCD);
}

#[test]
fn port_reset_does_not_complete_without_virtual_time() {
    let mut rig = rig();
    rig.set_dword(portsc(1), PORTSC_PR);
    for _ in 0..10 {
        assert_eq!(
            rig.dword(portsc(1)) & PORTSC_PR,
            PORTSC_PR,
            "no sleeping, no host clock"
        );
    }
}

#[test]
fn writing_ped_disables_an_enabled_port_without_pec() {
    let mut rig = rig();
    rig.set_dword(portsc(1), PORTSC_PR);
    rig.advance_ns(XHCI_USB_PORT_RESET_NS);
    rig.set_dword(portsc(1), PORTSC_PRC | PORTSC_CSC);
    rig.set_dword(portsc(1), PORTSC_PED);
    let value = rig.dword(portsc(1));
    assert_eq!(value & PORTSC_PED, 0);
    assert_eq!((value >> 5) & 0xF, 7, "PLS = Polling again");
    assert_eq!(
        value & (1 << 18),
        0,
        "PEC is not set by disabling a USB2 port"
    );
    assert_eq!(value, PORTSC_INITIAL & !PORTSC_CSC);
    // A disabled port needs a new reset.
    rig.set_dword(portsc(1), PORTSC_PR);
    rig.advance_ns(XHCI_USB_PORT_RESET_NS);
    assert_eq!(rig.dword(portsc(1)) & PORTSC_PED, PORTSC_PED);
}

#[test]
fn writing_ped_to_a_port_that_is_not_enabled_does_nothing() {
    let mut rig = rig();
    rig.set_dword(portsc(1), PORTSC_PED);
    assert_eq!(rig.dword(portsc(1)), PORTSC_INITIAL);
}

#[test]
fn change_bits_are_write_one_to_clear() {
    let mut rig = rig();
    rig.set_dword(portsc(1), PORTSC_PR);
    rig.advance_ns(XHCI_USB_PORT_RESET_NS);
    assert_eq!(
        rig.dword(portsc(1)) & (PORTSC_CSC | PORTSC_PRC),
        PORTSC_CSC | PORTSC_PRC
    );
    rig.set_dword(portsc(1), 0);
    assert_eq!(
        rig.dword(portsc(1)),
        PORTSC_AFTER_RESET,
        "writing 0 clears nothing"
    );
    rig.set_dword(portsc(1), PORTSC_CSC);
    assert_eq!(rig.dword(portsc(1)), PORTSC_AFTER_RESET & !PORTSC_CSC);
    rig.set_dword(portsc(1), PORTSC_PRC);
    assert_eq!(
        rig.dword(portsc(1)),
        PORTSC_AFTER_RESET & !PORTSC_CSC & !PORTSC_PRC
    );
}

#[test]
fn other_change_bits_never_set_and_ignore_writes() {
    let mut rig = rig();
    rig.set_dword(
        portsc(1),
        (1 << 18) | (1 << 19) | (1 << 20) | (1 << 22) | (1 << 23),
    );
    assert_eq!(rig.dword(portsc(1)), PORTSC_INITIAL);
}

#[test]
fn read_only_portsc_fields_ignore_writes() {
    let mut rig = rig();
    // CCS, OCA, PP, Port Speed, PIC, CAS, WCE/WDE/WOE, DR, WPR.
    let ro = 1 | (1 << 3) | (1 << 9) | (0xF << 10) | (3 << 14) | (1 << 24) | (7 << 25) | (3 << 30);
    rig.set_dword(portsc(1), ro);
    assert_eq!(rig.dword(portsc(1)), PORTSC_INITIAL);
}

#[test]
fn link_state_write_is_ignored_for_u2_u3_and_resume() {
    let mut rig = rig();
    rig.set_dword(portsc(1), PORTSC_PR);
    rig.advance_ns(XHCI_USB_PORT_RESET_NS);
    rig.set_dword(portsc(1), PORTSC_PRC | PORTSC_CSC);
    for pls in [0u32, 2, 3, 15] {
        rig.set_dword(portsc(1), (1 << 16) | (pls << 5));
        let value = rig.dword(portsc(1));
        assert_eq!((value >> 5) & 0xF, 0, "PLS = U0 after writing {pls}");
        assert_eq!(value & PORTSC_PED, PORTSC_PED);
    }
}

#[test]
fn port_status_change_events_follow_run_stop_and_the_event_ring() {
    let mut rig = rig();
    rig.program_event_ring();
    // Stopped: nothing is published.
    assert_eq!(rig.service(), XhciServiceReport::default());
    assert_eq!(rig.peek_events().len(), 0);
    rig.set_dword(USBCMD, RS);
    let report = rig.service();
    assert_eq!(report.events, 2);
    let events = rig.peek_events();
    assert_eq!(events.len(), 2);
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.kind, TRB_TYPE_PORT_STATUS_CHANGE_EVENT);
        assert_eq!(event.port_id(), index as u32 + 1);
        assert_eq!(event.code, CC_SUCCESS);
        assert!(event.cycle);
    }
    assert_eq!(rig.iman() & IP, IP);
    assert_eq!(rig.usbsts() & EINT, EINT);
    assert_eq!(rig.dword(ERDP) & EHB, EHB);
}

#[test]
fn port_status_change_event_waits_for_a_configured_event_ring() {
    let mut rig = rig();
    rig.set_dword(USBCMD, RS);
    assert_eq!(rig.service(), XhciServiceReport::default(), "no ERST yet");
    rig.program_event_ring_at(ERST, EVENT_RING, EVENT_TRBS);
    rig.set_dword(USBCMD, 0);
    rig.program_event_ring_at(ERST, EVENT_RING, EVENT_TRBS);
    rig.set_dword(USBCMD, RS);
    assert_eq!(
        rig.service().events,
        2,
        "published at the first opportunity"
    );
}

#[test]
fn port_status_change_event_is_published_once_until_all_change_bits_are_cleared() {
    let mut rig = running_rig();
    rig.set_dword(portsc(1), PORTSC_PR);
    rig.advance_ns(XHCI_USB_PORT_RESET_NS);
    assert_eq!(rig.usbsts() & PCD, PCD);
    rig.service();
    let events = rig.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].port_id(), 1);
    // A second change while the first is unacknowledged makes no new event.
    rig.set_dword(portsc(1), PORTSC_PED);
    rig.set_dword(portsc(1), PORTSC_PR);
    rig.advance_ns(XHCI_USB_PORT_RESET_NS);
    rig.service();
    assert_eq!(rig.events().len(), 0);
    // Clearing only one of the bits (PRC set, CSC clear) keeps the port "reported".
    rig.set_dword(portsc(1), PORTSC_CSC);
    rig.service();
    assert_eq!(rig.events().len(), 0);
    rig.set_dword(portsc(1), PORTSC_PRC);
    // All change bits are clear: the next change is reported again.
    rig.set_dword(portsc(1), PORTSC_PED);
    rig.set_dword(portsc(1), PORTSC_PR);
    rig.advance_ns(XHCI_USB_PORT_RESET_NS);
    rig.service();
    let events = rig.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].port_id(), 1);
}

#[test]
fn port_status_change_event_is_per_port() {
    let mut rig = running_rig();
    rig.set_dword(portsc(2), PORTSC_PR);
    rig.advance_ns(XHCI_USB_PORT_RESET_NS);
    rig.service();
    let events = rig.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].port_id(), 2);
}

#[test]
fn port_status_change_event_waits_for_room_in_a_full_event_ring() {
    let mut rig = running_rig();
    for _ in 0..16 {
        rig.push_command(no_op());
    }
    rig.ring_command_doorbell();
    rig.service();
    assert_eq!(
        rig.peek_events().last().unwrap().kind,
        TRB_TYPE_HOST_CONTROLLER_EVENT
    );
    rig.set_dword(portsc(1), PORTSC_PR);
    rig.advance_ns(XHCI_USB_PORT_RESET_NS);
    assert_eq!(
        rig.service().events,
        0,
        "the ring is full: port reporting is suspended"
    );
    rig.acknowledge_events();
    // The blocked 16th command runs too.
    assert_eq!(rig.service().events, 2);
    let events = rig.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, TRB_TYPE_PORT_STATUS_CHANGE_EVENT);
    assert_eq!(events[0].port_id(), 1);
    assert_eq!(events[1].kind, TRB_TYPE_COMMAND_COMPLETION_EVENT);
}

// ---------------------------------------------------------------- Enable Slot, Disable Slot, No Op

#[test]
fn enable_slot_allocates_the_smallest_free_slot_ids() {
    let mut rig = running_rig();
    for expected in 1..=3 {
        let event = rig.enable_slot();
        assert_eq!(event.code, CC_SUCCESS);
        assert_eq!(event.slot_id, expected);
        assert!(event.cycle);
    }
}

#[test]
fn enable_slot_completion_event_layout_follows_the_contract() {
    let mut rig = running_rig();
    let address = rig.push_command(enable_slot());
    rig.ring_command_doorbell();
    rig.service();
    let raw = rig.read_trb(EVENT_RING + 2 * 16);
    assert_eq!(
        raw[0], address as u32,
        "DW0:DW1 = address of the command TRB"
    );
    assert_eq!(raw[1], (address >> 32) as u32);
    assert_eq!(raw[2], 1 << 24, "code Success, parameter 0");
    assert_eq!(
        raw[3],
        (1 << 24) | (33 << 10) | 1,
        "slot 1, type 33, cycle 1"
    );
}

#[test]
fn enable_slot_is_no_slots_available_when_max_slots_en_is_zero() {
    let mut rig = rig();
    rig.start_with(0);
    let event = rig.enable_slot();
    assert_eq!(event.code, CC_NO_SLOTS_AVAILABLE);
    assert_eq!(event.slot_id, 0);
}

#[test]
fn enable_slot_honours_max_slots_en_below_the_controller_limit() {
    let mut rig = rig();
    rig.start_with(2);
    assert_eq!(rig.enable_slot().slot_id, 1);
    assert_eq!(rig.enable_slot().slot_id, 2);
    let event = rig.enable_slot();
    assert_eq!(event.code, CC_NO_SLOTS_AVAILABLE);
    assert_eq!(event.slot_id, 0);
}

#[test]
fn enable_slot_is_limited_to_eight_slots_even_when_max_slots_en_is_larger() {
    let mut rig = rig();
    rig.start_with(255);
    for expected in 1..=8 {
        assert_eq!(rig.enable_slot().slot_id, expected);
    }
    assert_eq!(rig.enable_slot().code, CC_NO_SLOTS_AVAILABLE);
}

#[test]
fn enable_slot_with_a_nonzero_slot_type_is_a_trb_error() {
    let mut rig = running_rig();
    let mut trb = enable_slot();
    trb[3] |= 1 << 16;
    let event = rig.command(trb);
    assert_eq!(event.code, CC_TRB_ERROR);
    assert_eq!(event.slot_id, 0);
    assert_eq!(rig.enable_slot().slot_id, 1, "no slot was consumed");
}

#[test]
fn disable_slot_frees_the_slot_for_the_next_enable() {
    let mut rig = running_rig();
    rig.enable_slot();
    rig.enable_slot();
    let event = rig.command(disable_slot(1));
    assert_eq!(event.code, CC_SUCCESS);
    assert_eq!(event.slot_id, 1);
    assert_eq!(
        rig.enable_slot().slot_id,
        1,
        "the smallest free id is reused"
    );
    assert_eq!(rig.enable_slot().slot_id, 3);
}

#[test]
fn disable_slot_of_a_slot_that_was_never_enabled_is_slot_not_enabled() {
    let mut rig = running_rig();
    rig.enable_slot();
    let event = rig.command(disable_slot(3));
    assert_eq!(event.code, CC_SLOT_NOT_ENABLED);
    assert_eq!(event.slot_id, 3, "the slot id of the TRB");
}

#[test]
fn disable_slot_twice_is_slot_not_enabled() {
    let mut rig = running_rig();
    rig.enable_slot();
    assert_eq!(rig.command(disable_slot(1)).code, CC_SUCCESS);
    assert_eq!(rig.command(disable_slot(1)).code, CC_SLOT_NOT_ENABLED);
}

#[test]
fn disable_slot_with_slot_id_zero_or_above_eight_is_slot_not_enabled() {
    let mut rig = running_rig();
    for slot_id in [0, 9, 255] {
        let event = rig.command(disable_slot(slot_id));
        assert_eq!(event.code, CC_SLOT_NOT_ENABLED, "slot {slot_id}");
        assert_eq!(event.slot_id, slot_id);
    }
}

#[test]
fn disable_slot_leaves_the_dcbaa_entry_to_the_driver() {
    let mut rig = running_rig();
    let marker = 0x0000_0000_0007_0040u64;
    rig.enable_slot();
    rig.mem_write(DCBAA + 8, &marker.to_le_bytes());
    rig.command(disable_slot(1));
    assert_eq!(rig.mem_read(DCBAA + 8, 8), marker.to_le_bytes());
}

#[test]
fn no_op_succeeds_with_slot_id_zero() {
    let mut rig = running_rig();
    let mut trb = no_op();
    trb[3] |= 5 << 24;
    let event = rig.command(trb);
    assert_eq!(event.code, CC_SUCCESS);
    assert_eq!(event.slot_id, 0);
}

#[test]
fn unsupported_commands_the_ring_allows_are_a_trb_error_with_the_slot_id_of_the_trb() {
    // Force Event, Negotiate Bandwidth, Set Latency Tolerance Value, Get Port Bandwidth,
    // Force Header, Get and Set Extended Property.
    let mut rig = running_rig();
    for kind in [18, 19, 20, 21, 22, 24, 25] {
        let event = rig.command(command_trb(kind, 2));
        assert_eq!(event.code, CC_TRB_ERROR, "command type {kind}");
        assert_eq!(event.slot_id, 2, "command type {kind}");
    }
}

#[test]
fn types_that_are_not_command_trbs_are_a_trb_error_with_slot_id_zero() {
    // Normal, Setup Stage, Status Stage, Transfer No-Op, reserved, events, vendor.
    let mut rig = running_rig();
    for kind in [1, 2, 4, 8, 26, 31, 32, 33, 34, 37, 48, 63] {
        let event = rig.command(command_trb(kind, 4));
        assert_eq!(event.code, CC_TRB_ERROR, "TRB type {kind}");
        assert_eq!(event.slot_id, 0, "TRB type {kind}");
    }
}

#[test]
fn a_trb_error_does_not_stop_the_command_ring() {
    let mut rig = running_rig();
    rig.push_command(command_trb(18, 0));
    rig.push_command(enable_slot());
    rig.ring_command_doorbell();
    let report = rig.service();
    assert_eq!(report.commands, 2);
    let events = rig.events();
    assert_eq!(events[0].code, CC_TRB_ERROR);
    assert_eq!(events[1].code, CC_SUCCESS);
}

// ---------------------------------------------------------------- command ring and event ring

#[test]
fn command_ring_wraps_through_the_link_trb_and_toggles_the_cycle_bit() {
    let mut rig = running_rig();
    // 70 commands over a segment of 31 command slots and a Link TRB: two Link TRB crossings,
    // and the event ring (16 TRBs) wraps along.
    for index in 0..70u32 {
        let event = rig.command(no_op());
        assert_eq!(event.code, CC_SUCCESS, "command {index}");
    }
    assert!(rig.cmd_cycle, "two crossings leave the producer cycle at 1");
    assert_eq!(rig.cmd_index, 8);
    assert_eq!(rig.dword(CRCR), CRR);
}

#[test]
fn link_trb_produces_no_event_and_is_not_counted_as_a_command() {
    let mut rig = running_rig();
    for _ in 0..30 {
        rig.command(no_op());
    }
    // The 31st command is the last slot before the Link TRB; the next two cross it.
    for _ in 0..3 {
        rig.push_command(no_op());
    }
    rig.ring_command_doorbell();
    let report = rig.service();
    assert_eq!(report.commands, 3, "the Link TRB is not a command");
    assert_eq!(report.events, 3);
    assert_eq!(
        rig.events().len(),
        3,
        "one event per command, none for the Link TRB"
    );
}

#[test]
fn link_trb_without_toggle_cycle_keeps_the_cycle_state() {
    let mut rig = running_rig();
    let target = CMD_RING + 0x100;
    rig.put_trb(CMD_RING, [0, 0, 0, (TRB_TYPE_NO_OP_COMMAND << 10) | 1]);
    rig.put_trb(
        CMD_RING + 16,
        [
            target as u32,
            (target >> 32) as u32,
            0,
            (TRB_TYPE_LINK << 10) | 1,
        ],
    );
    rig.put_trb(target, [0, 0, 0, (TRB_TYPE_NO_OP_COMMAND << 10) | 1]);
    rig.put_trb(target + 16, [0, 0, 0, (TRB_TYPE_ENABLE_SLOT << 10) | 1]);
    rig.ring_command_doorbell();
    assert_eq!(rig.service().commands, 3);
    let events = rig.events();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].parameter, CMD_RING);
    assert_eq!(
        events[1].parameter, target,
        "the ring continues at the Link target"
    );
    assert_eq!(events[2].parameter, target + 16);
    assert_eq!(events[2].slot_id, 1);
}

#[test]
fn link_trb_with_toggle_cycle_changes_which_cycle_bit_counts_as_new() {
    let mut rig = running_rig();
    let target = CMD_RING + 0x100;
    rig.put_trb(
        CMD_RING,
        [
            target as u32,
            (target >> 32) as u32,
            0,
            (TRB_TYPE_LINK << 10) | 2 | 1,
        ],
    );
    // After the toggle the ring expects cycle 0: a TRB with cycle 1 there is stale.
    rig.put_trb(target, [0, 0, 0, (TRB_TYPE_NO_OP_COMMAND << 10) | 1]);
    rig.ring_command_doorbell();
    assert_eq!(rig.service().commands, 0);
    assert!(rig.events().is_empty());
    rig.put_trb(target, [0, 0, 0, TRB_TYPE_NO_OP_COMMAND << 10]);
    // The next TRB is stale (cycle 1 where 0 is expected).
    rig.put_trb(target + 16, [0, 0, 0, (TRB_TYPE_NO_OP_COMMAND << 10) | 1]);
    rig.ring_command_doorbell();
    assert_eq!(rig.service().commands, 1);
}

#[test]
fn link_loop_is_bounded_by_the_command_budget() {
    let mut rig = running_rig();
    rig.put_trb(
        CMD_RING,
        [
            CMD_RING as u32,
            (CMD_RING >> 32) as u32,
            0,
            (TRB_TYPE_LINK << 10) | 1,
        ],
    );
    rig.ring_command_doorbell();
    let report = rig.service();
    assert_eq!(report, XhciServiceReport::default());
    assert!(
        rig.xhci.service_pending(),
        "the rest waits for the next service call"
    );
}

#[test]
fn command_ring_stops_at_the_first_trb_with_another_cycle_bit() {
    let mut rig = running_rig();
    rig.put_trb(CMD_RING, [0, 0, 0, (TRB_TYPE_NO_OP_COMMAND << 10) | 1]);
    rig.put_trb(CMD_RING + 16, [0, 0, 0, (TRB_TYPE_NO_OP_COMMAND << 10) | 1]);
    rig.put_trb(CMD_RING + 32, [0, 0, 0, TRB_TYPE_NO_OP_COMMAND << 10]);
    rig.ring_command_doorbell();
    assert_eq!(rig.service().commands, 2);
    assert_eq!(rig.events().len(), 2);
    assert!(!rig.xhci.service_pending(), "an empty ring is idle");
    assert_eq!(
        rig.dword(CRCR),
        CRR,
        "CRR stays set while the controller runs"
    );
}

#[test]
fn a_command_trb_with_a_stale_cycle_bit_runs_once_its_cycle_bit_is_set() {
    let mut rig = running_rig();
    rig.put_trb(CMD_RING, [0, 0, 0, TRB_TYPE_NO_OP_COMMAND << 10]);
    rig.ring_command_doorbell();
    assert_eq!(rig.service().commands, 0);
    rig.put_trb(CMD_RING, [0, 0, 0, (TRB_TYPE_NO_OP_COMMAND << 10) | 1]);
    rig.ring_command_doorbell();
    assert_eq!(rig.service().commands, 1);
}

#[test]
fn a_doorbell_on_an_empty_command_ring_writes_no_event() {
    let mut rig = running_rig();
    rig.ring_command_doorbell();
    assert_eq!(rig.service(), XhciServiceReport::default());
    assert!(rig.events().is_empty());
    assert_eq!(rig.dword(CRCR), CRR);
}

#[test]
fn an_empty_event_ring_stays_empty_and_quiet_without_work() {
    let mut rig = running_rig();
    let erdp = rig.qword(ERDP);
    for _ in 0..3 {
        assert_eq!(rig.service(), XhciServiceReport::default());
    }
    assert!(rig.peek_events().is_empty());
    assert_eq!(rig.iman(), IE, "IP clear");
    assert_eq!(rig.usbsts() & (EINT | PCD | HSE), 0);
    assert!(!rig.xhci.sample_pci_interrupts());
    assert_eq!(rig.qword(ERDP), erdp);
}

#[test]
fn event_ring_wraps_and_toggles_the_cycle_bit() {
    let mut rig = running_rig();
    // start() consumed two events (slots 0 and 1): 15 more reach slot 16 = slot 0 of lap two.
    for _ in 0..15 {
        rig.command(no_op());
    }
    assert_eq!(
        rig.read_trb(EVENT_RING)[3] & 1,
        0,
        "lap two writes cycle 0 at slot 0"
    );
    assert_eq!(
        rig.read_trb(EVENT_RING + 16)[3] & 1,
        1,
        "slot 1 still has lap one's cycle 1"
    );
    assert_eq!(rig.read_trb(EVENT_RING + 15 * 16)[3] & 1, 1);
    rig.command(no_op());
    assert_eq!(rig.read_trb(EVENT_RING + 16)[3] & 1, 0);
}

#[test]
fn event_ring_full_error_is_written_once_and_blocks_the_command_ring() {
    let mut rig = running_rig();
    // 16 commands: 15 fit with the last TRB reserved, the 16th finds one free TRB.
    for _ in 0..16 {
        rig.push_command(no_op());
    }
    rig.ring_command_doorbell();
    let report = rig.service();
    assert_eq!(report.commands, 15);
    assert_eq!(
        report.events, 16,
        "15 completions and the Event Ring Full Error"
    );
    let events = rig.peek_events();
    assert_eq!(events.len(), 16);
    assert!(
        events[..15]
            .iter()
            .all(|e| e.kind == TRB_TYPE_COMMAND_COMPLETION_EVENT)
    );
    let full = events[15];
    assert_eq!(full.kind, TRB_TYPE_HOST_CONTROLLER_EVENT);
    assert_eq!(full.code, CC_EVENT_RING_FULL_ERROR);
    assert_eq!((full.slot_id, full.parameter), (0, 0));
    // Nothing more happens, and the Event Ring Full Error is not repeated.
    assert_eq!(rig.service(), XhciServiceReport::default());
    assert_eq!(rig.service(), XhciServiceReport::default());
    assert!(
        !rig.xhci.service_pending(),
        "the 16th command waits for ERDP, not for another service call"
    );
    // Moving ERDP makes room: the command runs.
    rig.acknowledge_events();
    assert!(rig.xhci.service_pending());
    let report = rig.service();
    assert_eq!(report.commands, 1);
    let events = rig.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, TRB_TYPE_COMMAND_COMPLETION_EVENT);
    assert!(!rig.xhci.service_pending());
}

#[test]
fn event_ring_stays_blocked_while_erdp_is_written_without_acknowledging_anything() {
    let mut rig = running_rig();
    for _ in 0..16 {
        rig.push_command(no_op());
    }
    rig.ring_command_doorbell();
    rig.service();
    let pointer = EVENT_RING + 2 * 16;
    // Same pointer, EHB = 0: nothing was consumed.
    rig.set_qword(ERDP, pointer);
    assert_eq!(rig.service(), XhciServiceReport::default());
    // Two events consumed: the ring has room for an event and the reserved TRB.
    rig.set_qword(ERDP, (pointer + 32) | u64::from(EHB));
    assert_eq!(rig.service().commands, 1);
}

#[test]
fn erdp_outside_the_segment_consumes_nothing() {
    let mut rig = running_rig();
    for _ in 0..16 {
        rig.push_command(no_op());
    }
    rig.ring_command_doorbell();
    rig.service();
    rig.set_qword(ERDP, (EVENT_RING + 0x1000) | u64::from(EHB));
    assert_eq!(rig.service(), XhciServiceReport::default());
}

#[test]
fn ip_eint_and_ehb_are_set_by_the_first_event_and_not_again_until_ehb_is_cleared() {
    let mut rig = running_rig();
    rig.command(no_op());
    // events() cleared EHB: the next event sets IP again.
    assert_eq!(rig.iman() & IP, IP);
    rig.set_dword(IMAN, IP | IE);
    rig.set_dword(USBSTS, EINT);
    let address = rig.push_command(no_op());
    rig.ring_command_doorbell();
    rig.service();
    assert_eq!(
        rig.iman() & IP,
        IP,
        "EHB = 0 after the acknowledgement: IP is set"
    );
    assert_eq!(rig.dword(ERDP) & EHB, EHB);
    assert_eq!(rig.usbsts() & EINT, EINT);
    // While EHB = 1, a new event does not set IP again.
    rig.set_dword(IMAN, IP | IE);
    rig.set_dword(USBSTS, EINT);
    rig.push_command(no_op());
    rig.ring_command_doorbell();
    rig.service();
    assert_eq!(rig.iman() & IP, 0, "EHB = 1: no new interrupt");
    assert_eq!(rig.usbsts() & EINT, 0);
    assert!(!rig.xhci.sample_pci_interrupts());
    let events = rig.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].parameter, address);
}

#[test]
fn erdp_reads_back_the_pointer_and_ehb() {
    let mut rig = running_rig();
    assert_eq!(
        rig.qword(ERDP),
        EVENT_RING + 2 * 16,
        "pointer after the two port events"
    );
    rig.command(no_op());
    rig.set_qword(ERDP, (EVENT_RING + 3 * 16) | 0x5);
    assert_eq!(rig.qword(ERDP) & 0x7, 0x5, "DESI is stored");
    assert_eq!(rig.qword(ERDP) & !0xF, EVENT_RING + 3 * 16);
}

#[test]
fn usbsts_pcd_is_write_one_to_clear() {
    let mut rig = running_rig();
    rig.set_dword(portsc(1), PORTSC_PR);
    rig.advance_ns(XHCI_USB_PORT_RESET_NS);
    assert_eq!(rig.usbsts() & PCD, PCD);
    rig.set_dword(USBSTS, PCD);
    assert_eq!(rig.usbsts() & PCD, 0);
}

// ---------------------------------------------------------------- Host System Error

/// A stopped controller with a command ring and an ERST entry that describes the given segment,
/// started and serviced once.
fn rig_with_event_segment(erst: u64, base: u64, size: u32) -> Rig {
    let mut rig = rig();
    rig.program_event_ring_at(erst, base, size);
    rig.program_command_ring();
    rig.set_dword(USBCMD, RS);
    rig.service();
    rig
}

fn assert_host_system_error(rig: &mut Rig, cause: HseCause) {
    assert_eq!(rig.usbsts() & (HSE | HCH), HSE | HCH);
    assert_eq!(rig.dword(USBCMD) & RS, 0, "R/S is forced to 0");
    assert_eq!(rig.dword(CRCR) & CRR, 0);
    assert_eq!(rig.xhci.hse_cause(), Some(cause));
}

#[test]
fn erst_segment_of_15_trbs_is_a_host_system_error() {
    let mut rig = rig_with_event_segment(ERST, EVENT_RING, 15);
    assert_host_system_error(&mut rig, HseCause::EventRingSegment);
}

#[test]
fn erst_segment_of_4097_trbs_is_a_host_system_error() {
    let mut rig = rig_with_event_segment(ERST, EVENT_RING, 4097);
    assert_host_system_error(&mut rig, HseCause::EventRingSegment);
}

#[test]
fn erst_segment_of_zero_trbs_is_a_host_system_error() {
    let mut rig = rig_with_event_segment(ERST, EVENT_RING, 0);
    assert_host_system_error(&mut rig, HseCause::EventRingSegment);
}

#[test]
fn erst_segments_of_16_and_4096_trbs_are_valid() {
    for size in [16, 4096] {
        let mut rig = rig_with_event_segment(ERST, EVENT_RING, size);
        assert_eq!(rig.usbsts() & HSE, 0, "size {size}");
        assert_eq!(rig.xhci.hse_cause(), None);
        assert_eq!(
            rig.peek_events().len(),
            2,
            "size {size}: the port events are written"
        );
    }
}

#[test]
fn erst_segment_base_that_is_not_a_multiple_of_64_is_a_host_system_error() {
    let mut rig = rig_with_event_segment(ERST, EVENT_RING + 16, 16);
    assert_host_system_error(&mut rig, HseCause::EventRingSegment);
}

#[test]
fn erst_segment_outside_ram_is_a_host_system_error() {
    for base in [UNMAPPED, XHCI_PCI_BAR_BASE] {
        let mut rig = rig_with_event_segment(ERST, base, 16);
        assert_host_system_error(&mut rig, HseCause::EventRingSegment);
    }
}

#[test]
fn erst_segment_that_runs_past_the_end_of_ram_is_a_host_system_error() {
    let mut rig = rig_with_event_segment(ERST, RAM_BYTES - 64, 16);
    assert_host_system_error(&mut rig, HseCause::EventRingSegment);
}

#[test]
fn erst_entry_outside_ram_is_a_host_system_error() {
    for erst in [UNMAPPED, XHCI_PCI_BAR_BASE] {
        let mut rig = rig();
        rig.program_command_ring();
        rig.set_dword(ERSTSZ, 1);
        rig.set_qword(ERSTBA, erst);
        rig.set_dword(USBCMD, RS);
        rig.service();
        assert_host_system_error(&mut rig, HseCause::EventRingSegment);
    }
}

#[test]
fn erst_size_register_zero_leaves_the_event_ring_unconfigured_without_error() {
    let mut rig = rig();
    rig.program_event_ring();
    rig.set_dword(ERSTSZ, 0);
    rig.set_dword(USBCMD, RS);
    assert_eq!(rig.service(), XhciServiceReport::default());
    assert_eq!(rig.usbsts() & HSE, 0);
    // The ERST is loaded once ERSTSZ says one segment.
    rig.set_dword(USBCMD, 0);
    rig.set_dword(ERSTSZ, 1);
    rig.set_dword(USBCMD, RS);
    assert_eq!(rig.service().events, 2);
}

#[test]
fn command_ring_outside_ram_is_a_host_system_error() {
    for pointer in [UNMAPPED, XHCI_PCI_BAR_BASE] {
        let mut rig = rig();
        rig.begin();
        rig.set_dword(USBCMD, 0);
        rig.set_qword(CRCR, pointer | 1);
        rig.set_dword(USBCMD, RS);
        rig.ring_command_doorbell();
        rig.service();
        assert_host_system_error(&mut rig, HseCause::CommandRingDma);
    }
}

#[test]
fn link_trb_target_outside_ram_is_a_host_system_error() {
    let mut rig = running_rig();
    rig.put_trb(
        CMD_RING,
        [
            UNMAPPED as u32,
            (UNMAPPED >> 32) as u32,
            0,
            (TRB_TYPE_LINK << 10) | 1,
        ],
    );
    rig.ring_command_doorbell();
    rig.service();
    assert_host_system_error(&mut rig, HseCause::CommandRingDma);
}

#[test]
fn a_host_system_error_stops_all_processing_and_ignores_doorbells() {
    let mut rig = running_rig();
    rig.put_trb(
        CMD_RING,
        [
            UNMAPPED as u32,
            (UNMAPPED >> 32) as u32,
            0,
            (TRB_TYPE_LINK << 10) | 1,
        ],
    );
    rig.ring_command_doorbell();
    rig.service();
    rig.set_dword(doorbell(0), 0);
    assert_eq!(rig.xhci.ignored_doorbells(), 1);
    rig.set_dword(portsc(1), PORTSC_PR);
    rig.advance_ns(XHCI_USB_PORT_RESET_NS);
    assert_eq!(rig.service(), XhciServiceReport::default());
    assert!(rig.peek_events().is_empty());
}

#[test]
fn run_stop_write_is_ignored_after_a_host_system_error_until_hcrst() {
    let mut rig = rig_with_event_segment(ERST, EVENT_RING, 15);
    rig.set_dword(USBCMD, RS);
    assert_eq!(rig.dword(USBCMD) & RS, 0);
    assert_eq!(rig.usbsts() & HCH, HCH);
    rig.set_dword(USBSTS, HSE);
    assert_eq!(rig.usbsts() & HSE, 0, "HSE is write-one-to-clear");
    rig.set_dword(USBCMD, RS);
    assert_eq!(
        rig.usbsts() & HCH,
        HCH,
        "clearing HSE does not allow R/S = 1: only HCRST does"
    );
    rig.set_dword(USBCMD, HCRST);
    rig.set_dword(USBCMD, RS);
    assert_eq!(rig.usbsts() & HCH, 0);
}

#[test]
fn host_system_error_raises_intx_only_with_hsee_ie_and_intx_enabled() {
    let mut rig = rig_with_event_segment(ERST, EVENT_RING, 15);
    assert!(!rig.xhci.sample_pci_interrupts(), "HSEE = 0");
    rig.set_dword(USBCMD, HSEE);
    assert!(!rig.xhci.sample_pci_interrupts(), "IMAN.IE = 0");
    rig.set_dword(IMAN, IE);
    assert!(rig.xhci.sample_pci_interrupts());
    rig.cfg_write(
        0x04,
        AccessWidth::Word,
        COMMAND_MSE | COMMAND_BME | COMMAND_INTX_DISABLE,
    );
    assert!(!rig.xhci.sample_pci_interrupts(), "Command.INTx Disable");
    rig.cfg_write(0x04, AccessWidth::Word, COMMAND_MSE | COMMAND_BME);
    rig.set_dword(USBSTS, HSE);
    assert!(!rig.xhci.sample_pci_interrupts(), "HSE cleared");
}

#[test]
fn hse_cause_is_not_set_without_an_error() {
    let rig = running_rig();
    assert_eq!(rig.xhci.hse_cause(), None);
}

#[test]
fn erstsz_ignores_reserved_bits_above_15() {
    let mut rig = rig();
    rig.set_dword(ERSTSZ, 0x1_0001);
    assert_eq!(rig.dword(ERSTSZ), 1);
}

#[test]
fn link_trb_target_that_is_not_64_byte_aligned_is_a_host_system_error() {
    let mut rig = running_rig();
    let target = CMD_RING + 0x110;
    rig.put_trb(
        CMD_RING,
        [
            target as u32,
            (target >> 32) as u32,
            0,
            (TRB_TYPE_LINK << 10) | 1,
        ],
    );
    rig.ring_command_doorbell();
    rig.service();
    assert_host_system_error(&mut rig, HseCause::CommandRingDma);
}

#[test]
fn erdp_past_the_enqueue_pointer_is_ignored() {
    let mut rig = running_rig();
    for _ in 0..3 {
        rig.command(no_op());
    }
    // Three events written; the driver claims it consumed more than that.
    let before = rig.qword(ERDP);
    rig.set_qword(ERDP, (before & !0xF) + 10 * 16);
    for _ in 0..12 {
        rig.push_command(no_op());
    }
    rig.ring_command_doorbell();
    rig.service();
    // The ring still knows nothing was consumed beyond the real events: 12 more fit, the
    // thirteenth would not, and no event was overwritten.
    let events = rig.peek_events();
    assert!(
        events
            .iter()
            .all(|e| e.kind == TRB_TYPE_COMMAND_COMPLETION_EVENT)
    );
}

#[test]
fn clearing_ehb_with_events_left_in_the_ring_raises_the_interrupt_again() {
    let mut rig = running_rig();
    // The driver has read the ring; before it writes ERDP the controller adds one more event.
    rig.push_command(no_op());
    rig.ring_command_doorbell();
    rig.service();
    assert_eq!(rig.iman() & IP, IP);
    let first = rig.peek_events();
    assert_eq!(first.len(), 1);
    rig.set_dword(IMAN, IP | IE);
    rig.push_command(no_op());
    rig.ring_command_doorbell();
    rig.service();
    assert_eq!(
        rig.iman() & IP,
        0,
        "EHB = 1: the second event raised no interrupt"
    );
    // ERDP acknowledges the first event only.
    rig.acknowledge_events();
    assert_eq!(
        rig.iman() & IP,
        IP,
        "an unacknowledged event is still in the ring"
    );
    assert_eq!(rig.dword(ERDP) & EHB, EHB);
    assert!(rig.xhci.sample_pci_interrupts());
}

#[test]
fn service_pending_covers_the_erst_load_and_owed_port_events() {
    let mut rig = rig();
    assert!(!rig.xhci.service_pending());
    rig.program_event_ring();
    assert!(rig.xhci.service_pending(), "ERST to load");
    rig.service();
    assert!(!rig.xhci.service_pending(), "stopped: no port events yet");
    rig.set_dword(USBCMD, RS);
    assert!(rig.xhci.service_pending(), "two connect events are owed");
    rig.service();
    assert!(!rig.xhci.service_pending());
    rig.events();
    rig.set_dword(portsc(1), PORTSC_CSC);
    rig.set_dword(portsc(2), PORTSC_CSC);
    rig.set_dword(portsc(1), PORTSC_PR);
    assert!(!rig.xhci.service_pending(), "reset not finished");
    rig.advance_ns(XHCI_USB_PORT_RESET_NS);
    assert!(rig.xhci.service_pending());
}
