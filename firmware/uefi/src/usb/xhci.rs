//! xHCI host controller driver for the pre-boot keyboard (S7-34 slice A, XHCI_DEVICE.md
//! section 15). One controller, polling only: no interrupts, no MSI.
//!
//! Hardware-facing code (MMIO and DMA through identity-mapped addresses); the logic it uses --
//! TRBs, rings, descriptors, contexts, report decoding -- lives in the sibling modules and is
//! covered by host tests. This file is exercised by the freestanding run against the board's
//! xHCI model (`tests/system/firmware_xhci_smoke.py`).

use core::{
    ptr,
    sync::atomic::{Ordering, compiler_fence},
};

use crate::{
    bs::{event::stall, memory::allocate_pages},
    mem::map::MemoryType,
    paging::PAGE_BYTES,
    uefi::status::Status,
};

use super::{
    Step, UsbError,
    desc::{
        self, DEVICE_DESCRIPTOR_BYTES, KeyboardInterface, MAX_CONFIGURATION_BYTES, SetupPacket,
    },
    hid_keyboard::{BOOT_REPORT_BYTES, BootKeyboard},
    trb::{
        DCI_EP0, DCI_INTERRUPT_IN, DEVICE_CONTEXT_BYTES, EP0_MAX_PACKET, EventCursor,
        INPUT_CONTEXT_BYTES, InputContext, RingCursor, TRB_BYTES, TransferType, Trb, completion,
        erdp_value, trb_type,
    },
};

const PAGE: u64 = PAGE_BYTES;

// DMA layout: one allocation of nine pages, one structure per page (no structure crosses a
// 64 KiB boundary, XHCI_DEVICE.md section 8).
const PAGE_DCBAA: u64 = 0;
const PAGE_COMMAND_RING: u64 = 1;
const PAGE_EVENT_RING: u64 = 2;
const PAGE_ERST: u64 = 3;
const PAGE_INPUT_CONTEXT: u64 = 4;
const PAGE_OUTPUT_CONTEXT: u64 = 5;
const PAGE_EP0_RING: u64 = 6;
const PAGE_INTERRUPT_RING: u64 = 7;
const PAGE_DATA: u64 = 8;
pub(super) const DMA_PAGES: usize = 9;

/// TRBs per ring segment, Link TRB included (the event ring has no Link TRB).
const RING_TRBS: usize = 64;
const EVENT_TRBS: usize = 64;
/// Data page layout: descriptor buffer, Interrupt IN report, LED Output report.
const DATA_DESCRIPTOR: u64 = 0;
const DATA_REPORT: u64 = 0x100;
const DATA_LED: u64 = 0x140;

/// Slots the driver enables (CONFIG.MaxSlotsEn): one keyboard plus one being probed.
const SLOTS: u32 = 2;

// Capability registers (offsets from BAR0).
const CAP_HCIVERSION_LENGTH: usize = 0x00;
const CAP_HCSPARAMS1: usize = 0x04;
const CAP_HCCPARAMS1: usize = 0x10;
const CAP_DBOFF: usize = 0x14;
const CAP_RTSOFF: usize = 0x18;
const BAR_BYTES: usize = 0x4000;

// Operational registers (offsets from the operational base).
const OP_USBCMD: usize = 0x00;
const OP_USBSTS: usize = 0x04;
const OP_CRCR: usize = 0x18;
const OP_DCBAAP: usize = 0x30;
const OP_CONFIG: usize = 0x38;
const OP_PORTSC: usize = 0x400;
const USBCMD_RUN: u32 = 1;
const USBCMD_HCRST: u32 = 1 << 1;
const USBSTS_HCH: u32 = 1;
const USBSTS_CNR: u32 = 1 << 11;
const CRCR_RCS: u64 = 1;

// Interrupter 0 (offsets from the runtime base).
const RT_ERSTSZ: usize = 0x28;
const RT_ERSTBA: usize = 0x30;
const RT_ERDP: usize = 0x38;

// PORTSC bits (xHCI 1.2 section 5.4.8, XHCI_DEVICE.md section 6).
const PORTSC_CCS: u32 = 1;
const PORTSC_PED: u32 = 1 << 1;
const PORTSC_PR: u32 = 1 << 4;
const PORTSC_CSC: u32 = 1 << 17;
const PORTSC_PRC: u32 = 1 << 21;
/// Read-write bits written back as read (PP, PIC, wake enables); the RW1C change bits and PED
/// (write 1 disables the port) are written as 0 unless the caller sets them.
const PORTSC_PRESERVE: u32 = (1 << 9) | (3 << 14) | (7 << 25);

const POLL_US: usize = 10;
/// Waits are bounded in time through the Boot Services stall clock (the board's virtual clock):
/// register handshakes, one command or transfer, one port reset. The port reset takes 50 ms of
/// virtual time on the board (XHCI_DEVICE.md section 6), so its bound is four times that.
const REGISTER_WAIT_US: usize = 20_000;
const TRANSFER_WAIT_US: usize = 100_000;
const PORT_RESET_WAIT_US: usize = 200_000;

#[derive(Clone, Copy)]
struct Mmio(usize);

impl Mmio {
    fn read(self, offset: usize) -> u32 {
        // SAFETY: the BAR0 window was validated by PCI discovery; accesses are aligned dwords.
        let value = unsafe { ptr::with_exposed_provenance::<u32>(self.0 + offset).read_volatile() };
        // The firmware CPU profile executes no read-modify-write instruction on memory
        // (`and $imm, (mem)`): keep LLVM from folding a register read and its write-back into one.
        core::hint::black_box(value)
    }

    fn write(self, offset: usize, value: u32) {
        // SAFETY: as in `read`.
        unsafe { ptr::with_exposed_provenance_mut::<u32>(self.0 + offset).write_volatile(value) };
    }

    /// 64-bit registers are written as two dwords, low first.
    fn write64(self, offset: usize, value: u64) {
        self.write(offset, value as u32);
        self.write(offset + 4, (value >> 32) as u32);
    }
}

// Guest RAM accessors. The DMA pages belong to the driver; the controller reads and writes them
// asynchronously, so every access is volatile.

fn ram_read32(address: u64) -> u32 {
    // SAFETY: callers pass addresses inside the driver's DMA allocation.
    let value = unsafe { ptr::with_exposed_provenance::<u32>(address as usize).read_volatile() };
    // No read-modify-write folding (see `Mmio::read`).
    core::hint::black_box(value)
}

fn ram_write32(address: u64, value: u32) {
    // SAFETY: as in `ram_read32`.
    unsafe { ptr::with_exposed_provenance_mut::<u32>(address as usize).write_volatile(value) };
}

fn ram_write64(address: u64, value: u64) {
    ram_write32(address, value as u32);
    ram_write32(address + 4, (value >> 32) as u32);
}

fn ram_zero(address: u64, bytes: u64) {
    let mut offset = 0;
    while offset < bytes {
        ram_write32(address + offset, 0);
        offset += 4;
    }
}

fn ram_read_bytes(address: u64, out: &mut [u8]) {
    for (index, byte) in out.iter_mut().enumerate() {
        // SAFETY: as in `ram_read32`.
        *byte =
            unsafe { ptr::with_exposed_provenance::<u8>(address as usize + index).read_volatile() };
    }
}

fn ram_write_words(address: u64, words: &[u32]) {
    for (index, word) in words.iter().enumerate() {
        ram_write32(address + index as u64 * 4, *word);
    }
}

/// Writes dwords 0..2, then dword 3 with the cycle bit: the controller must not see a partly
/// written TRB.
fn write_trb(address: u64, trb: Trb) {
    let words = trb.dwords();
    ram_write32(address, words[0]);
    ram_write32(address + 4, words[1]);
    ram_write32(address + 8, words[2]);
    compiler_fence(Ordering::Release);
    ram_write32(address + 12, words[3]);
}

fn read_trb(address: u64) -> Trb {
    let control = ram_read32(address + 12);
    compiler_fence(Ordering::Acquire);
    Trb::from_dwords([
        ram_read32(address),
        ram_read32(address + 4),
        ram_read32(address + 8),
        control,
    ])
}

fn wait(limit_us: usize, mut ready: impl FnMut() -> bool) -> bool {
    let mut waited = 0;
    loop {
        if ready() {
            return true;
        }
        if waited >= limit_us {
            return false;
        }
        // SAFETY: Boot Services are initialized before the driver starts; BSP only.
        if unsafe { stall(POLL_US) } != Status::SUCCESS {
            return false;
        }
        waited += POLL_US;
    }
}

pub(super) struct Xhci {
    regs: Mmio,
    operational: usize,
    runtime: usize,
    doorbells: usize,
    ports: u8,
    max_slots: u32,
    /// First of the [`DMA_PAGES`] DMA pages (identity mapped).
    dma: u64,
    command: RingCursor,
    events: EventCursor,
    ep0: RingCursor,
    interrupt: RingCursor,
    /// ECAM window, PCI device number and the Command register value read before the driver
    /// changed it; `quiesce` restores it.
    ecam: u64,
    pci_device: u8,
    saved_command: u32,
    /// The controller was reset by `quiesce` (ExitBootServices); no ring is touched afterwards.
    stopped: bool,
    pub(super) keyboard: Option<Keyboard>,
    pub(super) decoder: BootKeyboard,
}

/// The enumerated keyboard: slot, root-hub port and HID interface number.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Keyboard {
    pub(super) slot: u8,
    pub(super) port: u8,
    pub(super) interface: u8,
    /// Completion code of the last failed Interrupt IN transfer (0: none).
    pub(super) failure: u32,
}

impl Xhci {
    /// Reads and checks the capability registers; the controller is not touched otherwise.
    ///
    /// # Safety
    /// `bar0` is the validated xHCI register window (identity mapped, MMIO), `dma` the first of
    /// [`DMA_PAGES`] allocated, zeroed pages owned by the driver, `ecam` the ECAM window that
    /// holds PCI device `pci_device` (its Command register was `saved_command`). Boot Services
    /// run on the BSP.
    pub(super) unsafe fn new(
        bar0: u64,
        dma: u64,
        ecam: u64,
        pci_device: u8,
        saved_command: u32,
    ) -> Result<Self, UsbError> {
        let regs = Mmio(bar0 as usize);
        let version_length = regs.read(CAP_HCIVERSION_LENGTH);
        let length = (version_length & 0xff) as usize;
        let version = version_length >> 16;
        let params1 = regs.read(CAP_HCSPARAMS1);
        let params_cap = regs.read(CAP_HCCPARAMS1);
        let doorbell_offset = (regs.read(CAP_DBOFF) & !3) as usize;
        let runtime_offset = (regs.read(CAP_RTSOFF) & !0x1f) as usize;
        let max_slots = params1 & 0xff;
        let ports = (params1 >> 24) as u8;
        // Interface 1.0 or newer, 64-bit addressing, 32-byte contexts (CSZ = 0), at least one
        // port and slot, and register blocks inside the 16 KiB window.
        if version < 0x0100
            || length < 0x20
            || length + OP_PORTSC + 0x10 * usize::from(ports) > BAR_BYTES
        {
            return Err(UsbError::new(Step::Capabilities, version_length));
        }
        if params_cap & 1 == 0 || params_cap & (1 << 2) != 0 {
            return Err(UsbError::new(Step::Capabilities, params_cap));
        }
        if ports == 0
            || max_slots == 0
            || doorbell_offset + 4 * (max_slots as usize + 1) > BAR_BYTES
            || runtime_offset + RT_ERDP + 8 > BAR_BYTES
        {
            return Err(UsbError::new(Step::Capabilities, params1));
        }
        Ok(Self {
            regs,
            operational: length,
            runtime: runtime_offset,
            doorbells: doorbell_offset,
            ports,
            max_slots,
            dma,
            command: RingCursor::new(dma + PAGE_COMMAND_RING * PAGE, RING_TRBS),
            events: EventCursor::new(dma + PAGE_EVENT_RING * PAGE, EVENT_TRBS),
            ep0: RingCursor::new(dma + PAGE_EP0_RING * PAGE, RING_TRBS),
            interrupt: RingCursor::new(dma + PAGE_INTERRUPT_RING * PAGE, RING_TRBS),
            ecam,
            pci_device,
            saved_command,
            stopped: false,
            keyboard: None,
            decoder: BootKeyboard::new(),
        })
    }

    /// Resets the controller and brings it to Running with the command ring, the event ring
    /// and the device context array set up.
    pub(super) fn initialize(&mut self) -> Result<(), UsbError> {
        let max_slots = self.max_slots;
        self.reset()?;
        self.configure_rings(max_slots.min(SLOTS));
        self.run()
    }

    /// ExitBootServices (XHCI_DEVICE.md section 12.3): R/S = 0, HCH, HCRST, then the PCI
    /// Command register as found. Afterwards the controller references no memory and the driver
    /// touches no ring. Idempotent.
    pub(super) fn quiesce(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.keyboard = None;
        let _ = self.reset();
        // SAFETY: `new` guarantees the ECAM window and device number.
        let ecam = unsafe { super::pci::Ecam::new(self.ecam) };
        super::pci::ConfigSpace::write(
            &ecam,
            self.pci_device,
            0,
            super::pci::COMMAND_REGISTER,
            self.saved_command,
        );
    }

    fn op_read(&self, offset: usize) -> u32 {
        self.regs.read(self.operational + offset)
    }

    fn op_write(&self, offset: usize, value: u32) {
        self.regs.write(self.operational + offset, value);
    }

    fn page(&self, page: u64) -> u64 {
        self.dma + page * PAGE
    }

    /// R/S = 0 and HCH = 1, then HCRST and the CNR handshake (XHCI_DEVICE.md section 12.2:
    /// waits are bounded, a stuck controller is an error, not a hang).
    fn reset(&self) -> Result<(), UsbError> {
        let halted = |xhci: &Self| xhci.op_read(OP_USBSTS) & USBSTS_HCH != 0;
        self.op_write(OP_USBCMD, self.op_read(OP_USBCMD) & !USBCMD_RUN);
        if !wait(REGISTER_WAIT_US, || halted(self)) {
            return Err(UsbError::new(
                Step::ControllerReset,
                self.op_read(OP_USBSTS),
            ));
        }
        self.op_write(OP_USBCMD, USBCMD_HCRST);
        let finished = wait(REGISTER_WAIT_US, || {
            self.op_read(OP_USBCMD) & USBCMD_HCRST == 0 && self.op_read(OP_USBSTS) & USBSTS_CNR == 0
        });
        if !finished {
            return Err(UsbError::new(
                Step::ControllerReset,
                self.op_read(OP_USBSTS),
            ));
        }
        Ok(())
    }

    /// Stops the controller so it no longer references the DMA pages: R/S = 0, HCH, HCRST.
    /// Returns whether the controller reached the reset state.
    pub(super) fn stop(&self) -> bool {
        self.reset().is_ok()
    }

    fn configure_rings(&mut self, slots: u32) {
        // DMA pages come zeroed; ring and context pages are rewritten here and in `address`.
        let events = self.page(PAGE_EVENT_RING);
        let erst = self.page(PAGE_ERST);
        ram_write64(erst, events);
        ram_write32(erst + 8, EVENT_TRBS as u32);
        ram_write32(erst + 12, 0);
        self.op_write(OP_CONFIG, slots);
        self.regs
            .write64(self.operational + OP_DCBAAP, self.page(PAGE_DCBAA));
        self.regs
            .write64(self.operational + OP_CRCR, self.command.base() | CRCR_RCS);
        self.regs.write(self.runtime + RT_ERSTSZ, 1);
        // The model loads the ERST entry when the high dword of ERSTBA is written.
        self.regs.write64(self.runtime + RT_ERSTBA, erst);
        self.regs.write64(self.runtime + RT_ERDP, events);
    }

    fn run(&self) -> Result<(), UsbError> {
        self.op_write(OP_USBCMD, USBCMD_RUN);
        if !wait(REGISTER_WAIT_US, || {
            self.op_read(OP_USBSTS) & USBSTS_HCH == 0
        }) {
            return Err(UsbError::new(Step::Run, self.op_read(OP_USBSTS)));
        }
        Ok(())
    }

    /// The board advances its virtual clock and services the controller between guest quanta,
    /// and a guest write to an xHCI register ends the quantum (XHCI_DEVICE.md section 14,
    /// item 19c); inside a quantum the controller's time stands still. A wait for progress on
    /// the device side (a port reset takes 50 ms of that clock, a command or transfer needs one
    /// service call) therefore writes a register with no effect (USBSTS is write-1-to-clear:
    /// writing 0 clears nothing) on every poll. On hardware it is a harmless register write.
    fn yield_quantum(&self) {
        self.op_write(OP_USBSTS, 0);
    }

    fn ring_doorbell(&self, index: usize, target: u8) {
        self.regs
            .write(self.doorbells + 4 * index, u32::from(target));
    }

    fn write_erdp(&self) {
        let value = erdp_value(self.events.pending());
        self.regs.write64(self.runtime + RT_ERDP, value);
    }

    /// Takes the next event off the ring, if the controller has produced one, and tells the
    /// controller (ERDP with EHB = 1, XHCI_DEVICE.md section 15).
    fn next_event(&mut self) -> Option<Trb> {
        let trb = read_trb(self.events.pending());
        if !self.events.owns(trb) {
            return None;
        }
        self.events.accept();
        self.write_erdp();
        Some(trb)
    }

    /// Waits for an event that `matches`; Interrupt IN completions that arrive meanwhile go to
    /// the report decoder, every other event is dropped.
    fn wait_event(
        &mut self,
        step: Step,
        limit_us: usize,
        matches: impl Fn(&Trb) -> bool,
    ) -> Result<Trb, UsbError> {
        let mut waited = 0;
        loop {
            while let Some(event) = self.next_event() {
                if matches(&event) {
                    return Ok(event);
                }
                self.dispatch(event);
            }
            if waited >= limit_us {
                return Err(UsbError::new(step, self.op_read(OP_USBSTS)));
            }
            self.yield_quantum();
            // SAFETY: as in `wait`.
            if unsafe { stall(POLL_US) } != Status::SUCCESS {
                return Err(UsbError::new(step, self.op_read(OP_USBSTS)));
            }
            waited += POLL_US;
        }
    }

    fn dispatch(&mut self, event: Trb) {
        let Some(keyboard) = self.keyboard else {
            return;
        };
        if event.trb_type() == trb_type::TRANSFER_EVENT
            && event.slot_id() == keyboard.slot
            && event.endpoint_id() == DCI_INTERRUPT_IN
        {
            self.report_completed(event);
        }
    }

    fn command(&mut self, step: Step, trb: Trb) -> Result<Trb, UsbError> {
        let address = self.command.push(trb, write_trb);
        self.ring_doorbell(0, 0);
        let event = self.wait_event(step, TRANSFER_WAIT_US, |event| {
            event.trb_type() == trb_type::COMMAND_COMPLETION && event.parameter == address
        })?;
        if event.completion_code() != completion::SUCCESS {
            return Err(UsbError::new(step, event.completion_code()));
        }
        Ok(event)
    }

    /// One control transfer on EP0 of `slot`: Setup, optional Data, Status with IOC. The data
    /// buffer is a DMA address and the direction (true: device to host).
    fn control(
        &mut self,
        step: Step,
        slot: u8,
        setup: SetupPacket,
        data: Option<(u64, bool)>,
    ) -> Result<(), UsbError> {
        let transfer = match data {
            None => TransferType::NoData,
            Some((_, true)) => TransferType::In,
            Some((_, false)) => TransferType::Out,
        };
        self.ep0
            .push(Trb::setup_stage(setup.as_u64(), transfer), write_trb);
        if let Some((buffer, device_to_host)) = data {
            self.ep0.push(
                Trb::data_stage(buffer, u32::from(setup.length), device_to_host),
                write_trb,
            );
        }
        let device_to_host_status = data.is_none_or(|(_, device_to_host)| !device_to_host);
        self.ep0
            .push(Trb::status_stage(device_to_host_status), write_trb);
        self.ring_doorbell(usize::from(slot), DCI_EP0);
        let event = self.wait_event(step, TRANSFER_WAIT_US, |event| {
            event.trb_type() == trb_type::TRANSFER_EVENT
                && event.slot_id() == slot
                && event.endpoint_id() == DCI_EP0
        })?;
        if event.completion_code() != completion::SUCCESS {
            return Err(UsbError::new(step, event.completion_code()));
        }
        Ok(())
    }

    fn portsc(&self, port: u8) -> u32 {
        self.op_read(OP_PORTSC + 0x10 * (usize::from(port) - 1))
    }

    fn write_portsc(&self, port: u8, value: u32) {
        self.op_write(OP_PORTSC + 0x10 * (usize::from(port) - 1), value);
    }

    /// Port reset (XHCI_DEVICE.md section 6, 15 item 3): clear CSC, PR = 1, wait for PRC with
    /// PED = 1, clear PRC. The board completes it after 50 ms of virtual time.
    fn reset_port(&self, port: u8) -> Result<(), UsbError> {
        let preserved = |status: u32| status & PORTSC_PRESERVE;
        self.write_portsc(port, preserved(self.portsc(port)) | PORTSC_CSC);
        self.write_portsc(port, preserved(self.portsc(port)) | PORTSC_PR);
        let done = wait(PORT_RESET_WAIT_US, || {
            self.yield_quantum();
            self.portsc(port) & PORTSC_PRC != 0
        });
        let status = self.portsc(port);
        if !done || status & (PORTSC_PR | PORTSC_PED | PORTSC_CCS) != (PORTSC_PED | PORTSC_CCS) {
            return Err(UsbError::new(Step::PortReset, status));
        }
        self.write_portsc(port, preserved(status) | PORTSC_PRC);
        Ok(())
    }

    /// Finds and brings up the first HID boot keyboard on the root hub ports.
    pub(super) fn enumerate_keyboard(&mut self) -> Result<(), UsbError> {
        for port in 1..=self.ports {
            if self.portsc(port) & PORTSC_CCS == 0 {
                continue;
            }
            self.reset_port(port)?;
            if self.bring_up(port)? {
                return Ok(());
            }
        }
        Err(UsbError::new(Step::NoKeyboard, u32::from(self.ports)))
    }

    /// Enable Slot, Address Device and descriptor reads for the device on `port`. Returns
    /// `Ok(false)` after releasing the slot when the device is not a boot keyboard.
    fn bring_up(&mut self, port: u8) -> Result<bool, UsbError> {
        let slot = self
            .command(Step::EnableSlot, Trb::enable_slot())?
            .slot_id();
        let output = self.page(PAGE_OUTPUT_CONTEXT);
        let input = self.page(PAGE_INPUT_CONTEXT);
        ram_zero(output, DEVICE_CONTEXT_BYTES as u64);
        ram_zero(self.ep0.base(), PAGE);
        ram_zero(self.interrupt.base(), PAGE);
        self.ep0 = RingCursor::new(self.ep0.base(), RING_TRBS);
        self.interrupt = RingCursor::new(self.interrupt.base(), RING_TRBS);
        ram_write64(self.page(PAGE_DCBAA) + 8 * u64::from(slot), output);
        let context = InputContext::address_device(port, &self.ep0);
        ram_zero(input, INPUT_CONTEXT_BYTES as u64);
        ram_write_words(input, &context.0);
        self.command(Step::AddressDevice, Trb::address_device(input, slot))?;

        let descriptor = self.page(PAGE_DATA) + DATA_DESCRIPTOR;
        let mut bytes = [0u8; MAX_CONFIGURATION_BYTES];
        self.read_descriptor(
            Step::DeviceDescriptor,
            slot,
            SetupPacket::get_device_descriptor(),
            descriptor,
            &mut bytes[..DEVICE_DESCRIPTOR_BYTES],
        )?;
        let device = desc::parse_device(&bytes[..DEVICE_DESCRIPTOR_BYTES])
            .map_err(|_| UsbError::new(Step::DeviceDescriptor, 0xffff_ffff))?;
        // The contract fixes EP0 at 64 bytes (the Address Device input above); a device with
        // another size would be driven with a wrong packet size, so it is an error.
        if u32::from(device.max_packet_size0) != EP0_MAX_PACKET {
            return Err(UsbError::new(
                Step::DeviceDescriptor,
                u32::from(device.max_packet_size0),
            ));
        }
        self.read_descriptor(
            Step::ConfigurationDescriptor,
            slot,
            SetupPacket::get_configuration_descriptor(desc::CONFIGURATION_HEADER_BYTES as u16),
            descriptor,
            &mut bytes[..desc::CONFIGURATION_HEADER_BYTES],
        )?;
        let total = desc::configuration_total_length(&bytes[..desc::CONFIGURATION_HEADER_BYTES])
            .map_err(|_| UsbError::new(Step::ConfigurationDescriptor, 0xffff_ffff))?;
        if total > MAX_CONFIGURATION_BYTES {
            return Err(UsbError::new(Step::ConfigurationDescriptor, total as u32));
        }
        self.read_descriptor(
            Step::ConfigurationDescriptor,
            slot,
            SetupPacket::get_configuration_descriptor(total as u16),
            descriptor,
            &mut bytes[..total],
        )?;
        let keyboard = desc::parse_keyboard(&bytes[..total])
            .map_err(|_| UsbError::new(Step::ConfigurationDescriptor, 0xffff_fffe))?;
        let Some(interface) = keyboard else {
            // Not a boot keyboard (the pointer): release the slot and go on.
            self.command(Step::DisableSlot, Trb::disable_slot(slot))?;
            return Ok(false);
        };
        self.configure_keyboard(port, slot, interface)?;
        Ok(true)
    }

    fn read_descriptor(
        &mut self,
        step: Step,
        slot: u8,
        setup: SetupPacket,
        buffer: u64,
        out: &mut [u8],
    ) -> Result<(), UsbError> {
        ram_zero(buffer, MAX_CONFIGURATION_BYTES as u64);
        self.control(step, slot, setup, Some((buffer, true)))?;
        ram_read_bytes(buffer, out);
        Ok(())
    }

    /// SET_CONFIGURATION, Configure Endpoint for DCI 3, then boot protocol, idle 0 and LEDs
    /// off (XHCI_DEVICE.md section 15, item 3), and the first Interrupt IN TRB.
    fn configure_keyboard(
        &mut self,
        port: u8,
        slot: u8,
        interface: KeyboardInterface,
    ) -> Result<(), UsbError> {
        if interface.max_packet_size < BOOT_REPORT_BYTES as u16 || interface.max_packet_size > 64 {
            return Err(UsbError::new(
                Step::ConfigurationDescriptor,
                u32::from(interface.max_packet_size),
            ));
        }
        self.control(
            Step::SetConfiguration,
            slot,
            SetupPacket::set_configuration(interface.configuration),
            None,
        )?;
        let input = self.page(PAGE_INPUT_CONTEXT);
        let context = InputContext::configure_interrupt_in(
            port,
            desc::full_speed_interval_exponent(interface.interval),
            u32::from(interface.max_packet_size),
            &self.interrupt,
        );
        ram_zero(input, INPUT_CONTEXT_BYTES as u64);
        ram_write_words(input, &context.0);
        self.command(
            Step::ConfigureEndpoint,
            Trb::configure_endpoint(input, slot),
        )?;
        self.control(
            Step::SetProtocol,
            slot,
            SetupPacket::set_protocol(interface.interface, 0),
            None,
        )?;
        self.control(
            Step::SetIdle,
            slot,
            SetupPacket::set_idle(interface.interface, 0),
            None,
        )?;
        let led = self.page(PAGE_DATA) + DATA_LED;
        ram_write32(led, 0);
        self.control(
            Step::SetReport,
            slot,
            SetupPacket::set_output_report(interface.interface),
            Some((led, false)),
        )?;
        self.keyboard = Some(Keyboard {
            slot,
            port,
            interface: interface.interface,
            failure: 0,
        });
        self.arm_interrupt(slot);
        Ok(())
    }

    /// Queues one Interrupt IN TRB for a boot report and rings the endpoint's doorbell.
    fn arm_interrupt(&mut self, slot: u8) {
        let report = self.page(PAGE_DATA) + DATA_REPORT;
        ram_write64(report, 0);
        self.interrupt
            .push(Trb::normal(report, BOOT_REPORT_BYTES as u32), write_trb);
        self.ring_doorbell(usize::from(slot), DCI_INTERRUPT_IN);
    }

    /// An Interrupt IN completion: decode the report and queue the TRB again. A transfer
    /// error is recorded and the endpoint is not re-armed.
    fn report_completed(&mut self, event: Trb) {
        let Some(mut keyboard) = self.keyboard else {
            return;
        };
        let code = event.completion_code();
        if code != completion::SUCCESS && code != completion::SHORT_PACKET {
            keyboard.failure = code;
            self.keyboard = Some(keyboard);
            return;
        }
        // Only a full 8-byte report is decoded; a short one is dropped.
        if event.residual() == 0 {
            let mut report = [0u8; BOOT_REPORT_BYTES];
            ram_read_bytes(self.page(PAGE_DATA) + DATA_REPORT, &mut report);
            self.decoder.process_report(&report);
        }
        self.arm_interrupt(keyboard.slot);
    }

    /// Polls the event ring: decodes finished Interrupt IN reports into the key queue.
    pub(super) fn poll(&mut self) {
        if self.stopped {
            return;
        }
        while let Some(event) = self.next_event() {
            self.dispatch(event);
        }
    }
}

/// Allocates and zeroes the driver's DMA pages (Boot Services data).
///
/// # Safety
/// BSP only, after Boot Services memory is initialized.
pub(super) unsafe fn allocate_dma() -> Result<u64, UsbError> {
    let mut base = 0u64;
    // SAFETY: `base` is a writable local; allocation type 0 is AllocateAnyPages.
    let status = unsafe { allocate_pages(0, MemoryType::BOOT_SERVICES_DATA, DMA_PAGES, &mut base) };
    if status != Status::SUCCESS {
        return Err(UsbError::new(Step::Allocate, status.raw() as u32));
    }
    ram_zero(base, DMA_PAGES as u64 * PAGE);
    Ok(base)
}

const _: () = {
    // Each ring fills at most its page.
    assert!(RING_TRBS as u64 * TRB_BYTES <= PAGE);
    assert!(EVENT_TRBS as u64 * TRB_BYTES <= PAGE);
    assert!(INPUT_CONTEXT_BYTES as u64 <= PAGE);
    assert!(DEVICE_CONTEXT_BYTES as u64 <= PAGE);
    assert!(DATA_LED < PAGE && DATA_REPORT + 8 <= DATA_LED);
    assert!(DATA_DESCRIPTOR + MAX_CONFIGURATION_BYTES as u64 <= DATA_REPORT);
};
