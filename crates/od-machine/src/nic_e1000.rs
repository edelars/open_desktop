//! NIC model compatible with the Intel 82540EM (PCI 8086:100E) for the board's PCI function
//! 00:06.0 (S11-32). Contract: `docs/specs/NIC_DEVICE.md`.
//!
//! The device is a `BusDevice` on BAR0 (128 KiB of registers). A `BusDevice` cannot reach guest
//! RAM, so register writes only record state; the machine owner calls [`NicHandle::service`]
//! between guest quanta, which walks the legacy transmit descriptors, hands complete frames to
//! the [`NetBackend`] and delivers the backend's frames into the receive ring. Interrupts are
//! one level (ICR & IMS) on PCI INTA, routed to IOAPIC GSI 18.

mod eeprom;
pub(crate) mod irq_source;
mod phy;
mod regs;
mod rings;
#[cfg(test)]
mod tests;

use self::eeprom::Microwire;
use self::phy::Phy;
use self::regs::*;
use crate::net_backend::{NET_MAX_FRAME, NetBackend, NetCounters};
use crate::nvme::{BusDma, GuestDma};
use crate::pci_ecam::{
    COMMAND_BUS_MASTER, COMMAND_INTX_DISABLE, COMMAND_MEMORY_ENABLE, PciFunction, Type0Config,
};
use crate::{
    AccessWidth, BusDevice, DeviceAccess, DeviceError, GuestPhysicalRange, IoSpace, MachineBus,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// BAR0 of PCI function 00:06.0: 32-bit non-prefetchable memory, 128 KiB, programmed at board
/// creation (NIC_DEVICE.md section 2).
pub const NIC_PCI_BAR_BASE: u64 = 0xFC02_0000;
pub const NIC_PCI_BAR_BYTES: u64 = 0x2_0000;
/// PCI device number of the NIC on bus 0 (function 0).
pub const NIC_PCI_SLOT: u8 = 6;
/// Fixed route of INTA to the IO-APIC (NVMe uses GSI 16, xHCI GSI 17).
pub const NIC_INTX_GSI: u32 = 18;
pub const NIC_VENDOR_ID: u16 = 0x8086;
pub const NIC_DEVICE_ID: u16 = 0x100E;
/// MAC address of the backend's gateway and DNS (NETWORK.md section 3): a guest must not use it.
/// The NAT backend in `od-host` keeps its own copy; a test there compares the two.
pub const NIC_GATEWAY_MAC: [u8; 6] = [0x02, 0x4F, 0x44, 0x00, 0x00, 0x02];

/// Descriptors the transmit side handles in one `service` call, and received frames the receive
/// side delivers in one call. A larger backlog keeps the wake-up flag set.
const TX_BUDGET: u32 = 256;
const RX_BUDGET: u32 = 64;
/// Largest frame the transmit side assembles from descriptors (the jumbo limit of the chip); a
/// longer chain is dropped as a whole.
const TX_ASSEMBLY_LIMIT: usize = 16 * 1024;

/// What the host configures; the rest of the NIC is fixed by the contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NicConfig {
    /// MAC address of the guest's NIC (from the identity bundle). Unicast, not all zero and not
    /// [`NIC_GATEWAY_MAC`] (NETWORK.md section 3).
    pub mac: [u8; 6],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NicConfigError {
    /// The group bit (bit 0 of the first octet) is set.
    MacNotUnicast,
    MacIsZero,
    MacIsGateway,
}

impl NicConfig {
    pub fn validate(&self) -> Result<(), NicConfigError> {
        if self.mac[0] & 1 != 0 {
            Err(NicConfigError::MacNotUnicast)
        } else if self.mac == [0; 6] {
            Err(NicConfigError::MacIsZero)
        } else if self.mac == NIC_GATEWAY_MAC {
            Err(NicConfigError::MacIsGateway)
        } else {
            Ok(())
        }
    }
}

/// [`NicHandle::attach_backend`] refused the backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NicAttachError {
    AlreadyAttached,
}

/// Monotonic diagnostics of the model; they survive resets. Frames are counted as the NIC sees
/// them; the backend keeps its own counters ([`NetCounters`]).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NicCounters {
    /// Frames the backend accepted, and their bytes.
    pub tx_frames: u64,
    pub tx_bytes: u64,
    /// Frames the backend refused at once (still a successful transmission for the guest).
    pub tx_backend_dropped: u64,
    /// Frames completed with no backend attached.
    pub tx_no_backend: u64,
    /// Chains dropped: an extended descriptor (not implemented) or more than 16 KiB.
    pub tx_discarded: u64,
    pub tx_descriptors: u64,
    pub rx_frames: u64,
    pub rx_bytes: u64,
    pub rx_descriptors: u64,
    /// Frames the receive filter rejected.
    pub rx_filtered: u64,
    /// Frames dropped because the backend reported a length beyond the 1514-byte buffer.
    pub rx_bad_backend_length: u64,
    /// Guest RAM accesses that failed (a ring or a buffer outside RAM, or an address overflow).
    pub dma_errors: u64,
    /// Servicing skipped because the head or the tail lies outside the ring.
    pub ring_errors: u64,
    /// Accesses to registers the model does not implement (reads return 0, writes are ignored).
    pub unmodelled_reads: u64,
    pub unmodelled_writes: u64,
    pub resets: u64,
}

/// Work done by one [`NicHandle::service`] call.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NicServiceReport {
    /// Transmit descriptors consumed.
    pub tx_descriptors: u32,
    /// Frames delivered into the receive ring.
    pub rx_frames: u32,
}

/// Everything CTRL.RST resets: the registers, the rings' pointers and the frame being assembled.
struct Hw {
    ctrl: u32,
    ctrl_ext: u32,
    eecd: u32,
    microwire: Microwire,
    eerd: u32,
    mdic: u32,
    phy: Phy,
    icr: u32,
    ims: u32,
    rctl: u32,
    tctl: u32,
    rdba: u64,
    rdlen: u32,
    rdh: u32,
    rdt: u32,
    tdba: u64,
    tdlen: u32,
    tdh: u32,
    tdt: u32,
    mta: [u32; MTA_ENTRIES],
    ra: [[u32; 2]; RA_ENTRIES],
    latched: [u32; LATCHED.len()],
    /// The chain being assembled is dropped at its last descriptor.
    tx_discard: bool,
    /// A descriptor fetch failed: the ring is not served until the driver reprograms it.
    tx_fault: bool,
    rx_fault: bool,
    tx_frame: Vec<u8>,
}

impl Hw {
    fn new(mac: [u8; 6]) -> Self {
        let mut ra = [[0u32; 2]; RA_ENTRIES];
        ra[0] = [
            u32::from_le_bytes([mac[0], mac[1], mac[2], mac[3]]),
            u32::from(mac[4]) | (u32::from(mac[5]) << 8) | RAH_VALID,
        ];
        Self {
            ctrl: CTRL_RESET_VALUE,
            ctrl_ext: 0,
            eecd: 0,
            microwire: Microwire::new(),
            eerd: 0,
            mdic: MDIC_READY,
            phy: Phy::new(),
            icr: 0,
            ims: 0,
            rctl: 0,
            tctl: 0,
            rdba: 0,
            rdlen: 0,
            rdh: 0,
            rdt: 0,
            tdba: 0,
            tdlen: 0,
            tdh: 0,
            tdt: 0,
            mta: [0; MTA_ENTRIES],
            ra,
            latched: std::array::from_fn(|index| LATCHED[index].1),
            tx_discard: false,
            tx_fault: false,
            rx_fault: false,
            tx_frame: Vec::with_capacity(NET_MAX_FRAME),
        }
    }
}

impl Hw {
    /// The driver reprogrammed the transmit ring: the packet assembled so far (descriptors up to
    /// a missing EOP) and the fault latch belong to the old ring and go.
    fn reset_tx_chain(&mut self) {
        self.tx_frame.clear();
        self.tx_discard = false;
        self.tx_fault = false;
    }
}

/// Address valid bit of the high dword of a receive address pair.
const RAH_VALID: u32 = 1 << 31;

struct NicState {
    mac: [u8; 6],
    eeprom: [u16; eeprom::WORDS],
    pci: Type0Config,
    hw: Hw,
    backend: Option<Box<dyn NetBackend>>,
    /// Frame taken from the backend and not delivered yet (no free descriptors), and its buffer.
    rx_staged: Option<usize>,
    rx_buffer: Box<[u8]>,
    counters: NicCounters,
    host_dirty: Arc<AtomicBool>,
}

impl NicState {
    fn new(config: NicConfig, host_dirty: Arc<AtomicBool>) -> Self {
        Self {
            mac: config.mac,
            eeprom: eeprom::image(config.mac),
            pci: Self::initial_pci(),
            hw: Hw::new(config.mac),
            backend: None,
            rx_staged: None,
            rx_buffer: vec![0u8; NET_MAX_FRAME].into_boxed_slice(),
            counters: NicCounters::default(),
            host_dirty,
        }
    }

    /// The PCI configuration at board creation: 8086:100E, revision 2, class 0x020000 (Ethernet),
    /// BAR0 programmed at [`NIC_PCI_BAR_BASE`], INTA.
    fn initial_pci() -> Type0Config {
        let mut pci = Type0Config::new(
            NIC_VENDOR_ID,
            NIC_DEVICE_ID,
            0x02,
            0x02_00_00,
            Some(NIC_PCI_BAR_BYTES as u32),
        )
        .expect("the NIC has a valid constant class and BAR size");
        pci.configure_32_bit_memory_bar0(NIC_PCI_BAR_BASE as u32);
        pci.configure_inta();
        pci
    }

    /// CTRL.RST: the registers return to their power-on values (the MAC is reloaded from the
    /// EEPROM image), the staged frame and the backend's flows go. PCI configuration stays.
    fn device_reset(&mut self) {
        self.hw = Hw::new(self.mac);
        self.rx_staged = None;
        self.counters.resets += 1;
        if let Some(backend) = self.backend.as_mut() {
            backend.reset();
        }
    }

    /// Board reset: the device reset and the PCI configuration back to its values at creation.
    fn platform_reset(&mut self) {
        self.device_reset();
        self.pci = Self::initial_pci();
        self.host_dirty.store(false, Ordering::Relaxed);
    }

    fn memory_decode_enabled(&self) -> bool {
        self.pci.command() & COMMAND_MEMORY_ENABLE != 0
            && self.pci.memory_bar0_address() == NIC_PCI_BAR_BASE
    }

    fn dma_enabled(&self) -> bool {
        self.pci.command() & COMMAND_BUS_MASTER != 0
    }

    /// INTx level: any enabled cause pending, unless Command.INTx Disable is set.
    fn interrupt_level(&self) -> bool {
        self.hw.icr & self.hw.ims != 0 && self.pci.command() & COMMAND_INTX_DISABLE == 0
    }

    fn check_access(offset: u64, width: AccessWidth) -> Result<(), DeviceError> {
        if width != AccessWidth::Dword {
            return Err(DeviceError::UnsupportedWidth);
        }
        if !offset.is_multiple_of(4) {
            return Err(DeviceError::Misaligned);
        }
        if offset >= NIC_PCI_BAR_BYTES {
            return Err(DeviceError::UnknownOffset);
        }
        Ok(())
    }

    fn read_register(&mut self, offset: u64, width: AccessWidth) -> Result<u64, DeviceError> {
        Self::check_access(offset, width)?;
        Ok(u64::from(self.read_dword(offset)))
    }

    fn write_register(
        &mut self,
        offset: u64,
        width: AccessWidth,
        value: u64,
    ) -> Result<(), DeviceError> {
        Self::check_access(offset, width)?;
        self.write_dword(offset, value as u32);
        Ok(())
    }

    fn read_dword(&mut self, offset: u64) -> u32 {
        let hw = &self.hw;
        match offset {
            CTRL => hw.ctrl,
            STATUS => STATUS_VALUE,
            EECD => {
                let mut value = hw.eecd | EECD_PRES;
                if hw.eecd & EECD_REQ != 0 {
                    value |= EECD_GNT;
                }
                if hw.microwire.data_out() {
                    value |= EECD_DO;
                }
                value
            }
            EERD => hw.eerd,
            CTRL_EXT => hw.ctrl_ext,
            MDIC => hw.mdic,
            // Reading the cause register clears it.
            ICR => std::mem::take(&mut self.hw.icr),
            IMS => hw.ims,
            RCTL => hw.rctl,
            RDBAL => hw.rdba as u32,
            RDBAH => (hw.rdba >> 32) as u32,
            RDLEN => hw.rdlen,
            RDH => hw.rdh,
            RDT => hw.rdt,
            TCTL => hw.tctl,
            TDBAL => hw.tdba as u32,
            TDBAH => (hw.tdba >> 32) as u32,
            TDLEN => hw.tdlen,
            TDH => hw.tdh,
            TDT => hw.tdt,
            _ => {
                if let Some(index) = Self::latched_index(offset) {
                    return self.hw.latched[index];
                }
                if let Some(entry) = Self::mta_index(offset) {
                    return self.hw.mta[entry];
                }
                if let Some((entry, half)) = Self::ra_index(offset) {
                    return self.hw.ra[entry][half];
                }
                self.counters.unmodelled_reads += 1;
                0
            }
        }
    }

    fn write_dword(&mut self, offset: u64, value: u32) {
        match offset {
            CTRL => self.write_ctrl(value),
            EECD => {
                self.hw.eecd = value & EECD_WRITABLE;
                self.hw.microwire.update(self.hw.eecd, &self.eeprom);
            }
            EERD => {
                if value & EERD_START != 0 {
                    let address = ((value >> 8) & 0xFF) as usize;
                    let word = self.eeprom.get(address).copied().unwrap_or(0);
                    self.hw.eerd = (u32::from(word) << 16) | (value & 0xFF00) | EERD_DONE;
                } else {
                    self.hw.eerd = value & 0xFF00;
                }
            }
            CTRL_EXT => self.hw.ctrl_ext = value,
            MDIC => self.write_mdic(value),
            ICS => self.hw.icr |= value & INT_MASK,
            IMS => self.hw.ims |= value & INT_MASK,
            IMC => self.hw.ims &= !(value & INT_MASK),
            RCTL => {
                self.hw.rctl = value & !1;
                self.hw.rx_fault = false;
            }
            RDBAL => {
                self.hw.rdba = (self.hw.rdba & !0xFFFF_FFFF) | u64::from(value & !0xF);
                self.hw.rx_fault = false;
            }
            RDBAH => {
                self.hw.rdba = (self.hw.rdba & 0xFFFF_FFFF) | (u64::from(value) << 32);
                self.hw.rx_fault = false;
            }
            RDLEN => {
                self.hw.rdlen = value & RING_LEN_MASK;
                self.hw.rx_fault = false;
            }
            RDH => {
                self.hw.rdh = value & 0xFFFF;
                self.hw.rx_fault = false;
            }
            RDT => self.hw.rdt = value & 0xFFFF,
            TCTL => {
                // Disabling the transmitter abandons a half-assembled packet.
                if self.hw.tctl & TCTL_EN != 0 && value & TCTL_EN == 0 {
                    self.hw.reset_tx_chain();
                }
                self.hw.tctl = value & !1;
                self.hw.tx_fault = false;
            }
            TDBAL => {
                self.hw.tdba = (self.hw.tdba & !0xFFFF_FFFF) | u64::from(value & !0xF);
                self.hw.reset_tx_chain();
            }
            TDBAH => {
                self.hw.tdba = (self.hw.tdba & 0xFFFF_FFFF) | (u64::from(value) << 32);
                self.hw.reset_tx_chain();
            }
            TDLEN => {
                self.hw.tdlen = value & RING_LEN_MASK;
                self.hw.reset_tx_chain();
            }
            TDH => {
                self.hw.tdh = value & 0xFFFF;
                self.hw.reset_tx_chain();
            }
            TDT => self.hw.tdt = value & 0xFFFF,
            // A 1 written to a cause bit clears it.
            ICR => self.hw.icr &= !value,
            // Read-only.
            STATUS => {}
            _ => {
                if let Some(index) = Self::latched_index(offset) {
                    self.hw.latched[index] = value;
                } else if let Some(entry) = Self::mta_index(offset) {
                    self.hw.mta[entry] = value;
                } else if let Some((entry, half)) = Self::ra_index(offset) {
                    self.hw.ra[entry][half] = value;
                } else {
                    self.counters.unmodelled_writes += 1;
                }
            }
        }
    }

    fn write_ctrl(&mut self, value: u32) {
        if value & CTRL_RST != 0 {
            self.device_reset();
            return;
        }
        let link_was_requested = self.hw.ctrl & CTRL_SLU != 0;
        if value & CTRL_PHY_RST != 0 {
            self.hw.phy = Phy::new();
        }
        // RST and PHY_RST read back as zero.
        self.hw.ctrl = value & !(CTRL_RST | CTRL_PHY_RST);
        // The link comes up when the driver sets SLU: a link status change.
        if !link_was_requested && value & CTRL_SLU != 0 {
            self.hw.icr |= INT_LSC;
        }
    }

    fn write_mdic(&mut self, value: u32) {
        let register = ((value >> MDIC_REG_SHIFT) & 0x1F) as usize;
        let phy_address = (value >> MDIC_PHY_SHIFT) & 0x1F;
        let operation = (value >> MDIC_OP_SHIFT) & 0b11;
        let mut result = value & (MDIC_INT_EN | (0b11 << MDIC_OP_SHIFT) | 0x03FF_0000);
        let mut error = phy_address != PHY_ADDRESS;
        match operation {
            MDIC_OP_READ if !error => {
                result |= u32::from(self.hw.phy.read(register));
            }
            MDIC_OP_WRITE if !error => {
                self.hw.phy.write(register, (value & MDIC_DATA_MASK) as u16);
            }
            _ => error = true,
        }
        if error {
            result |= MDIC_ERROR;
        }
        self.hw.mdic = result | MDIC_READY;
        if value & MDIC_INT_EN != 0 {
            self.hw.icr |= INT_MDAC;
        }
    }

    fn latched_index(offset: u64) -> Option<usize> {
        LATCHED.iter().position(|(register, _)| *register == offset)
    }

    fn mta_index(offset: u64) -> Option<usize> {
        let index = offset.checked_sub(MTA_BASE)? / 4;
        (index < MTA_ENTRIES as u64).then_some(index as usize)
    }

    /// The receive address pair and the half (0 = low dword) of a register offset.
    fn ra_index(offset: u64) -> Option<(usize, usize)> {
        let index = offset.checked_sub(RA_BASE)? / 4;
        (index < 2 * RA_ENTRIES as u64).then_some(((index / 2) as usize, (index % 2) as usize))
    }
}

/// Mask of the ring length registers: bits 19:7 (a multiple of 128 bytes).
const RING_LEN_MASK: u32 = 0x000F_FF80;

fn lock(state: &Mutex<NicState>) -> std::sync::MutexGuard<'_, NicState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The NIC as an MMIO `BusDevice` on BAR0.
pub struct NicDevice {
    state: Arc<Mutex<NicState>>,
}

/// Host-side handle aliasing the NIC state; drives servicing and reports diagnostics.
#[derive(Clone, Debug)]
pub struct NicHandle {
    state: Arc<Mutex<NicState>>,
    dirty: Arc<AtomicBool>,
}

// The state holds a trait object; Debug of the handle is the pointer identity only.
impl std::fmt::Debug for NicState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NicState")
            .field("mac", &self.mac)
            .field("counters", &self.counters)
            .finish_non_exhaustive()
    }
}

/// The ECAM/conf1 frontend of PCI function 00:06.0. Command, BAR0 and the INTx level share the
/// device's mutex with MMIO and DMA.
pub struct NicPciFunction(NicHandle);

impl PciFunction for NicPciFunction {
    fn config_read(&mut self, register: u16, width: AccessWidth) -> u32 {
        let mut state = lock(&self.0.state);
        let asserted = state.interrupt_level();
        state.pci.observe_interrupt(asserted);
        state.pci.config_read(register, width)
    }

    fn config_write(&mut self, register: u16, width: AccessWidth, value: u32) {
        // Command.IO (bit 0, the byte at 0x04) is not implemented: the function has no I/O BAR.
        let value = if register == 0x04 { value & !1 } else { value };
        lock(&self.0.state).pci.config_write(register, width, value);
    }
}

impl NicDevice {
    /// Creates the NIC (no backend attached yet) with its handle and the PCI function.
    pub fn new_pci(config: NicConfig) -> Result<(Self, NicHandle, NicPciFunction), NicConfigError> {
        config.validate()?;
        let dirty = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(NicState::new(config, Arc::clone(&dirty))));
        let handle = NicHandle {
            state: Arc::clone(&state),
            dirty,
        };
        Ok((Self { state }, handle.clone(), NicPciFunction(handle)))
    }

    /// A second device on the same state, for the firmware's exact-window MMIO path.
    pub(crate) fn mmio_endpoint(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
        }
    }

    pub(crate) fn read_mmio(&self, access: DeviceAccess) -> Result<u64, DeviceError> {
        if access.space != IoSpace::Mmio {
            return Err(DeviceError::UnsupportedSpace);
        }
        let mut state = lock(&self.state);
        if !state.memory_decode_enabled() {
            return Ok(u64::MAX >> (64 - access.width.bytes() * 8));
        }
        state.read_register(access.offset, access.width)
    }

    pub(crate) fn write_mmio(&self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        if access.space != IoSpace::Mmio {
            return Err(DeviceError::UnsupportedSpace);
        }
        let mut state = lock(&self.state);
        if !state.memory_decode_enabled() {
            return Ok(());
        }
        state.write_register(access.offset, access.width, value)?;
        // A guest write to a register (a doorbell) ends the quantum so that it is served at once.
        state.host_dirty.store(true, Ordering::Relaxed);
        Ok(())
    }
}

impl BusDevice for NicDevice {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        self.read_mmio(access)
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        self.write_mmio(access, value)
    }
}

impl NicHandle {
    /// Connects the backend (NETWORK.md section 4). One backend per NIC; the NIC owns it from now
    /// on and calls it only from `service` and reset paths, never from a guest register access.
    pub fn attach_backend(&self, backend: Box<dyn NetBackend>) -> Result<(), NicAttachError> {
        let mut state = lock(&self.state);
        if state.backend.is_some() {
            return Err(NicAttachError::AlreadyAttached);
        }
        state.backend = Some(backend);
        Ok(())
    }

    /// The guest wrote a register since the last `service`: the run loop ends its quantum.
    pub fn wakeup_pending(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    /// The flag of [`Self::wakeup_pending`] itself (the board's `stop` reads it).
    pub(crate) fn wakeup_flag(&self) -> &AtomicBool {
        &self.dirty
    }

    /// Board reset: registers, staged frame, backend flows and PCI configuration as at creation.
    pub fn platform_reset(&self) {
        lock(&self.state).platform_reset();
    }

    /// The INTx level without waiting for a worker that holds the device (`None` then).
    pub fn try_sample_pci_interrupts(&self) -> Option<bool> {
        match self.state.try_lock() {
            Ok(state) => Some(state.interrupt_level()),
            Err(std::sync::TryLockError::Poisoned(error)) => {
                Some(error.into_inner().interrupt_level())
            }
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
    }

    pub fn sample_pci_interrupts(&self) -> bool {
        lock(&self.state).interrupt_level()
    }

    /// Diagnostics of the model.
    pub fn counters(&self) -> NicCounters {
        lock(&self.state).counters
    }

    /// Counters of the attached backend; `None` without one.
    pub fn backend_counters(&self) -> Option<NetCounters> {
        lock(&self.state).backend.as_ref().map(|b| b.counters())
    }

    /// The MAC address the NIC was created with.
    pub fn mac(&self) -> [u8; 6] {
        lock(&self.state).mac
    }

    /// Walks the transmit ring and delivers received frames (NIC_DEVICE.md section 8). Guest
    /// memory is touched only through `bus` and only in RAM; `committed_write` receives every
    /// written range. Without Command.BME nothing is done.
    pub fn service(
        &self,
        bus: &mut MachineBus,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> NicServiceReport {
        let mut state = lock(&self.state);
        state.host_dirty.store(false, Ordering::Relaxed);
        if !state.dma_enabled() {
            return NicServiceReport::default();
        }
        let mut dma = BusDma {
            memory: bus.physical_memory(),
            committed: committed_write,
        };
        state.service(&mut dma)
    }
}

impl NicState {
    fn service(&mut self, dma: &mut dyn GuestDma) -> NicServiceReport {
        let tx_descriptors = self.service_tx(dma);
        let rx_frames = self.service_rx(dma);
        NicServiceReport {
            tx_descriptors,
            rx_frames,
        }
    }
}
