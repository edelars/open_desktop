//! Tests of the NIC model (S11-32) through the PCI function and `MachineBus`, the way a driver
//! uses it: registers, descriptor rings in guest RAM, a scripted backend. Contract:
//! `docs/specs/NIC_DEVICE.md`.

use super::regs::*;
use super::*;
use crate::net_backend::{Dropped, NET_MAX_FRAME};
use crate::{
    BusAccessError, GuestPhysicalAddress, MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec,
    MmioBinding, RamLimits,
};
use std::collections::VecDeque;

const RAM_BYTES: u64 = 16 << 20;
const TX_RING: u64 = 0x1_0000;
const RX_RING: u64 = 0x2_0000;
const TX_BUFFERS: u64 = 0x10_0000;
const RX_BUFFERS: u64 = 0x20_0000;
/// Not mapped at all.
const UNMAPPED: u64 = 0x8000_0000;
const GUEST_MAC: [u8; 6] = [0x02, 0x4F, 0x44, 0x00, 0x00, 0x10];
const COMMAND_MSE: u32 = 1 << 1;
const COMMAND_BME: u32 = 1 << 2;

/// What the scripted backend saw and what it will hand to the guest.
#[derive(Default)]
struct Script {
    sent: Vec<Vec<u8>>,
    to_guest: VecDeque<Vec<u8>>,
    refuse: Option<Dropped>,
    polls: u32,
    resets: u32,
}

#[derive(Clone, Default)]
struct ScriptedBackend(Arc<Mutex<Script>>);

impl ScriptedBackend {
    fn script(&self) -> std::sync::MutexGuard<'_, Script> {
        self.0.lock().unwrap()
    }
}

impl NetBackend for ScriptedBackend {
    fn transmit(&mut self, frame: &[u8]) -> Result<(), Dropped> {
        let mut script = self.script();
        if let Some(reason) = script.refuse {
            return Err(reason);
        }
        script.sent.push(frame.to_vec());
        Ok(())
    }

    fn poll_receive(&mut self, buf: &mut [u8]) -> Option<usize> {
        let mut script = self.script();
        script.polls += 1;
        assert!(buf.len() >= NET_MAX_FRAME);
        let frame = script.to_guest.pop_front()?;
        buf[..frame.len()].copy_from_slice(&frame);
        Some(frame.len())
    }

    fn counters(&self) -> NetCounters {
        NetCounters {
            tx_frames: self.script().sent.len() as u64,
            ..NetCounters::default()
        }
    }

    fn reset(&mut self) {
        self.script().resets += 1;
    }
}

struct Rig {
    bus: MachineBus,
    pci: NicPciFunction,
    nic: NicHandle,
    script: ScriptedBackend,
}

fn bare_rig() -> Rig {
    build_rig(true)
}

fn build_rig(attach_backend: bool) -> Rig {
    let (device, nic, pci) = NicDevice::new_pci(NicConfig { mac: GUEST_MAC }).unwrap();
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
    let script = ScriptedBackend::default();
    if attach_backend {
        nic.attach_backend(Box::new(script.clone())).unwrap();
    }
    Rig {
        bus,
        pci,
        nic,
        script,
    }
}

/// Memory decoding and bus mastering enabled.
fn rig() -> Rig {
    let mut rig = bare_rig();
    rig.cfg_write(0x04, AccessWidth::Word, COMMAND_MSE | COMMAND_BME);
    rig
}

impl Rig {
    fn address(offset: u64) -> GuestPhysicalAddress {
        GuestPhysicalAddress::new(NIC_PCI_BAR_BASE + offset)
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

    fn reg(&mut self, offset: u64) -> u32 {
        self.read(offset, AccessWidth::Dword).unwrap() as u32
    }

    fn set_reg(&mut self, offset: u64, value: u32) {
        self.write(offset, AccessWidth::Dword, u64::from(value))
            .unwrap();
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

    fn service(&mut self) -> NicServiceReport {
        self.nic.service(&mut self.bus, |_| {})
    }

    /// Programs the transmit ring of `count` descriptors at [`TX_RING`] and enables it.
    fn init_tx(&mut self, count: u32) {
        self.set_reg(TDBAL, TX_RING as u32);
        self.set_reg(TDBAH, 0);
        self.set_reg(TDLEN, count * 16);
        self.set_reg(TDH, 0);
        self.set_reg(TDT, 0);
        self.set_reg(TCTL, TCTL_EN);
    }

    /// Writes transmit descriptor `index` and its buffer; the buffer address is derived from
    /// the index.
    fn put_tx(&mut self, index: u32, data: &[u8], command: u8) -> u64 {
        let buffer = TX_BUFFERS + u64::from(index) * 0x1000;
        if !data.is_empty() {
            self.mem_write(buffer, data);
        }
        let mut descriptor = [0u8; 16];
        descriptor[0..8].copy_from_slice(&buffer.to_le_bytes());
        descriptor[8..10].copy_from_slice(&(data.len() as u16).to_le_bytes());
        descriptor[11] = command;
        let address = TX_RING + u64::from(index) * 16;
        self.mem_write(address, &descriptor);
        address
    }

    fn tx_status(&mut self, index: u32) -> u8 {
        self.mem_read(TX_RING + u64::from(index) * 16 + 12, 1)[0]
    }

    /// Programs the receive ring of `count` descriptors at [`RX_RING`] with 2 KiB buffers
    /// and enables reception with `rctl`; the tail leaves every descriptor to the hardware.
    fn init_rx(&mut self, count: u32, rctl: u32) {
        for index in 0..count {
            let mut descriptor = [0u8; 16];
            let buffer = RX_BUFFERS + u64::from(index) * 0x4000;
            descriptor[0..8].copy_from_slice(&buffer.to_le_bytes());
            self.mem_write(RX_RING + u64::from(index) * 16, &descriptor);
        }
        self.set_reg(RDBAL, RX_RING as u32);
        self.set_reg(RDBAH, 0);
        self.set_reg(RDLEN, count * 16);
        self.set_reg(RDH, 0);
        self.set_reg(RDT, count - 1);
        self.set_reg(RCTL, rctl);
    }

    fn rx_descriptor(&mut self, index: u32) -> [u8; 16] {
        self.mem_read(RX_RING + u64::from(index) * 16, 16)
            .try_into()
            .unwrap()
    }

    fn rx_buffer(&mut self, index: u32, len: usize) -> Vec<u8> {
        self.mem_read(RX_BUFFERS + u64::from(index) * 0x4000, len)
    }
}

/// A frame of `len` bytes addressed to `destination`.
fn frame_to(destination: [u8; 6], len: usize) -> Vec<u8> {
    let mut frame = vec![0u8; len];
    frame[0..6].copy_from_slice(&destination);
    frame[6..12].copy_from_slice(&NIC_GATEWAY_MAC);
    frame[12] = 0x08;
    for (index, byte) in frame.iter_mut().enumerate().skip(14) {
        *byte = index as u8;
    }
    frame
}

const RCTL_ACCEPT: u32 = RCTL_EN | RCTL_BAM | RCTL_SECRC;

// ---- PCI identity ---------------------------------------------------------------------------

#[test]
fn pci_function_is_an_82540em_ethernet_controller_with_one_memory_bar() {
    let mut rig = bare_rig();
    assert_eq!(rig.cfg_read(0x00, AccessWidth::Dword), 0x100E_8086);
    // Class 02:00:00 (Ethernet), revision 2.
    assert_eq!(rig.cfg_read(0x08, AccessWidth::Dword), 0x0200_0002);
    // BAR0: 32-bit non-prefetchable memory at the board address; BAR1 is not implemented.
    assert_eq!(
        rig.cfg_read(0x10, AccessWidth::Dword),
        NIC_PCI_BAR_BASE as u32
    );
    assert_eq!(rig.cfg_read(0x14, AccessWidth::Dword), 0);
    // INTA.
    assert_eq!(rig.cfg_read(0x3D, AccessWidth::Byte), 1);
    // Sizing: all ones read back the 128 KiB mask.
    rig.cfg_write(0x10, AccessWidth::Dword, 0xFFFF_FFFF);
    assert_eq!(rig.cfg_read(0x10, AccessWidth::Dword), 0xFFFE_0000);
}

#[test]
fn command_io_space_is_not_implemented_and_intx_disable_is() {
    let mut rig = bare_rig();
    rig.cfg_write(0x04, AccessWidth::Word, 0b111 | (1 << 10));
    let command = rig.cfg_read(0x04, AccessWidth::Word);
    assert_eq!(command & 1, 0, "no I/O BAR");
    assert_eq!(
        command & (COMMAND_MSE | COMMAND_BME),
        COMMAND_MSE | COMMAND_BME
    );
    assert_ne!(command & (1 << 10), 0);
}

#[test]
fn registers_are_not_decoded_before_memory_space_is_enabled() {
    let mut rig = bare_rig();
    assert_eq!(rig.read(CTRL, AccessWidth::Dword).unwrap(), 0xFFFF_FFFF);
    rig.set_reg(IMS, INT_TXDW);
    rig.cfg_write(0x04, AccessWidth::Word, COMMAND_MSE);
    assert_eq!(rig.reg(IMS), 0, "the write was dropped");
}

#[test]
fn moved_bar0_stops_the_decode() {
    let mut rig = rig();
    rig.cfg_write(0x10, AccessWidth::Dword, 0xFC40_0000);
    assert_eq!(rig.read(STATUS, AccessWidth::Dword).unwrap(), 0xFFFF_FFFF);
}

#[test]
fn mac_must_be_unicast_non_zero_and_not_the_gateway() {
    let bad = |mac| NicDevice::new_pci(NicConfig { mac }).err();
    assert_eq!(
        bad([0x01, 0, 0x5E, 0, 0, 1]),
        Some(NicConfigError::MacNotUnicast)
    );
    assert_eq!(bad([0; 6]), Some(NicConfigError::MacIsZero));
    assert_eq!(bad(NIC_GATEWAY_MAC), Some(NicConfigError::MacIsGateway));
    assert!(NicDevice::new_pci(NicConfig { mac: GUEST_MAC }).is_ok());
}

// ---- register access -------------------------------------------------------------------------

#[test]
fn only_aligned_dword_accesses_are_accepted() {
    let mut rig = rig();
    assert_eq!(
        Rig::device_error(rig.read(CTRL, AccessWidth::Byte)),
        DeviceError::UnsupportedWidth
    );
    assert_eq!(
        Rig::device_error(rig.read(CTRL, AccessWidth::Qword)),
        DeviceError::UnsupportedWidth
    );
    assert_eq!(
        Rig::device_error(rig.write(IMS, AccessWidth::Word, 1)),
        DeviceError::UnsupportedWidth
    );
    assert_eq!(
        Rig::device_error(rig.read(CTRL + 2, AccessWidth::Dword)),
        DeviceError::Misaligned
    );
    assert_eq!(rig.reg(IMS), 0, "no refused write changed state");
}

#[test]
fn unmodelled_registers_read_zero_and_ignore_writes_and_are_counted() {
    let mut rig = rig();
    // A statistics register and an unassigned one.
    assert_eq!(rig.reg(0x4000), 0);
    rig.set_reg(0x4000, 0xFFFF_FFFF);
    assert_eq!(rig.reg(0x4000), 0);
    assert_eq!(rig.reg(0x1_0000), 0);
    let counters = rig.nic.counters();
    assert_eq!(counters.unmodelled_reads, 3);
    assert_eq!(counters.unmodelled_writes, 1);
}

#[test]
fn status_reports_a_full_duplex_gigabit_link_and_ignores_writes() {
    let mut rig = rig();
    let status = rig.reg(STATUS);
    assert_eq!(status & 1, 1, "full duplex");
    assert_eq!(status & 2, 2, "link up");
    assert_eq!((status >> 6) & 3, 0b10, "1000 Mb/s");
    rig.set_reg(STATUS, 0);
    assert_eq!(rig.reg(STATUS), status);
}

#[test]
fn latched_registers_keep_what_the_driver_wrote() {
    let mut rig = rig();
    rig.set_reg(0x00C4, 0x1234); // ITR
    assert_eq!(rig.reg(0x00C4), 0x1234);
    assert_eq!(rig.reg(0x0410), 0x0060_200A, "TIPG reset value");
    rig.set_reg(MTA_BASE as u64 + 8, 0xA5A5_0001);
    assert_eq!(rig.reg(MTA_BASE as u64 + 8), 0xA5A5_0001);
}

#[test]
fn receive_address_zero_holds_the_mac_with_the_valid_bit() {
    let mut rig = rig();
    assert_eq!(
        rig.reg(RA_BASE),
        u32::from_le_bytes([0x02, 0x4F, 0x44, 0x00])
    );
    assert_eq!(rig.reg(RA_BASE + 4), 0x1000 | (1 << 31));
    assert_eq!(rig.reg(RA_BASE + 8), 0, "the other pairs are empty");
}

#[test]
fn ring_length_and_pointer_registers_mask_reserved_bits() {
    let mut rig = rig();
    rig.set_reg(TDLEN, 0xFFFF_FFFF);
    assert_eq!(rig.reg(TDLEN), 0x000F_FF80);
    rig.set_reg(TDH, 0xFFFF_FFFF);
    assert_eq!(rig.reg(TDH), 0xFFFF);
    rig.set_reg(RDBAL, 0xFFFF_FFFF);
    assert_eq!(rig.reg(RDBAL), 0xFFFF_FFF0);
}

// ---- reset -----------------------------------------------------------------------------------

#[test]
fn ctrl_rst_returns_registers_to_power_on_values_and_resets_the_backend() {
    let mut rig = rig();
    rig.init_tx(8);
    rig.init_rx(8, RCTL_ACCEPT);
    rig.set_reg(IMS, INT_TXDW);
    rig.set_reg(RA_BASE + 8, 0x1111_1111);
    rig.set_reg(CTRL, CTRL_SLU);
    rig.set_reg(CTRL, CTRL_RST | CTRL_SLU);
    assert_eq!(rig.reg(CTRL) & CTRL_RST, 0, "RST self-clears");
    assert_eq!(rig.reg(CTRL), CTRL_RESET_VALUE);
    for register in [RCTL, TCTL, IMS, TDLEN, RDLEN, TDT, RDT, RA_BASE + 8] {
        assert_eq!(rig.reg(register), 0, "register {register:#x}");
    }
    assert_eq!(rig.reg(RA_BASE + 4), 0x1000 | (1 << 31), "MAC reloaded");
    assert_eq!(rig.script.script().resets, 1);
    // PCI configuration survives.
    assert_ne!(rig.cfg_read(0x04, AccessWidth::Word) & COMMAND_MSE, 0);
    assert_eq!(rig.nic.counters().resets, 1);
}

#[test]
fn platform_reset_also_restores_pci_configuration() {
    let mut rig = rig();
    rig.set_reg(IMS, INT_TXDW);
    rig.nic.platform_reset();
    assert_eq!(rig.cfg_read(0x04, AccessWidth::Word), 0);
    assert_eq!(
        rig.cfg_read(0x10, AccessWidth::Dword),
        NIC_PCI_BAR_BASE as u32
    );
    assert_eq!(rig.script.script().resets, 1);
    assert_eq!(rig.nic.counters().resets, 1);
}

// ---- EEPROM ----------------------------------------------------------------------------------

fn eerd_read(rig: &mut Rig, word: u32) -> u32 {
    rig.set_reg(EERD, EERD_START | (word << 8));
    let value = rig.reg(EERD);
    assert_ne!(value & EERD_DONE, 0);
    value >> 16
}

#[test]
fn eerd_returns_the_mac_words_and_a_valid_checksum() {
    let mut rig = rig();
    assert_eq!(eerd_read(&mut rig, 0), 0x4F02);
    assert_eq!(eerd_read(&mut rig, 1), 0x0044);
    assert_eq!(eerd_read(&mut rig, 2), 0x1000);
    assert_eq!(eerd_read(&mut rig, 0x0D), 0x100E, "device identifier");
    let sum = (0..64).fold(0u16, |sum, word| {
        sum.wrapping_add(eerd_read(&mut rig, word) as u16)
    });
    assert_eq!(sum, 0xBABA);
    assert_eq!(eerd_read(&mut rig, 200), 0, "beyond the part");
}

/// One Microwire READ the way the Linux driver clocks it: request the bus, shift out start,
/// opcode and the address, clock sixteen data bits in, release.
fn microwire_read(rig: &mut Rig, word: u32) -> u16 {
    let eecd = |rig: &mut Rig, value: u32| rig.set_reg(EECD, value);
    eecd(rig, EECD_REQ);
    assert_ne!(rig.reg(EECD) & EECD_GNT, 0, "grant follows the request");
    eecd(rig, EECD_REQ | EECD_CS);
    let command = (0b110 << 6) | word;
    for bit in (0..9).rev() {
        let di = if command >> bit & 1 != 0 { EECD_DI } else { 0 };
        let base = EECD_REQ | EECD_CS | di;
        eecd(rig, base);
        eecd(rig, base | EECD_SK);
        eecd(rig, base);
    }
    let mut data = 0u16;
    for _ in 0..16 {
        eecd(rig, EECD_REQ | EECD_CS | EECD_SK);
        data = (data << 1) | u16::from(rig.reg(EECD) & EECD_DO != 0);
        eecd(rig, EECD_REQ | EECD_CS);
    }
    eecd(rig, EECD_REQ);
    eecd(rig, 0);
    assert_eq!(rig.reg(EECD) & EECD_GNT, 0);
    data
}

#[test]
fn microwire_bit_bang_reads_the_same_words() {
    let mut rig = rig();
    for word in [0u32, 1, 2, 0x0D, 0x3F] {
        let expected = eerd_read(&mut rig, word) as u16;
        assert_eq!(microwire_read(&mut rig, word), expected, "word {word:#x}");
    }
    assert_ne!(rig.reg(EECD) & EECD_PRES, 0);
}

#[test]
fn microwire_ignores_clocks_while_chip_select_is_low() {
    let mut rig = rig();
    rig.set_reg(EECD, EECD_REQ | EECD_SK | EECD_DI);
    rig.set_reg(EECD, EECD_REQ);
    assert_eq!(rig.reg(EECD) & EECD_DO, 0);
    // A normal read still works afterwards.
    assert_eq!(microwire_read(&mut rig, 0), 0x4F02);
}

// ---- PHY -------------------------------------------------------------------------------------

fn mdic(rig: &mut Rig, phy: u32, register: u32, operation: u32, data: u32) -> u32 {
    rig.set_reg(
        MDIC,
        data | (register << MDIC_REG_SHIFT)
            | (phy << MDIC_PHY_SHIFT)
            | (operation << MDIC_OP_SHIFT),
    );
    rig.reg(MDIC)
}

#[test]
fn phy_identifies_as_a_marvell_gigabit_phy_with_link_up() {
    let mut rig = rig();
    let id1 = mdic(&mut rig, 1, 2, MDIC_OP_READ, 0);
    let id2 = mdic(&mut rig, 1, 3, MDIC_OP_READ, 0);
    assert_ne!(id1 & MDIC_READY, 0);
    assert_eq!(id1 & MDIC_ERROR, 0);
    assert_eq!(((id1 & 0xFFFF) << 16) | (id2 & 0xFFFF), 0x0141_0C20);
    let status = mdic(&mut rig, 1, 1, MDIC_OP_READ, 0) & 0xFFFF;
    assert_ne!(status & (1 << 2), 0, "link up");
    assert_ne!(status & (1 << 5), 0, "auto-negotiation complete");
    let specific = mdic(&mut rig, 1, 0x11, MDIC_OP_READ, 0) & 0xFFFF;
    assert_eq!(specific >> 14, 0b10, "1000 Mb/s");
}

#[test]
fn phy_writes_stick_except_on_identity_registers_and_reset_self_clears() {
    let mut rig = rig();
    mdic(&mut rig, 1, 4, MDIC_OP_WRITE, 0x0DE1);
    assert_eq!(mdic(&mut rig, 1, 4, MDIC_OP_READ, 0) & 0xFFFF, 0x0DE1);
    mdic(&mut rig, 1, 2, MDIC_OP_WRITE, 0xFFFF);
    assert_eq!(mdic(&mut rig, 1, 2, MDIC_OP_READ, 0) & 0xFFFF, 0x0141);
    mdic(&mut rig, 1, 0, MDIC_OP_WRITE, 1 << 15);
    assert_eq!(mdic(&mut rig, 1, 0, MDIC_OP_READ, 0) & (1 << 15), 0);
    assert_eq!(
        mdic(&mut rig, 1, 4, MDIC_OP_READ, 0) & 0xFFFF,
        0x01E1,
        "defaults back"
    );
}

#[test]
fn mdic_reports_an_error_for_another_phy_address_or_a_bad_operation() {
    let mut rig = rig();
    let wrong_phy = mdic(&mut rig, 0, 2, MDIC_OP_READ, 0);
    assert_ne!(wrong_phy & MDIC_ERROR, 0);
    assert_ne!(wrong_phy & MDIC_READY, 0);
    assert_eq!(wrong_phy & 0xFFFF, 0);
    assert_ne!(mdic(&mut rig, 1, 2, 0b11, 0) & MDIC_ERROR, 0);
}

#[test]
fn mdic_interrupt_enable_raises_mdac() {
    let mut rig = rig();
    mdic(&mut rig, 1, 2, MDIC_OP_READ, 0);
    assert_eq!(rig.reg(ICR), 0);
    rig.set_reg(
        MDIC,
        MDIC_INT_EN
            | (2 << MDIC_REG_SHIFT)
            | (1 << MDIC_PHY_SHIFT)
            | (MDIC_OP_READ << MDIC_OP_SHIFT),
    );
    assert_eq!(rig.reg(ICR), INT_MDAC);
}

// ---- interrupts ------------------------------------------------------------------------------

#[test]
fn icr_is_read_to_clear_and_the_line_follows_icr_and_ims() {
    let mut rig = rig();
    rig.set_reg(ICS, INT_TXDW | INT_RXT0);
    assert!(!rig.nic.sample_pci_interrupts(), "masked");
    rig.set_reg(IMS, INT_RXT0);
    assert_eq!(rig.reg(IMS), INT_RXT0);
    assert!(rig.nic.sample_pci_interrupts());
    assert_eq!(
        rig.cfg_read(0x04, AccessWidth::Dword) >> 16 & 8,
        8,
        "status.interrupt"
    );
    assert_eq!(rig.reg(ICR), INT_TXDW | INT_RXT0);
    assert_eq!(rig.reg(ICR), 0);
    assert!(!rig.nic.sample_pci_interrupts());
}

#[test]
fn imc_masks_causes_and_reserved_bits_do_not_stick() {
    let mut rig = rig();
    rig.set_reg(IMS, 0xFFFF_FFFF);
    assert_eq!(rig.reg(IMS), INT_MASK);
    rig.set_reg(IMC, INT_TXDW);
    assert_eq!(rig.reg(IMS), INT_MASK & !INT_TXDW);
    rig.set_reg(IMC, 0xFFFF_FFFF);
    assert_eq!(rig.reg(IMS), 0);
}

#[test]
fn writing_ones_to_icr_clears_those_causes() {
    let mut rig = rig();
    rig.set_reg(ICS, INT_TXDW | INT_TXQE);
    rig.set_reg(ICR, INT_TXDW);
    assert_eq!(rig.reg(ICR), INT_TXQE);
}

#[test]
fn intx_disable_silences_the_line() {
    let mut rig = rig();
    rig.set_reg(ICS, INT_TXDW);
    rig.set_reg(IMS, INT_TXDW);
    assert!(rig.nic.sample_pci_interrupts());
    rig.cfg_write(
        0x04,
        AccessWidth::Word,
        COMMAND_MSE | COMMAND_BME | (1 << 10),
    );
    assert!(!rig.nic.sample_pci_interrupts());
}

#[test]
fn setting_slu_raises_a_link_status_change_once() {
    let mut rig = rig();
    rig.set_reg(CTRL, CTRL_SLU);
    assert_eq!(rig.reg(ICR), INT_LSC);
    rig.set_reg(CTRL, CTRL_SLU | CTRL_FD);
    assert_eq!(rig.reg(ICR), 0);
}

// ---- transmit --------------------------------------------------------------------------------

#[test]
fn a_single_descriptor_frame_reaches_the_backend_and_is_written_back() {
    let mut rig = rig();
    rig.init_tx(8);
    rig.set_reg(IMS, INT_TXDW | INT_TXQE);
    let frame = frame_to(NIC_GATEWAY_MAC, 60);
    rig.put_tx(0, &frame, CMD_EOP | CMD_RS);
    rig.set_reg(TDT, 1);
    assert!(rig.nic.wakeup_pending(), "the doorbell ends the quantum");
    let report = rig.service();
    assert_eq!(report.tx_descriptors, 1);
    assert!(!rig.nic.wakeup_pending());
    assert_eq!(rig.script.script().sent, vec![frame]);
    assert_eq!(rig.tx_status(0), STA_DD);
    assert_eq!(rig.reg(TDH), 1);
    assert_eq!(rig.reg(ICR), INT_TXDW | INT_TXQE);
    let counters = rig.nic.counters();
    assert_eq!((counters.tx_frames, counters.tx_bytes), (1, 60));
}

#[test]
fn nothing_is_transmitted_without_the_doorbell_the_enable_bit_or_bus_mastering() {
    let mut rig = rig();
    rig.init_tx(8);
    rig.put_tx(0, &frame_to(NIC_GATEWAY_MAC, 60), CMD_EOP | CMD_RS);
    assert_eq!(rig.service().tx_descriptors, 0, "TDH == TDT");
    rig.set_reg(TDT, 1);
    rig.set_reg(TCTL, 0);
    assert_eq!(rig.service().tx_descriptors, 0, "TCTL.EN clear");
    rig.set_reg(TCTL, TCTL_EN);
    rig.cfg_write(0x04, AccessWidth::Word, COMMAND_MSE);
    assert_eq!(rig.service().tx_descriptors, 0, "no bus mastering");
    assert!(rig.script.script().sent.is_empty());
    rig.cfg_write(0x04, AccessWidth::Word, COMMAND_MSE | COMMAND_BME);
    assert_eq!(rig.service().tx_descriptors, 1);
}

#[test]
fn a_frame_in_several_descriptors_is_assembled_up_to_the_eop() {
    let mut rig = rig();
    rig.init_tx(8);
    let frame = frame_to(NIC_GATEWAY_MAC, 200);
    rig.put_tx(0, &frame[..14], 0);
    rig.put_tx(1, &[], 0); // a zero-length descriptor in the chain
    rig.put_tx(2, &frame[14..100], CMD_RS);
    rig.put_tx(3, &frame[100..], CMD_EOP | CMD_RS);
    rig.set_reg(TDT, 4);
    assert_eq!(rig.service().tx_descriptors, 4);
    assert_eq!(rig.script.script().sent, vec![frame]);
    assert_eq!(rig.tx_status(0), 0, "no write-back without RS");
    assert_eq!(rig.tx_status(2), STA_DD);
    assert_eq!(rig.tx_status(3), STA_DD);
}

#[test]
fn the_ring_wraps_and_a_second_pass_reuses_the_descriptors() {
    let mut rig = rig();
    rig.init_tx(8);
    for round in 0..3u8 {
        for index in 0..8 {
            let tail = (index + 1) % 8;
            let frame = frame_to(NIC_GATEWAY_MAC, 60 + usize::from(round));
            rig.put_tx(index, &frame, CMD_EOP | CMD_RS);
            rig.set_reg(TDT, tail);
            rig.service();
        }
    }
    assert_eq!(rig.script.script().sent.len(), 24);
    assert_eq!(rig.reg(TDH), 0);
}

#[test]
fn a_frame_the_backend_refuses_still_completes_for_the_guest() {
    let mut rig = rig();
    rig.init_tx(8);
    rig.script.script().refuse = Some(Dropped::QueueFull);
    rig.put_tx(0, &frame_to(NIC_GATEWAY_MAC, 60), CMD_EOP | CMD_RS);
    rig.set_reg(TDT, 1);
    rig.service();
    assert_eq!(rig.tx_status(0), STA_DD);
    assert_eq!(rig.reg(TDH), 1);
    let counters = rig.nic.counters();
    assert_eq!((counters.tx_frames, counters.tx_backend_dropped), (0, 1));
}

#[test]
fn without_a_backend_frames_complete_and_are_counted() {
    let mut rig = build_rig(false);
    rig.cfg_write(0x04, AccessWidth::Word, COMMAND_MSE | COMMAND_BME);
    rig.init_tx(8);
    rig.init_rx(8, RCTL_ACCEPT);
    rig.put_tx(0, &frame_to(NIC_GATEWAY_MAC, 60), CMD_EOP | CMD_RS);
    rig.set_reg(TDT, 1);
    let report = rig.service();
    assert_eq!((report.tx_descriptors, report.rx_frames), (1, 0));
    assert_eq!(rig.tx_status(0), STA_DD);
    assert_eq!(rig.nic.counters().tx_no_backend, 1);
    assert_eq!(rig.nic.backend_counters(), None);
}

#[test]
fn only_one_backend_can_be_attached() {
    let rig = rig();
    assert_eq!(
        rig.nic.attach_backend(Box::new(ScriptedBackend::default())),
        Err(NicAttachError::AlreadyAttached)
    );
}

#[test]
fn extended_descriptors_drop_their_packet_and_complete() {
    let mut rig = rig();
    rig.init_tx(8);
    // A context descriptor, then a data descriptor of the same packet, then a good frame.
    rig.put_tx(0, &[0u8; 8], CMD_DEXT | CMD_RS);
    rig.put_tx(
        1,
        &frame_to(NIC_GATEWAY_MAC, 60),
        CMD_DEXT | CMD_EOP | CMD_RS,
    );
    let good = frame_to(NIC_GATEWAY_MAC, 64);
    rig.put_tx(2, &good, CMD_EOP | CMD_RS);
    rig.set_reg(TDT, 3);
    assert_eq!(rig.service().tx_descriptors, 3);
    assert_eq!(rig.script.script().sent, vec![good]);
    assert_eq!(rig.tx_status(0), STA_DD);
    assert_eq!(rig.tx_status(1), STA_DD);
    assert_eq!(rig.nic.counters().tx_discarded, 1);
}

#[test]
fn a_chain_longer_than_the_assembly_limit_is_dropped_as_a_whole() {
    let mut rig = rig();
    rig.init_tx(16);
    // Five descriptors of 4000 bytes: the fifth crosses 16 KiB.
    for index in 0..5 {
        let command = if index == 4 { CMD_EOP | CMD_RS } else { 0 };
        rig.put_tx(index, &[0x55; 4000], command);
    }
    rig.set_reg(TDT, 5);
    rig.service();
    assert!(rig.script.script().sent.is_empty());
    assert_eq!(rig.nic.counters().tx_discarded, 1);
    assert_eq!(rig.tx_status(4), STA_DD);
    // The next frame is unaffected.
    rig.put_tx(5, &frame_to(NIC_GATEWAY_MAC, 60), CMD_EOP | CMD_RS);
    rig.set_reg(TDT, 6);
    rig.service();
    assert_eq!(rig.script.script().sent.len(), 1);
}

#[test]
fn an_oversize_frame_goes_to_the_backend_which_decides() {
    let mut rig = rig();
    rig.init_tx(8);
    rig.put_tx(0, &vec![0x11; 3000], CMD_EOP | CMD_RS);
    rig.set_reg(TDT, 1);
    rig.service();
    assert_eq!(rig.script.script().sent[0].len(), 3000);
}

#[test]
fn a_buffer_outside_ram_drops_the_packet_and_counts_a_dma_error() {
    let mut rig = rig();
    rig.init_tx(8);
    let mut descriptor = [0u8; 16];
    descriptor[0..8].copy_from_slice(&UNMAPPED.to_le_bytes());
    descriptor[8..10].copy_from_slice(&60u16.to_le_bytes());
    descriptor[11] = CMD_EOP | CMD_RS;
    rig.mem_write(TX_RING, &descriptor);
    rig.set_reg(TDT, 1);
    assert_eq!(rig.service().tx_descriptors, 1);
    assert!(rig.script.script().sent.is_empty());
    assert_eq!(rig.nic.counters().dma_errors, 1);
    assert_eq!(rig.tx_status(0), STA_DD);
}

#[test]
fn a_ring_outside_ram_stops_the_queue_until_it_is_reprogrammed() {
    let mut rig = rig();
    rig.init_tx(8);
    rig.set_reg(TDBAL, UNMAPPED as u32);
    rig.set_reg(TDT, 1);
    assert_eq!(rig.service().tx_descriptors, 0);
    assert_eq!(rig.nic.counters().dma_errors, 1);
    // Latched: the next call does not try again.
    rig.set_reg(TDT, 2);
    rig.service();
    assert_eq!(rig.nic.counters().dma_errors, 1);
    // Reprogramming the ring clears the fault.
    rig.set_reg(TDBAL, TX_RING as u32);
    rig.put_tx(0, &frame_to(NIC_GATEWAY_MAC, 60), CMD_EOP | CMD_RS);
    rig.put_tx(1, &frame_to(NIC_GATEWAY_MAC, 60), CMD_EOP | CMD_RS);
    assert_eq!(rig.service().tx_descriptors, 2);
}

#[test]
fn a_head_or_tail_outside_the_ring_is_refused_not_walked() {
    let mut rig = rig();
    rig.init_tx(8);
    rig.set_reg(TDT, 9);
    assert_eq!(rig.service().tx_descriptors, 0);
    assert_eq!(rig.nic.counters().ring_errors, 1);
    assert_eq!(rig.reg(TDH), 0);
    // A zero-length ring is simply not configured.
    rig.set_reg(TDLEN, 0);
    assert_eq!(rig.service().tx_descriptors, 0);
}

#[test]
fn a_backlog_beyond_the_budget_keeps_the_wakeup_flag() {
    let mut rig = rig();
    let count = 512u32;
    rig.init_tx(count);
    for index in 0..300u32 {
        let mut descriptor = [0u8; 16];
        descriptor[8..10].copy_from_slice(&0u16.to_le_bytes());
        descriptor[11] = CMD_EOP;
        rig.mem_write(TX_RING + u64::from(index) * 16, &descriptor);
    }
    rig.set_reg(TDT, 300);
    assert_eq!(rig.service().tx_descriptors, TX_BUDGET);
    assert!(rig.nic.wakeup_pending());
    assert_eq!(rig.service().tx_descriptors, 300 - TX_BUDGET);
    assert!(!rig.nic.wakeup_pending());
}

// ---- receive ---------------------------------------------------------------------------------

#[test]
fn a_frame_for_the_guest_fills_a_descriptor() {
    let mut rig = rig();
    rig.init_rx(8, RCTL_ACCEPT);
    rig.set_reg(IMS, INT_RXT0);
    let frame = frame_to(GUEST_MAC, 100);
    rig.script.script().to_guest.push_back(frame.clone());
    assert_eq!(rig.service().rx_frames, 1);
    let descriptor = rig.rx_descriptor(0);
    assert_eq!(u16::from_le_bytes([descriptor[8], descriptor[9]]), 100);
    assert_eq!(descriptor[12], STA_DD | STA_EOP | STA_IXSM);
    assert_eq!(descriptor[13], 0, "no errors");
    assert_eq!(rig.rx_buffer(0, 100), frame);
    assert_eq!(rig.reg(RDH), 1);
    assert!(rig.nic.sample_pci_interrupts());
    assert_eq!(rig.reg(ICR) & INT_RXT0, INT_RXT0);
    let counters = rig.nic.counters();
    assert_eq!((counters.rx_frames, counters.rx_bytes), (1, 100));
}

#[test]
fn short_frames_are_padded_to_60_bytes() {
    let mut rig = rig();
    rig.init_rx(8, RCTL_ACCEPT);
    let frame = frame_to(GUEST_MAC, 42);
    rig.script.script().to_guest.push_back(frame.clone());
    rig.service();
    let descriptor = rig.rx_descriptor(0);
    assert_eq!(u16::from_le_bytes([descriptor[8], descriptor[9]]), 60);
    let stored = rig.rx_buffer(0, 60);
    assert_eq!(&stored[..42], &frame[..]);
    assert!(stored[42..].iter().all(|byte| *byte == 0));
}

#[test]
fn without_secrc_the_fcs_is_appended() {
    let mut rig = rig();
    rig.init_rx(8, RCTL_EN | RCTL_BAM);
    let frame = frame_to(GUEST_MAC, 100);
    rig.script.script().to_guest.push_back(frame.clone());
    rig.service();
    let descriptor = rig.rx_descriptor(0);
    assert_eq!(u16::from_le_bytes([descriptor[8], descriptor[9]]), 104);
    let stored = rig.rx_buffer(0, 104);
    assert_eq!(&stored[..100], &frame[..]);
    assert_eq!(
        u32::from_le_bytes(stored[100..].try_into().unwrap()),
        crate::crc32::crc32_ieee(&[&frame])
    );
}

#[test]
fn the_backend_is_polled_only_while_a_descriptor_is_free() {
    let mut rig = rig();
    rig.init_rx(8, RCTL_ACCEPT);
    for _ in 0..10 {
        rig.script
            .script()
            .to_guest
            .push_back(frame_to(GUEST_MAC, 60));
    }
    // Seven descriptors are free (RDH = 0, RDT = 7): seven frames are delivered.
    assert_eq!(rig.service().rx_frames, 7);
    assert_eq!(
        rig.script.script().polls,
        7,
        "no poll with no free descriptor"
    );
    assert_eq!(rig.script.script().to_guest.len(), 3);
    assert_eq!(rig.service().rx_frames, 0);
    assert_eq!(rig.script.script().polls, 7);
    // The driver returns two descriptors.
    rig.set_reg(RDT, 1);
    assert_eq!(rig.service().rx_frames, 2);
    assert_eq!(rig.script.script().to_guest.len(), 1);
}

#[test]
fn a_ring_of_fewer_than_eight_descriptors_is_not_a_ring() {
    let mut rig = rig();
    rig.init_rx(4, RCTL_ACCEPT);
    assert_eq!(rig.reg(RDLEN), 0, "the length is a multiple of 128 bytes");
    rig.script
        .script()
        .to_guest
        .push_back(frame_to(GUEST_MAC, 60));
    assert_eq!(rig.service().rx_frames, 0);
    assert_eq!(rig.script.script().polls, 0);
}

#[test]
fn a_frame_in_several_buffers_ends_with_eop_in_the_last() {
    let mut rig = rig();
    // 256-byte buffers: BSIZE = 3.
    rig.init_rx(16, RCTL_ACCEPT | (3 << RCTL_BSIZE_SHIFT));
    let frame = frame_to(GUEST_MAC, 600);
    rig.script.script().to_guest.push_back(frame.clone());
    rig.service();
    assert_eq!(rig.reg(RDH), 3, "600 bytes need three 256-byte buffers");
    let lengths: Vec<u16> = (0..3)
        .map(|index| {
            let descriptor = rig.rx_descriptor(index);
            u16::from_le_bytes([descriptor[8], descriptor[9]])
        })
        .collect();
    assert_eq!(lengths, [256, 256, 88]);
    assert_eq!(rig.rx_descriptor(0)[12], STA_DD | STA_IXSM);
    assert_eq!(rig.rx_descriptor(1)[12], STA_DD | STA_IXSM);
    assert_eq!(rig.rx_descriptor(2)[12], STA_DD | STA_EOP | STA_IXSM);
    let mut joined = rig.rx_buffer(0, 256);
    joined.extend(rig.rx_buffer(1, 256));
    joined.extend(rig.rx_buffer(2, 88));
    assert_eq!(joined, frame);
}

#[test]
fn a_frame_waits_when_the_free_descriptors_are_too_few() {
    let mut rig = rig();
    rig.init_rx(8, RCTL_ACCEPT | (3 << RCTL_BSIZE_SHIFT));
    rig.set_reg(RDT, 2); // two free descriptors
    let frame = frame_to(GUEST_MAC, 600); // needs three
    rig.script.script().to_guest.push_back(frame.clone());
    assert_eq!(rig.service().rx_frames, 0);
    assert_eq!(rig.reg(RDH), 0);
    // The frame stays staged: no second poll, nothing lost.
    assert_eq!(rig.service().rx_frames, 0);
    assert_eq!(rig.script.script().polls, 1);
    rig.set_reg(RDT, 5);
    assert_eq!(rig.service().rx_frames, 1);
    assert_eq!(rig.reg(RDH), 3);
}

#[test]
fn the_largest_frame_in_the_smallest_buffers_fits_the_smallest_ring() {
    let mut rig = rig();
    // 1514 bytes in 256-byte buffers is six descriptors; the ring of eight offers seven.
    rig.init_rx(8, RCTL_ACCEPT | (3 << RCTL_BSIZE_SHIFT));
    rig.script
        .script()
        .to_guest
        .push_back(frame_to(GUEST_MAC, NET_MAX_FRAME));
    assert_eq!(rig.service().rx_frames, 1);
    assert_eq!(rig.reg(RDH), 6);
}

#[test]
fn the_receive_filter_follows_rctl_and_the_address_tables() {
    let mut rig = rig();
    rig.init_rx(32, RCTL_EN | RCTL_SECRC);
    let other = [0x02, 0x00, 0x00, 0x00, 0x00, 0x99];
    let multicast = [0x01, 0x00, 0x5E, 0x00, 0x00, 0xFB];
    for destination in [GUEST_MAC, other, [0xFF; 6], multicast] {
        rig.script
            .script()
            .to_guest
            .push_back(frame_to(destination, 60));
    }
    // Exact match only: the unicast frame passes; broadcast needs BAM.
    assert_eq!(rig.service().rx_frames, 1);
    assert_eq!(rig.nic.counters().rx_filtered, 3);
    // BAM.
    rig.set_reg(RCTL, RCTL_EN | RCTL_SECRC | RCTL_BAM);
    rig.script
        .script()
        .to_guest
        .push_back(frame_to([0xFF; 6], 60));
    assert_eq!(rig.service().rx_frames, 1);
    // Unicast promiscuous passes the foreign unicast, not the multicast.
    rig.set_reg(RCTL, RCTL_EN | RCTL_SECRC | RCTL_UPE);
    rig.script.script().to_guest.push_back(frame_to(other, 60));
    rig.script
        .script()
        .to_guest
        .push_back(frame_to(multicast, 60));
    assert_eq!(rig.service().rx_frames, 1);
    // Multicast promiscuous passes the multicast.
    rig.set_reg(RCTL, RCTL_EN | RCTL_SECRC | RCTL_MPE);
    rig.script
        .script()
        .to_guest
        .push_back(frame_to(multicast, 60));
    assert_eq!(rig.service().rx_frames, 1);
}

#[test]
fn the_multicast_hash_table_selects_by_twelve_address_bits() {
    let mut rig = rig();
    rig.init_rx(8, RCTL_EN | RCTL_SECRC);
    let multicast = [0x01, 0x00, 0x5E, 0x00, 0x12, 0x34];
    // Type 0 (RCTL.MO = 0): hash = (byte4 >> 4) | (byte5 << 4) over 12 bits.
    let hash = (u32::from(multicast[4]) >> 4 | u32::from(multicast[5]) << 4) & 0xFFF;
    rig.script
        .script()
        .to_guest
        .push_back(frame_to(multicast, 60));
    assert_eq!(rig.service().rx_frames, 0, "table empty");
    rig.set_reg(MTA_BASE + u64::from(hash >> 5) * 4, 1 << (hash & 31));
    rig.script
        .script()
        .to_guest
        .push_back(frame_to(multicast, 60));
    assert_eq!(rig.service().rx_frames, 1);
    // Another address with another hash stays out.
    rig.script
        .script()
        .to_guest
        .push_back(frame_to([0x01, 0, 0x5E, 0, 0x12, 0x35], 60));
    assert_eq!(rig.service().rx_frames, 0);
}

#[test]
fn a_second_receive_address_pair_accepts_its_address() {
    let mut rig = rig();
    rig.init_rx(8, RCTL_EN | RCTL_SECRC);
    let extra = [0x02, 0x00, 0x00, 0x00, 0x00, 0x77];
    rig.set_reg(RA_BASE + 8, u32::from_le_bytes([0x02, 0, 0, 0]));
    // Bytes 4 and 5 of the address, low byte first; the valid bit is not set yet.
    rig.set_reg(RA_BASE + 12, 0x7700);
    rig.script.script().to_guest.push_back(frame_to(extra, 60));
    assert_eq!(rig.service().rx_frames, 0);
    rig.set_reg(RA_BASE + 12, 0x7700 | (1 << 31));
    rig.script.script().to_guest.push_back(frame_to(extra, 60));
    assert_eq!(rig.service().rx_frames, 1);
}

#[test]
fn rxdmt0_is_raised_when_free_descriptors_fall_to_the_threshold() {
    let mut rig = rig();
    rig.init_rx(8, RCTL_ACCEPT); // RDMTS 0: half of the ring
    for _ in 0..3 {
        rig.script
            .script()
            .to_guest
            .push_back(frame_to(GUEST_MAC, 60));
    }
    rig.service();
    // Seven free descriptors at the start, four after three frames: threshold is 4.
    assert_eq!(rig.reg(ICR) & INT_RXDMT0, INT_RXDMT0);
}

#[test]
fn reception_is_off_until_rctl_en_and_with_a_broken_ring() {
    let mut rig = rig();
    rig.init_rx(8, RCTL_BAM | RCTL_SECRC);
    rig.script
        .script()
        .to_guest
        .push_back(frame_to(GUEST_MAC, 60));
    assert_eq!(rig.service().rx_frames, 0);
    assert_eq!(rig.script.script().polls, 0, "no poll with RCTL.EN clear");
    rig.set_reg(RCTL, RCTL_ACCEPT);
    rig.set_reg(RDH, 12); // outside the ring
    assert_eq!(rig.service().rx_frames, 0);
    assert_eq!(rig.nic.counters().ring_errors, 1);
}

#[test]
fn a_receive_ring_outside_ram_latches_a_fault_and_drops_the_frame() {
    let mut rig = rig();
    rig.init_rx(8, RCTL_ACCEPT);
    rig.set_reg(RDBAL, UNMAPPED as u32);
    rig.script
        .script()
        .to_guest
        .push_back(frame_to(GUEST_MAC, 60));
    assert_eq!(rig.service().rx_frames, 0);
    assert_eq!(rig.nic.counters().dma_errors, 1);
    assert_eq!(rig.reg(RDH), 0);
    rig.service();
    assert_eq!(rig.nic.counters().dma_errors, 1, "latched");
}

#[test]
fn a_receive_buffer_outside_ram_leaves_the_descriptor_unused() {
    let mut rig = rig();
    rig.init_rx(8, RCTL_ACCEPT);
    let mut descriptor = [0u8; 16];
    descriptor[0..8].copy_from_slice(&UNMAPPED.to_le_bytes());
    rig.mem_write(RX_RING, &descriptor);
    rig.script
        .script()
        .to_guest
        .push_back(frame_to(GUEST_MAC, 60));
    assert_eq!(rig.service().rx_frames, 0);
    assert_eq!(rig.nic.counters().dma_errors, 1);
    assert_eq!(rig.reg(RDH), 0);
    assert_eq!(rig.rx_descriptor(0)[12], 0);
}

#[test]
fn a_reset_forgets_a_staged_frame() {
    let mut rig = rig();
    rig.init_rx(8, RCTL_ACCEPT | (3 << RCTL_BSIZE_SHIFT));
    rig.set_reg(RDT, 1);
    rig.script
        .script()
        .to_guest
        .push_back(frame_to(GUEST_MAC, 600));
    rig.service();
    assert_eq!(
        rig.script.script().to_guest.len(),
        0,
        "staged inside the NIC"
    );
    rig.set_reg(CTRL, CTRL_RST);
    rig.init_rx(8, RCTL_ACCEPT);
    assert_eq!(rig.service().rx_frames, 0, "the staged frame is gone");
}

// ---- backend loop ----------------------------------------------------------------------------

#[test]
fn transmit_then_receive_round_trips_through_the_scripted_backend() {
    let mut rig = rig();
    rig.init_tx(8);
    rig.init_rx(8, RCTL_ACCEPT);
    rig.put_tx(0, &frame_to(NIC_GATEWAY_MAC, 60), CMD_EOP | CMD_RS);
    rig.set_reg(TDT, 1);
    rig.script
        .script()
        .to_guest
        .push_back(frame_to(GUEST_MAC, 70));
    let report = rig.service();
    assert_eq!((report.tx_descriptors, report.rx_frames), (1, 1));
    assert_eq!(rig.nic.backend_counters().unwrap().tx_frames, 1);
    assert_eq!(rig.nic.mac(), GUEST_MAC);
}
