//! Register file, run/stop and reset state machine, doorbells and the servicing
//! loop of the xHCI controller model (S8-42, slice A). Contract:
//! `docs/specs/XHCI_DEVICE.md`.
//!
//! The controller is a `BusDevice` on the BAR0 window of PCI function 00:05.0. A
//! `BusDevice` cannot reach guest RAM, so doorbell and register writes only record
//! state; the machine owner calls [`XhciHandle::service`] between guest quanta,
//! which loads the event ring segment, publishes Port Status Change Events and
//! executes the command ring (`docs/specs/XHCI_DEVICE.md` section 3). Time is the
//! virtual clock handed in at construction: port reset (50 ms) and `MFINDEX`.

use super::commands::{self, CommandEnv};
use super::device::{InterruptStatus, NullDevice, UsbDevice};
use super::ports::Port;
use super::regs::{self, MAX_PORTS, MAX_SLOTS};
use super::ring::{EventRing, MAX_SEGMENT_TRBS, MIN_SEGMENT_TRBS};
use super::slots::{DCI_EP0, DCI_INTERRUPT_IN, Endpoint, EndpointState, Slot, SlotState};
use super::transfers::{
    self, Disposition, Execution, PlannedEvent, Position, Scan, ScanStop, TRANSFER_BUDGET, TdKind,
};
use super::trb::{
    TRB_BYTES, TRB_TYPE_LINK, TRB_TYPE_STOP_ENDPOINT, Trb, command_completion_event,
    event_ring_full_event, port_status_change_event, transfer_event,
};
use super::{XHCI_PCI_BAR_BASE, XHCI_PCI_BAR_BYTES, XHCI_USB_PORT_RESET_NS};
use crate::nvme::{BusDma, DmaError, GuestDma};
use crate::pci_ecam::{
    COMMAND_BUS_MASTER, COMMAND_INTX_DISABLE, COMMAND_MEMORY_ENABLE, PciFunction, Type0Config,
};
use crate::{
    AccessWidth, BusDevice, DeviceAccess, DeviceError, GuestPhysicalRange, IoSpace, MachineBus,
    VirtualClockHandle,
};
use std::sync::{Arc, Mutex};

/// At most this many command TRBs (Link TRBs included) are consumed per `service` call.
const COMMAND_BUDGET: u32 = 256;

/// Configuration of the model that is not part of the guest-visible contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XhciConfig {
    /// Duration of a port reset in virtual nanoseconds. The contract value is
    /// [`XHCI_USB_PORT_RESET_NS`]; tests may set 0.
    pub port_reset_ns: u64,
}

impl Default for XhciConfig {
    fn default() -> Self {
        Self {
            port_reset_ns: XHCI_USB_PORT_RESET_NS,
        }
    }
}

/// Why the controller raised Host System Error (USBSTS.HSE), kept for diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HseCause {
    /// The ERST entry is not in RAM, or its segment base or size is invalid.
    EventRingSegment,
    /// The event ring segment cannot be written (not RAM).
    EventRingDma,
    /// A command TRB or Link target is not in RAM.
    CommandRingDma,
    /// A transfer ring TRB is not in RAM.
    TransferRingDma,
    /// An Input Context cannot be read, or an Output Device Context (DCBAA entry 0, unaligned
    /// or not in RAM) cannot be read or written.
    ContextDma,
}

/// Work done by one [`XhciHandle::service`] call.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct XhciServiceReport {
    /// Command TRBs executed (Link TRBs are not counted).
    pub commands: u32,
    /// Events written to the event ring, including Event Ring Full Error.
    pub events: u32,
}

/// Everything HCRST resets: Operational, Runtime and port state, slots and rings.
#[derive(Debug)]
struct Hw {
    running: bool,
    hse: bool,
    /// A Host System Error happened since the last HCRST: R/S = 1 is refused even after the
    /// driver cleared USBSTS.HSE.
    fault_latched: bool,
    eint: bool,
    pcd: bool,
    inte: bool,
    hsee: bool,
    dnctrl: u32,
    /// Command ring dequeue pointer and its cycle state (CRCR.RCS); CRR is set by the
    /// first doorbell 0 and cleared by R/S = 0 and HCRST.
    cmd_ptr: u64,
    cmd_cycle: bool,
    crr: bool,
    cmd_doorbell: bool,
    dcbaap: u64,
    max_slots_en: u8,
    iman_ip: bool,
    iman_ie: bool,
    imod: u32,
    erstsz: u16,
    erstba: u64,
    /// ERDP pointer (63:4) and DESI (2:0) as written; EHB is separate.
    erdp: u64,
    ehb: bool,
    ring: Option<EventRing>,
    erst_load_pending: bool,
    slots: [Slot; MAX_SLOTS],
    ports: [Port; MAX_PORTS],
}

impl Hw {
    fn new() -> Self {
        Self {
            running: false,
            hse: false,
            fault_latched: false,
            eint: false,
            pcd: false,
            inte: false,
            hsee: false,
            dnctrl: 0,
            cmd_ptr: 0,
            cmd_cycle: false,
            crr: false,
            cmd_doorbell: false,
            dcbaap: 0,
            max_slots_en: 0,
            iman_ip: false,
            iman_ie: false,
            imod: regs::IMOD_RESET,
            erstsz: 0,
            erstba: 0,
            erdp: 0,
            ehb: false,
            ring: None,
            erst_load_pending: false,
            slots: [Slot::disabled(); MAX_SLOTS],
            ports: [Port::new(); MAX_PORTS],
        }
    }
}

#[derive(Debug)]
pub(crate) struct XhciState {
    config: XhciConfig,
    clock: VirtualClockHandle,
    pci: Type0Config,
    hw: Hw,
    ignored_doorbells: u64,
    ram_accesses: u64,
    /// Events written since creation (not reset by HCRST); `service` reports the difference.
    events_written: u64,
    hse_cause: Option<HseCause>,
    devices: Devices,
}

/// The devices behind the root ports (they outlive HCRST; HCRST resets them).
struct Devices([Box<dyn UsbDevice>; MAX_PORTS]);

impl std::fmt::Debug for Devices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Devices")
    }
}

/// Counts the guest RAM accesses made on behalf of the controller.
struct CountingDma<'a> {
    inner: &'a mut dyn GuestDma,
    accesses: u64,
}

impl GuestDma for CountingDma<'_> {
    fn read(&mut self, address: u64, buffer: &mut [u8]) -> Result<(), DmaError> {
        self.accesses += 1;
        self.inner.read(address, buffer)
    }

    fn write(&mut self, address: u64, data: &[u8]) -> Result<(), DmaError> {
        self.accesses += 1;
        self.inner.write(address, data)
    }
}

impl XhciState {
    fn new(config: XhciConfig, clock: VirtualClockHandle) -> Self {
        // Reserved project identity (as the NVMe endpoint); drivers bind by class 0x0C0330.
        let mut pci = Type0Config::new(
            0xD1A6,
            0x0010,
            0x01,
            0x0C_03_30,
            Some(XHCI_PCI_BAR_BYTES as u32),
        )
        .expect("xHCI has a valid constant class and BAR size");
        pci.configure_64_bit_memory_bar0(XHCI_PCI_BAR_BASE);
        pci.configure_inta();
        Self {
            config,
            clock,
            pci,
            hw: Hw::new(),
            ignored_doorbells: 0,
            ram_accesses: 0,
            events_written: 0,
            hse_cause: None,
            devices: Devices(std::array::from_fn(|_| {
                Box::new(NullDevice::default()) as Box<dyn UsbDevice>
            })),
        }
    }

    fn now_ns(&self) -> u64 {
        self.clock.snapshot().now_ns()
    }

    fn memory_decode_enabled(&self) -> bool {
        self.pci.command() & COMMAND_MEMORY_ENABLE != 0
            && self.pci.memory_bar0_address() == XHCI_PCI_BAR_BASE
    }

    fn dma_enabled(&self) -> bool {
        self.pci.command() & COMMAND_BUS_MASTER != 0
    }

    /// Completes port resets whose virtual time has come.
    fn poll_ports(&mut self) {
        let now = self.now_ns();
        for (port, device) in self.hw.ports.iter_mut().zip(self.devices.0.iter_mut()) {
            if let Some(newly_changed) = port.poll(now) {
                // The bus reset returns the device to the Default state.
                device.reset();
                self.hw.pcd |= newly_changed;
            }
        }
    }

    /// INTx level of Interrupter 0 (contract section 5): IP with IE and INTE, or a Host
    /// System Error with HSEE, unless Command.INTx Disable is set.
    fn interrupt_level(&self) -> bool {
        let hw = &self.hw;
        let line = (hw.iman_ip && hw.iman_ie && hw.inte) || (hw.hse && hw.hsee && hw.iman_ie);
        line && self.pci.command() & COMMAND_INTX_DISABLE == 0
    }

    /// Host System Error: the controller stops and waits for HCRST.
    fn fault(&mut self, cause: HseCause) {
        let hw = &mut self.hw;
        hw.hse = true;
        hw.fault_latched = true;
        hw.running = false;
        hw.crr = false;
        hw.cmd_doorbell = false;
        self.hse_cause = Some(cause);
    }

    /// An event was written: IP, EHB and EINT go up unless an earlier event is still
    /// unacknowledged or the event blocks its interrupt (BEI).
    fn note_event_written(&mut self, raise_interrupt: bool) {
        self.events_written += 1;
        let hw = &mut self.hw;
        if raise_interrupt && !hw.ehb {
            hw.iman_ip = true;
            hw.ehb = true;
            hw.eint = true;
        }
    }

    fn usbsts(&self) -> u32 {
        let hw = &self.hw;
        (if hw.running { 0 } else { regs::USBSTS_HCH })
            | if hw.hse { regs::USBSTS_HSE } else { 0 }
            | if hw.eint { regs::USBSTS_EINT } else { 0 }
            | if hw.pcd { regs::USBSTS_PCD } else { 0 }
    }

    /// A 4-byte aligned register or register half.
    fn read_dword(&self, offset: u64) -> u32 {
        let hw = &self.hw;
        match offset {
            0x00 => regs::CAPLENGTH | (regs::HCIVERSION << 16),
            0x04 => regs::HCSPARAMS1,
            0x08 | 0x0C | 0x1C => 0,
            0x10 => regs::HCCPARAMS1,
            0x14 => regs::DBOFF,
            0x18 => regs::RTSOFF,
            regs::USBCMD => {
                (if hw.running { regs::USBCMD_RS } else { 0 })
                    | if hw.inte { regs::USBCMD_INTE } else { 0 }
                    | if hw.hsee { regs::USBCMD_HSEE } else { 0 }
            }
            regs::USBSTS => self.usbsts(),
            regs::PAGESIZE => 1,
            regs::DNCTRL => hw.dnctrl,
            regs::CRCR => {
                if hw.crr {
                    regs::CRCR_CRR
                } else {
                    0
                }
            }
            regs::DCBAAP => hw.dcbaap as u32,
            0x54 => (hw.dcbaap >> 32) as u32,
            regs::CONFIG => u32::from(hw.max_slots_en),
            regs::PORT_BASE..=0x43F => {
                let port = ((offset - regs::PORT_BASE) / regs::PORT_STRIDE) as usize;
                if (offset - regs::PORT_BASE) % regs::PORT_STRIDE == 0 {
                    hw.ports[port].portsc()
                } else {
                    0
                }
            }
            regs::MFINDEX => regs::mfindex(self.now_ns()),
            regs::IMAN => {
                (if hw.iman_ip { regs::IMAN_IP } else { 0 })
                    | if hw.iman_ie { regs::IMAN_IE } else { 0 }
            }
            regs::IMOD => hw.imod,
            regs::ERSTSZ => u32::from(hw.erstsz),
            regs::ERSTBA => hw.erstba as u32,
            0x1034 => (hw.erstba >> 32) as u32,
            regs::ERDP => (hw.erdp as u32) | if hw.ehb { regs::ERDP_EHB } else { 0 },
            0x103C => (hw.erdp >> 32) as u32,
            regs::XECP..=0x3FFF => regs::extended_capability(offset),
            // Doorbells read 0; every other offset inside BAR0 is undefined and reads 0.
            _ => 0,
        }
    }

    fn check_access(offset: u64, width: AccessWidth, write: bool) -> Result<(), DeviceError> {
        let narrow_read = !write
            && matches!(width, AccessWidth::Byte | AccessWidth::Word)
            && offset < regs::CAPABILITY_END;
        match width {
            AccessWidth::Byte | AccessWidth::Word if !narrow_read => {
                return Err(DeviceError::UnsupportedWidth);
            }
            AccessWidth::Qword if !regs::is_qword_register(offset & !7) => {
                return Err(DeviceError::UnsupportedWidth);
            }
            _ => {}
        }
        if !offset.is_multiple_of(u64::from(width.bytes())) {
            return Err(DeviceError::Misaligned);
        }
        Ok(())
    }

    fn read_register(&self, offset: u64, width: AccessWidth) -> Result<u64, DeviceError> {
        Self::check_access(offset, width, false)?;
        Ok(match width {
            AccessWidth::Qword => {
                u64::from(self.read_dword(offset)) | (u64::from(self.read_dword(offset + 4)) << 32)
            }
            AccessWidth::Dword => u64::from(self.read_dword(offset)),
            narrow => {
                let shift = (offset % 4) * 8;
                let mask = (1u64 << (narrow.bytes() * 8)) - 1;
                (u64::from(self.read_dword(offset & !3)) >> shift) & mask
            }
        })
    }

    fn write_register(
        &mut self,
        offset: u64,
        width: AccessWidth,
        value: u64,
    ) -> Result<(), DeviceError> {
        Self::check_access(offset, width, true)?;
        if width == AccessWidth::Qword {
            self.write_dword(offset, value as u32, false);
            self.write_dword(offset + 4, (value >> 32) as u32, false);
            self.apply_erdp(offset, value as u32 & regs::ERDP_EHB != 0);
        } else {
            self.write_dword(offset, value as u32, true);
        }
        Ok(())
    }

    /// A 4-byte aligned register write. `apply_erdp_now` is false for the halves of a
    /// qword write, which moves the event ring dequeue pointer once afterwards.
    fn write_dword(&mut self, offset: u64, value: u32, apply_erdp_now: bool) {
        let now = self.now_ns();
        let stopped = !self.hw.running;
        match offset {
            regs::USBCMD => self.write_usbcmd(value),
            regs::USBSTS => {
                let hw = &mut self.hw;
                if value & regs::USBSTS_HSE != 0 {
                    hw.hse = false;
                }
                if value & regs::USBSTS_EINT != 0 {
                    hw.eint = false;
                }
                if value & regs::USBSTS_PCD != 0 {
                    hw.pcd = false;
                }
            }
            regs::DNCTRL => self.hw.dnctrl = value & 0xFFFF,
            regs::CRCR if !self.hw.crr => {
                let hw = &mut self.hw;
                hw.cmd_cycle = value & regs::CRCR_RCS != 0;
                hw.cmd_ptr = (hw.cmd_ptr & !0xFFFF_FFFF) | u64::from(value & 0xFFFF_FFC0);
            }
            0x3C if !self.hw.crr => {
                let hw = &mut self.hw;
                hw.cmd_ptr = (hw.cmd_ptr & 0xFFFF_FFFF) | (u64::from(value) << 32);
            }
            regs::DCBAAP if stopped => {
                let hw = &mut self.hw;
                hw.dcbaap = (hw.dcbaap & !0xFFFF_FFFF) | u64::from(value & 0xFFFF_FFC0);
            }
            0x54 if stopped => {
                let hw = &mut self.hw;
                hw.dcbaap = (hw.dcbaap & 0xFFFF_FFFF) | (u64::from(value) << 32);
            }
            regs::CONFIG if stopped => self.hw.max_slots_en = value as u8,
            regs::PORT_BASE..=0x43F => {
                if (offset - regs::PORT_BASE) % regs::PORT_STRIDE == 0 {
                    let port = ((offset - regs::PORT_BASE) / regs::PORT_STRIDE) as usize;
                    let reset_ns = self.config.port_reset_ns;
                    if let Some(newly_changed) = self.hw.ports[port].write(value, now, reset_ns) {
                        self.devices.0[port].reset();
                        self.hw.pcd |= newly_changed;
                    }
                }
            }
            regs::DOORBELLS..=0x823 => {
                self.write_doorbell(((offset - regs::DOORBELLS) / 4) as usize, value)
            }
            regs::IMAN => {
                let hw = &mut self.hw;
                if value & regs::IMAN_IP != 0 {
                    hw.iman_ip = false;
                }
                hw.iman_ie = value & regs::IMAN_IE != 0;
            }
            regs::IMOD => self.hw.imod = value,
            regs::ERSTSZ if stopped && value & 0xFFFF <= 1 => {
                self.hw.erstsz = (value & 0xFFFF) as u16
            }
            regs::ERSTBA if stopped => {
                let hw = &mut self.hw;
                hw.erstba = (hw.erstba & !0xFFFF_FFFF) | u64::from(value & 0xFFFF_FFC0);
            }
            0x1034 if stopped => {
                let hw = &mut self.hw;
                hw.erstba = (hw.erstba & 0xFFFF_FFFF) | (u64::from(value) << 32);
                hw.erst_load_pending = true;
            }
            regs::ERDP => {
                let hw = &mut self.hw;
                hw.erdp = (hw.erdp & !0xFFFF_FFFF) | u64::from(value & 0xFFFF_FFF7);
                if value & regs::ERDP_EHB != 0 {
                    hw.ehb = false;
                }
                if apply_erdp_now {
                    self.apply_erdp(offset, value & regs::ERDP_EHB != 0);
                }
            }
            0x103C => {
                let hw = &mut self.hw;
                hw.erdp = (hw.erdp & 0xFFFF_FFFF) | (u64::from(value) << 32);
                if apply_erdp_now {
                    self.apply_erdp(offset, false);
                }
            }
            // Read-only registers, reserved bits and undefined offsets ignore writes; so do
            // writes the Run/Stop state refuses (CRCR while CRR = 1, DCBAAP, CONFIG, ERSTSZ and
            // ERSTBA while R/S = 1).
            _ => {}
        }
    }

    /// Moves the event ring dequeue pointer to the ERDP the driver wrote; `acknowledged` is
    /// EHB = 1 in the write.
    fn apply_erdp(&mut self, offset: u64, acknowledged: bool) {
        if offset & !7 != regs::ERDP {
            return;
        }
        let pointer = self.hw.erdp & !0xF;
        let Some(ring) = self.hw.ring.as_mut() else {
            return;
        };
        ring.set_dequeue_pointer(pointer, acknowledged);
        // Events written while EHB = 1 raised no interrupt: when the driver clears EHB and
        // events are still in the ring, IP, EHB and EINT go up again (xHCI 4.17.2).
        if acknowledged && ring.unconsumed() > 0 {
            let hw = &mut self.hw;
            hw.iman_ip = true;
            hw.ehb = true;
            hw.eint = true;
        }
    }

    fn write_usbcmd(&mut self, value: u32) {
        if value & regs::USBCMD_HCRST != 0 {
            // HCRST resets the whole controller state; the rest of the write is moot. The PCI
            // configuration, the configuration of the model and the diagnostics stay.
            self.hw = Hw::new();
            self.devices.0.iter_mut().for_each(|device| device.reset());
            return;
        }
        let hw = &mut self.hw;
        hw.inte = value & regs::USBCMD_INTE != 0;
        hw.hsee = value & regs::USBCMD_HSEE != 0;
        let run = value & regs::USBCMD_RS != 0;
        if run && !hw.running {
            // R/S = 1 is ignored after a Host System Error until HCRST.
            hw.running = !hw.hse && !hw.fault_latched;
        } else if !run && hw.running {
            hw.running = false;
            hw.crr = false;
            hw.cmd_doorbell = false;
        }
    }

    /// Doorbell `index` (0 is the command ring, 1..=8 are slots). A doorbell the controller
    /// cannot act on is ignored and counted.
    fn write_doorbell(&mut self, index: usize, value: u32) {
        let target = value & 0xFF;
        let hw = &mut self.hw;
        if !hw.running {
            self.ignored_doorbells += 1;
        } else if index == 0 {
            if target == 0 {
                hw.crr = true;
                hw.cmd_doorbell = true;
            } else {
                self.ignored_doorbells += 1;
            }
        } else if !Self::slot_doorbell(&mut hw.slots[index - 1], value) {
            self.ignored_doorbells += 1;
        }
    }

    /// A doorbell of a slot (contract 9.1 and 9.3): the slot must be Default, Addressed or
    /// Configured, the DCI 1 or 3 (DCI 3 only in a Configured slot), the stream id 0. A Stopped endpoint
    /// becomes Running; Disabled, Halted and Error endpoints ignore the doorbell. Returns
    /// `false` when the doorbell is ignored. An accepted doorbell leaves transfer work pending
    /// for `service`.
    fn slot_doorbell(slot: &mut Slot, value: u32) -> bool {
        let dci = value & 0xFF;
        let stream_id = value >> 16;
        let slot_state = slot.state;
        let usable = match dci {
            // EP0 of a Default slot carries the descriptor requests sent to address 0.
            1 => slot_state.is_addressable(),
            3 => slot_state == SlotState::Configured,
            _ => false,
        };
        if !usable || stream_id != 0 {
            return false;
        }
        match slot.endpoint_mut(dci) {
            Some(endpoint) if endpoint.state == EndpointState::Stopped => {
                endpoint.state = EndpointState::Running;
                // The Output Endpoint Context says Stopped until `service` writes Running.
                slot.unmirrored |= 1 << dci;
                slot.pending |= 1 << dci;
                true
            }
            Some(endpoint) if endpoint.state == EndpointState::Running => {
                slot.pending |= 1 << dci;
                true
            }
            _ => false,
        }
    }

    /// Loads the single ERST entry written to ERSTBA and initializes the event ring.
    /// Writes EP State and the dequeue pointer of endpoints that a doorbell or a transfer changed
    /// into their Output Endpoint Contexts.
    fn mirror_endpoint_states(&mut self, dma: &mut dyn GuestDma) {
        for index in 0..MAX_SLOTS {
            let slot = self.hw.slots[index];
            if slot.unmirrored == 0 {
                continue;
            }
            self.hw.slots[index].unmirrored = 0;
            let result = commands::output_context_of(self.hw.dcbaap, dma, index as u32 + 1)
                .and_then(|output| {
                    (0..32u32)
                        .filter(|dci| slot.unmirrored & (1 << dci) != 0)
                        .try_for_each(|dci| {
                            let endpoint = slot
                                .endpoint(dci)
                                .copied()
                                .unwrap_or_else(Endpoint::disabled);
                            commands::mirror_endpoint(dma, output, dci, &endpoint)
                        })
                });
            if result.is_err() {
                self.fault(HseCause::ContextDma);
                return;
            }
        }
    }

    fn load_event_ring(&mut self, dma: &mut dyn GuestDma) {
        if !self.hw.erst_load_pending || self.hw.erstsz != 1 {
            return;
        }
        self.hw.erst_load_pending = false;
        let mut entry = [0u8; 16];
        if dma.read(self.hw.erstba, &mut entry).is_err() {
            self.fault(HseCause::EventRingSegment);
            return;
        }
        let base = u64::from_le_bytes(entry[0..8].try_into().expect("8 bytes"));
        let size = u32::from(u16::from_le_bytes([entry[8], entry[9]]));
        let size_valid = (MIN_SEGMENT_TRBS..=MAX_SEGMENT_TRBS).contains(&size);
        let mut probe = [0u8; 16];
        let in_ram = size_valid
            && base.is_multiple_of(64)
            && dma.read(base, &mut probe).is_ok()
            && dma
                .read(base + u64::from(size - 1) * TRB_BYTES, &mut probe)
                .is_ok();
        if !in_ram {
            self.fault(HseCause::EventRingSegment);
            return;
        }
        self.hw.ring = Some(EventRing::new(base, size));
    }

    /// Makes room for `count` more events and the TRB kept for the Event Ring Full Error. When
    /// there is not enough room the ring gets the Event Ring Full Error once and processing is
    /// blocked until the driver moves ERDP.
    fn reserve_events(&mut self, dma: &mut dyn GuestDma, count: u32) -> bool {
        let Some(ring) = self.hw.ring.as_mut() else {
            return false;
        };
        if ring.full_reported() || ring.free() == 0 {
            return false;
        }
        // A TD with more events than the ring can hold waits for an empty ring and loses the
        // rest (a driver gives the event ring room for its rings).
        let count = count.min(ring.size() - 1);
        if ring.free() > count {
            return true;
        }
        // The last TRBs are reserved for the Event Ring Full Error.
        match ring.write(dma, event_ring_full_event()) {
            Ok(()) => {
                ring.mark_full_reported();
                self.note_event_written(true);
            }
            Err(DmaError) => self.fault(HseCause::EventRingDma),
        }
        false
    }

    /// Writes an event into room made by [`Self::reserve_events`].
    fn post_event(&mut self, dma: &mut dyn GuestDma, event: Trb) {
        self.post_event_with(dma, event, true);
    }

    /// Like [`Self::post_event`]; `raise_interrupt` is false for a TRB with BEI = 1.
    fn post_event_with(&mut self, dma: &mut dyn GuestDma, event: Trb, raise_interrupt: bool) {
        let Some(ring) = self.hw.ring.as_mut() else {
            return;
        };
        if ring.free() == 0 {
            return;
        }
        match ring.write(dma, event) {
            Ok(()) => self.note_event_written(raise_interrupt),
            Err(DmaError) => self.fault(HseCause::EventRingDma),
        }
    }

    fn publish_port_events(&mut self, dma: &mut dyn GuestDma) {
        for index in 0..MAX_PORTS {
            if !self.hw.ports[index].event_due() {
                continue;
            }
            if !self.reserve_events(dma, 1) {
                return;
            }
            self.post_event(dma, port_status_change_event(index as u32 + 1));
            if self.hw.hse {
                return;
            }
            self.hw.ports[index].mark_event_reported();
        }
    }

    /// Executes the command ring from the dequeue pointer until the first TRB whose cycle bit
    /// differs from the ring cycle state (empty ring), the event ring has no room, or the
    /// budget of one call is spent.
    fn run_commands(&mut self, dma: &mut dyn GuestDma, report: &mut XhciServiceReport) {
        if !self.hw.cmd_doorbell {
            return;
        }
        for _ in 0..COMMAND_BUDGET {
            let address = self.hw.cmd_ptr;
            let mut raw = [0u8; 16];
            if dma.read(address, &mut raw).is_err() {
                self.fault(HseCause::CommandRingDma);
                return;
            }
            let trb = Trb::from_bytes(&raw);
            if trb.cycle() != self.hw.cmd_cycle {
                self.hw.cmd_doorbell = false;
                return;
            }
            if trb.trb_type() == TRB_TYPE_LINK {
                // A Link TRB moves the ring without an event.
                // The target is the start of a command ring segment: 64-byte aligned.
                let target = trb.parameter() & !0xF;
                if !target.is_multiple_of(64) {
                    self.fault(HseCause::CommandRingDma);
                    return;
                }
                let hw = &mut self.hw;
                hw.cmd_cycle ^= trb.toggle_cycle();
                hw.cmd_ptr = target;
                continue;
            }
            // Stop Endpoint writes a Transfer Event before the Command Completion Event.
            let events = if trb.trb_type() == TRB_TYPE_STOP_ENDPOINT {
                2
            } else {
                1
            };
            if !self.reserve_events(dma, events) {
                return;
            }
            let hw = &mut self.hw;
            let mut env = CommandEnv {
                slots: &mut hw.slots,
                ports: &mut hw.ports,
                devices: &mut self.devices.0,
                max_slots_enabled: hw.max_slots_en,
                dcbaap: hw.dcbaap,
                dma: &mut *dma,
            };
            let Ok(result) = commands::execute(&mut env, &trb) else {
                self.fault(HseCause::ContextDma);
                return;
            };
            self.hw.cmd_ptr = address.wrapping_add(TRB_BYTES);
            if let Some(event) = result.transfer_event {
                self.post_event(dma, event);
            }
            self.post_event(
                dma,
                command_completion_event(address, result.code, result.slot_id),
            );
            if self.hw.hse {
                return;
            }
            report.commands += 1;
        }
    }

    fn work_pending(&self) -> bool {
        let hw = &self.hw;
        if hw.hse {
            return false;
        }
        if hw.erst_load_pending && hw.erstsz == 1 {
            return true;
        }
        let room = hw.ring.as_ref().is_some_and(EventRing::has_room);
        hw.running
            && (hw.slots.iter().any(|slot| slot.unmirrored != 0)
                || room
                    && (hw.cmd_doorbell
                        || hw.ports.iter().any(Port::event_due)
                        || hw.slots.iter().any(|slot| slot.pending != 0)))
    }

    fn service(&mut self, dma: &mut dyn GuestDma) -> XhciServiceReport {
        let mut report = XhciServiceReport::default();
        let events_before = self.events_written;
        self.service_rings(dma, &mut report);
        report.events = (self.events_written - events_before) as u32;
        report
    }

    fn service_rings(&mut self, dma: &mut dyn GuestDma, report: &mut XhciServiceReport) {
        if self.hw.hse {
            return;
        }
        self.load_event_ring(dma);
        if self.hw.hse || !self.hw.running {
            return;
        }
        self.mirror_endpoint_states(dma);
        if self.hw.hse {
            return;
        }
        self.publish_port_events(dma);
        if !self.hw.hse {
            self.run_commands(dma, report);
        }
        if !self.hw.hse {
            self.run_transfers(dma);
        }
        if !self.hw.hse {
            self.mirror_endpoint_states(dma);
        }
    }

    /// Executes the transfer rings of every endpoint with pending work.
    fn run_transfers(&mut self, dma: &mut dyn GuestDma) {
        for index in 0..MAX_SLOTS {
            for dci in [DCI_EP0, DCI_INTERRUPT_IN] {
                if self.hw.slots[index].pending & (1 << dci) != 0 {
                    self.run_endpoint(dma, index, dci);
                    if self.hw.hse {
                        return;
                    }
                }
            }
        }
    }

    /// Executes TDs of one endpoint until its ring is empty or incomplete, the device NAKs, the
    /// endpoint stops running, the event ring has no room, or the budget is spent.
    fn run_endpoint(&mut self, dma: &mut dyn GuestDma, index: usize, dci: u32) {
        let bit = 1 << dci;
        let slot_id = index as u32 + 1;
        let mut budget = TRANSFER_BUDGET;
        loop {
            let slot = self.hw.slots[index];
            let endpoint = slot
                .endpoint(dci)
                .copied()
                .unwrap_or_else(Endpoint::disabled);
            let slot_usable = if dci == DCI_EP0 {
                slot.state.is_addressable()
            } else {
                slot.state == SlotState::Configured
            };
            if !slot_usable || endpoint.state != EndpointState::Running {
                self.hw.slots[index].pending &= !bit;
                return;
            }
            let first_td = budget == TRANSFER_BUDGET;
            let scan = if dci == DCI_EP0 {
                transfers::scan_control(dma, &endpoint, &mut budget)
            } else {
                transfers::scan_interrupt(dma, &endpoint, &mut budget)
            };
            let execution = match scan {
                Err(ScanStop::Dma) => {
                    self.fault(HseCause::TransferRingDma);
                    return;
                }
                // The rest of the ring waits for the next `service` call, unless a whole call
                // could not get through one TD: such a ring (Link TRBs in a loop) is a TRB Error.
                Err(ScanStop::Budget) if !first_td => return,
                Err(ScanStop::Budget) => {
                    if !self.reserve_events(dma, 1) {
                        return;
                    }
                    Execution::trb_error_at(Position {
                        pointer: endpoint.dequeue,
                        dcs: endpoint.dcs,
                    })
                }
                Ok(Scan::Empty | Scan::Incomplete) => {
                    self.hw.slots[index].pending &= !bit;
                    return;
                }
                Ok(Scan::Invalid(located)) => {
                    if !self.reserve_events(dma, 1) {
                        return;
                    }
                    Execution::trb_error(&located)
                }
                Ok(Scan::Td(td)) => {
                    let port = usize::from(slot.port) - 1;
                    // The device must be where the slot's context says: address 0 in a Default
                    // slot, the Slot ID once addressed (a port reset puts it back to 0).
                    let expected = if slot.state == SlotState::Default {
                        0
                    } else {
                        slot_id as u8
                    };
                    let usable = self.hw.ports[port].is_enabled()
                        && self.devices.0[port].address() == expected;
                    // An Interrupt IN poll that gets a NAK writes no event and needs no room.
                    if td.kind() == TdKind::InterruptIn
                        && usable
                        && self.devices.0[port].interrupt_in() == InterruptStatus::Nak
                    {
                        return;
                    }
                    if !self.reserve_events(dma, td.event_bound()) {
                        return;
                    }
                    let device = &mut *self.devices.0[port];
                    match td.kind() {
                        TdKind::Control => transfers::execute_control(dma, &td, device, usable),
                        TdKind::InterruptIn => {
                            transfers::execute_interrupt(dma, &td, device, usable)
                        }
                    }
                }
            };
            if !self.finish_transfer(dma, index, dci, &execution) {
                return;
            }
        }
    }

    /// Posts the events of an executed TD and moves the endpoint. Returns `true` when the next TD
    /// may run.
    fn finish_transfer(
        &mut self,
        dma: &mut dyn GuestDma,
        index: usize,
        dci: u32,
        execution: &Execution,
    ) -> bool {
        let slot_id = index as u32 + 1;
        for PlannedEvent {
            address,
            code,
            remaining,
            block_interrupt,
        } in &execution.events
        {
            let event = transfer_event(*address, *code, *remaining, slot_id, dci);
            self.post_event_with(dma, event, !block_interrupt);
            if self.hw.hse {
                return false;
            }
        }
        let slot = &mut self.hw.slots[index];
        let Some(endpoint) = slot.endpoint_mut(dci) else {
            return false;
        };
        match execution.disposition {
            Disposition::Retry => return false,
            Disposition::Done(position) => {
                endpoint.dequeue = position.pointer;
                endpoint.dcs = position.dcs;
            }
            Disposition::Halt(position) | Disposition::Fail(position) => {
                endpoint.state = if matches!(execution.disposition, Disposition::Fail(_)) {
                    EndpointState::Error
                } else {
                    EndpointState::Halted
                };
                endpoint.dequeue = position.pointer;
                endpoint.dcs = position.dcs;
            }
        }
        let still_running = endpoint.state == EndpointState::Running;
        slot.unmirrored |= 1 << dci;
        if !still_running {
            slot.pending &= !(1 << dci);
        }
        still_running
    }
}

#[cfg(test)]
impl XhciHandle {
    /// Test hook: the endpoint state of a slot (slot ids and DCI 1 or 3).
    pub(super) fn endpoint_state(&self, slot_id: usize, dci: u32) -> EndpointState {
        lock(&self.state).hw.slots[slot_id - 1].endpoint_state(dci)
    }

    /// Test hook: puts an endpoint into a state transfers will reach in the transfer slice
    /// (Halted, Error).
    pub(super) fn force_endpoint_state(&self, slot_id: usize, dci: u32, state: EndpointState) {
        let mut guard = lock(&self.state);
        let endpoint = guard.hw.slots[slot_id - 1]
            .endpoint_mut(dci)
            .expect("endpoint exists");
        endpoint.state = state;
    }

    /// Test hook: the USB address the device behind a root port was given.
    pub(super) fn device_address(&self, port: usize) -> u8 {
        lock(&self.state).devices.0[port - 1].address()
    }
}

fn lock(state: &Mutex<XhciState>) -> std::sync::MutexGuard<'_, XhciState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The xHCI controller as an MMIO `BusDevice` on BAR0.
pub struct XhciDevice {
    state: Arc<Mutex<XhciState>>,
}

/// Host-side handle aliasing the controller state; drives servicing and reports diagnostics.
#[derive(Clone, Debug)]
pub struct XhciHandle {
    state: Arc<Mutex<XhciState>>,
}

/// The ECAM/conf1 frontend of PCI function 00:05.0. Command, BAR0 and the INTx level share
/// the controller's mutex with MMIO and DMA.
pub struct XhciPciFunction(XhciHandle);

impl PciFunction for XhciPciFunction {
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

impl XhciDevice {
    /// Creates the controller (stopped, ports attached and not reset) with its handle and
    /// the PCI function. `clock` is the board's virtual clock.
    pub fn new_pci(
        config: XhciConfig,
        clock: VirtualClockHandle,
    ) -> (Self, XhciHandle, XhciPciFunction) {
        let state = Arc::new(Mutex::new(XhciState::new(config, clock)));
        let handle = XhciHandle {
            state: Arc::clone(&state),
        };
        (Self { state }, handle.clone(), XhciPciFunction(handle))
    }
}

/// A port number outside `1..=2`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidPort(pub usize);

impl XhciHandle {
    /// Attaches the model of a device to root port `port` (1 or 2), replacing the previous one.
    /// The device is reset: board setup time, not for a running guest.
    pub fn attach_device(
        &self,
        port: usize,
        mut device: Box<dyn UsbDevice>,
    ) -> Result<(), InvalidPort> {
        if !(1..=MAX_PORTS).contains(&port) {
            return Err(InvalidPort(port));
        }
        device.reset();
        lock(&self.state).devices.0[port - 1] = device;
        Ok(())
    }

    /// The INTx level of the function (before the IO-APIC): Interrupter 0 or Host System Error.
    pub fn sample_pci_interrupts(&self) -> bool {
        let mut state = lock(&self.state);
        state.poll_ports();
        state.interrupt_level()
    }

    /// `service` has work it can do: an ERST to load, or (running, with room in the event
    /// ring) a command doorbell or a Port Status Change Event owed. The machine owner services
    /// the controller between guest quanta while this is true. A blocked ring (full event ring)
    /// is not work until the driver moves ERDP.
    pub fn service_pending(&self) -> bool {
        let mut state = lock(&self.state);
        state.poll_ports();
        state.dma_enabled() && state.work_pending()
    }

    /// Doorbells the controller ignored (halted, slot not enabled, DCI not 1 or 3, stream id,
    /// or a nonzero target of doorbell 0).
    pub fn ignored_doorbells(&self) -> u64 {
        lock(&self.state).ignored_doorbells
    }

    /// Guest RAM accesses (each read or write) made by the controller since creation.
    pub fn ram_accesses(&self) -> u64 {
        lock(&self.state).ram_accesses
    }

    /// The cause of the last Host System Error, if any since creation.
    pub fn hse_cause(&self) -> Option<HseCause> {
        lock(&self.state).hse_cause
    }

    /// Loads the event ring, publishes pending Port Status Change Events and executes the
    /// command ring. Guest memory is touched only through `bus` and only in RAM;
    /// `committed_write` receives every written range. Without Command.BME nothing is
    /// done.
    pub fn service(
        &self,
        bus: &mut MachineBus,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> XhciServiceReport {
        let mut state = lock(&self.state);
        state.poll_ports();
        if !state.dma_enabled() {
            return XhciServiceReport::default();
        }
        let mut bus_dma = BusDma {
            memory: bus.physical_memory(),
            committed: committed_write,
        };
        let mut dma = CountingDma {
            inner: &mut bus_dma,
            accesses: 0,
        };
        let report = state.service(&mut dma);
        let accesses = dma.accesses;
        state.ram_accesses += accesses;
        report
    }
}

impl XhciDevice {
    fn read_mmio(&self, access: DeviceAccess) -> Result<u64, DeviceError> {
        if access.space != IoSpace::Mmio {
            return Err(DeviceError::UnsupportedSpace);
        }
        let mut state = lock(&self.state);
        if !state.memory_decode_enabled() {
            return Ok(u64::MAX >> (64 - access.width.bytes() * 8));
        }
        state.poll_ports();
        state.read_register(access.offset, access.width)
    }

    fn write_mmio(&self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        if access.space != IoSpace::Mmio {
            return Err(DeviceError::UnsupportedSpace);
        }
        let mut state = lock(&self.state);
        if !state.memory_decode_enabled() {
            return Ok(());
        }
        state.poll_ports();
        state.write_register(access.offset, access.width, value)
    }
}

impl BusDevice for XhciDevice {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        self.read_mmio(access)
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        self.write_mmio(access, value)
    }
}
