//! Controller ownership and synchronous NVMe commands for Boot Services.

use core::{cell::Cell, ptr};

use crate::{
    acpi::manifest::device_type,
    bs::{
        event::stall,
        memory::{allocate_pages, free_pages},
    },
    handoff::{Handoff, region_kind},
    mem::map::MemoryType,
    paging::PAGE_BYTES,
    uefi::status::Status,
};

use super::queue::{Command, DEPTH, Queue};

pub(crate) const REGISTERS_BASE: u64 = 0xfc00_1000;
pub(crate) const REGISTERS_BYTES: u64 = 0x2000;
/// NVM Express: base class 0x01 (mass storage), subclass 0x08 (NVM), programming interface 0x02.
const NVME_CLASS: u32 = 0x01_0802;
pub(crate) const NAMESPACE_ID: u32 = 1;
pub(crate) const BLOCK_BYTES: usize = 512;
const PAGE: usize = PAGE_BYTES as usize;
const DMA_PAGES: usize = 5;
pub(super) const POLL_US: usize = 10;
const CSTS: usize = 0x1c;
const CC: usize = 0x14;
const RDY: u32 = 1;
const CFS: u32 = 1 << 1;
const ENABLE: u32 = 1 | (6 << 16) | (4 << 20);
const AQA: usize = 0x24;
const ASQ: usize = 0x28;
const ACQ: usize = 0x30;

#[derive(Clone, Copy)]
pub(super) struct Registers(usize);

impl Registers {
    pub(super) fn read(self, offset: usize) -> u32 {
        // SAFETY: discovery validated this MMIO window (fixed or PCI BAR0). Callers use aligned dwords.
        unsafe { ptr::with_exposed_provenance::<u32>(self.0 + offset).read_volatile() }
    }

    pub(super) fn write(self, offset: usize, word: u32) {
        // SAFETY: discovery validated this MMIO window (fixed or PCI BAR0). Callers use aligned dwords.
        unsafe { ptr::with_exposed_provenance_mut::<u32>(self.0 + offset).write_volatile(word) };
    }

    fn write_address(self, offset: usize, address: u64) {
        self.write(offset, address as u32);
        self.write(offset + 4, (address >> 32) as u32);
    }

    pub(super) fn failed(self) -> bool {
        self.read(CSTS) & (RDY | CFS) != RDY
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct Namespace {
    pub(crate) sectors: u64,
    pub(crate) eui: u64,
}

/// Where the controller's registers are (S7-32).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Location {
    /// The firmware's fixed polled window (handoff v1.1/v1.2 entry).
    Fixed,
    /// BAR0 of function 0 of `device` on PCI bus 0 (handoff v1.3 entry with a slot).
    Pci { device: u8 },
}

/// Why a PCI function is not the board's NVMe controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PciNvmeError {
    /// No function at the slot, or another class.
    Class,
    /// BAR0 is not a 64-bit memory BAR of the entry's size, or not at the entry's base.
    Bar,
}

impl PciNvmeError {
    const fn name(self) -> &'static [u8] {
        match self {
            Self::Class => b"class",
            Self::Bar => b"bar",
        }
    }
}

const PCI_REFUSED_PREFIX: &[u8] = b"OD-FW nvme: PCI function refused step=";
const PCI_REFUSED_LINE_BYTES: usize = 64;

/// `OD-FW nvme: PCI function refused step=<class|bar> device=0xNN\r\n` into `out` (the format of
/// the xHCI failure lines); returns the length.
fn format_pci_refusal(
    error: PciNvmeError,
    device: u8,
    out: &mut [u8; PCI_REFUSED_LINE_BYTES],
) -> usize {
    let mut length = 0;
    let mut put = |bytes: &[u8]| {
        out[length..length + bytes.len()].copy_from_slice(bytes);
        length += bytes.len();
    };
    put(PCI_REFUSED_PREFIX);
    put(error.name());
    put(b" device=0x");
    let digit = |nibble: u8| b"0123456789ABCDEF"[usize::from(nibble & 0xf)];
    put(&[digit(device >> 4), digit(device)]);
    put(b"\r\n");
    length
}

/// Checks function 0 of `device` against the handoff entry (class 0x010802, BAR0 a 64-bit memory
/// BAR of `length` bytes at `base`) and turns memory decoding and bus mastering on; returns the
/// command register before the call. The BAR is probed with decoding and bus mastering off; on a
/// failed check the command register is restored.
fn enable_pci_function(
    config: &impl crate::pci::ConfigSpace,
    device: u8,
    base: u64,
    length: u64,
) -> Result<u32, PciNvmeError> {
    use crate::pci::{COMMAND_BUS_MASTER, COMMAND_MEMORY_SPACE, COMMAND_REGISTER};
    if crate::pci::function_class(config, device) != Some(NVME_CLASS) {
        return Err(PciNvmeError::Class);
    }
    let saved = config.read(device, 0, COMMAND_REGISTER) & 0xffff;
    config.write(
        device,
        0,
        COMMAND_REGISTER,
        saved & !(COMMAND_MEMORY_SPACE | COMMAND_BUS_MASTER),
    );
    if crate::pci::probe_bar0(config, device, length) != Ok(base) {
        config.write(device, 0, COMMAND_REGISTER, saved);
        return Err(PciNvmeError::Bar);
    }
    config.write(
        device,
        0,
        COMMAND_REGISTER,
        saved | COMMAND_MEMORY_SPACE | COMMAND_BUS_MASTER,
    );
    Ok(saved)
}

/// One COM1 line for a refused PCI function (target builds only).
fn report_pci_refusal(error: PciNvmeError, device: u8) {
    let mut line = [0u8; PCI_REFUSED_LINE_BYTES];
    let length = format_pci_refusal(error, device, &mut line);
    #[cfg(any(target_os = "none", target_os = "uefi"))]
    crate::console::serial_term::write(&line[..length]);
    #[cfg(not(any(target_os = "none", target_os = "uefi")))]
    let _ = length;
}

/// The PCI function the driver turned on and its command register before: restored whenever the
/// driver lets the controller go (a failed bring-up, [`Controller::release`] from the rollback of
/// `storage::install`). ExitBootServices only quiesces the controller and leaves decoding on for
/// the OS.
#[derive(Clone, Copy)]
struct PciFunction {
    ecam: u64,
    device: u8,
    saved_command: u32,
}

impl PciFunction {
    fn restore(self) {
        // SAFETY: discovery took `ecam` from the handoff ECAM entry, which covers bus 0 and is
        // identity mapped.
        let config = unsafe { crate::pci::Ecam::new(self.ecam) };
        crate::pci::ConfigSpace::write(
            &config,
            self.device,
            0,
            crate::pci::COMMAND_REGISTER,
            self.saved_command,
        );
    }
}

pub(crate) struct Controller {
    registers: Registers,
    location: Location,
    pci: Option<PciFunction>,
    admin: Queue,
    io: Queue,
    data: u64,
    ready_waits: usize,
    namespace: Namespace,
    busy: Cell<bool>,
    online: Cell<bool>,
}

struct Busy<'a>(&'a Cell<bool>);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl Controller {
    /// # Safety
    /// Call on the BSP with the live board's identity map and Boot Services allocator.
    pub(crate) unsafe fn discover(handoff: &Handoff<'_>) -> Result<Option<Self>, Status> {
        let (devices, count) = handoff.device_entries();
        let Some((index, entry)) = devices[..count]
            .iter()
            .enumerate()
            .find(|(_, entry)| entry.device_type == device_type::NVME && entry.is_present())
        else {
            return Ok(None);
        };
        // Every check that does not touch the controller comes first: the configuration space and
        // the registers are reached only for an entry the handoff fully describes.
        let (location, pci_target) = if entry.is_pci_nvme() {
            // S7-32: the controller is a PCI function; the handoff names its slot and BAR0, which
            // holds at least the registers and the doorbells the driver uses.
            let Ok(Some(ecam)) = crate::pci::ecam_window(handoff) else {
                return Err(Status::UNSUPPORTED);
            };
            let Some(device) = u8::try_from(entry.param0)
                .ok()
                .filter(|device| *device < 32)
            else {
                return Err(Status::UNSUPPORTED);
            };
            if entry.length < REGISTERS_BYTES {
                return Err(Status::UNSUPPORTED);
            }
            (Location::Pci { device }, Some((ecam, device)))
        } else if entry.base == REGISTERS_BASE && entry.length == REGISTERS_BYTES {
            (Location::Fixed, None)
        } else {
            return Err(Status::UNSUPPORTED);
        };
        if !handoff.regions().any(|region| {
            region.kind == region_kind::MMIO
                && region.owner == index as u32
                && region.base == entry.base
                && region.length == entry.length
        }) {
            return Err(Status::UNSUPPORTED);
        }
        let pci = match pci_target {
            Some((ecam, device)) => {
                // SAFETY: the handoff ECAM entry covers bus 0 and the window is identity mapped.
                let config = unsafe { crate::pci::Ecam::new(ecam) };
                match enable_pci_function(&config, device, entry.base, entry.length) {
                    Ok(saved_command) => Some(PciFunction {
                        ecam,
                        device,
                        saved_command,
                    }),
                    Err(error) => {
                        report_pci_refusal(error, device);
                        return Err(Status::UNSUPPORTED);
                    }
                }
            }
            None => None,
        };
        let refuse = |status: Status| {
            if let Some(pci) = pci {
                pci.restore();
            }
            Err(status)
        };
        let registers = Registers(entry.base as usize);
        let cap_low = registers.read(0);
        let cap_high = registers.read(4);
        // Fixed queue depth, doorbell stride 0, NVM command set, 4 KiB host pages.
        if cap_low & 0xffff < DEPTH - 1
            || cap_high & (0x0f | 0x000f_0000) != 0
            || cap_high & (1 << 5) == 0
        {
            return refuse(Status::UNSUPPORTED);
        }
        let mut base = 0;
        let status =
            unsafe { allocate_pages(0, MemoryType::BOOT_SERVICES_DATA, DMA_PAGES, &mut base) };
        if status != Status::SUCCESS {
            return refuse(status);
        }
        let mut controller = Self {
            registers,
            location,
            pci,
            admin: Queue::new(0, base, base + PAGE_BYTES),
            io: Queue::new(1, base + 2 * PAGE_BYTES, base + 3 * PAGE_BYTES),
            data: base + 4 * PAGE_BYTES,
            ready_waits: (((cap_low >> 24) as usize).max(1) * 500_000) / POLL_US,
            namespace: Namespace { sectors: 0, eui: 0 },
            busy: Cell::new(false),
            online: Cell::new(false),
        };
        match controller.start() {
            Ok(namespace) => {
                controller.namespace = namespace;
                Ok(Some(controller))
            }
            Err(error) => {
                unsafe { controller.release() };
                Err(error)
            }
        }
    }

    /// # Safety
    /// Called before publication, or after all handles and events have been removed.
    pub(crate) unsafe fn release(self) {
        self.quiesce();
        // Keep the queues owned until the controller has stopped referencing guest RAM.
        if self.wait_ready(false).is_ok() {
            unsafe { free_pages(self.admin.submission, DMA_PAGES) };
        }
        // A PCI function gets its command register back (bus mastering off unless it was on).
        if let Some(pci) = self.pci {
            pci.restore();
        }
    }

    pub(crate) fn namespace(&self) -> Namespace {
        self.namespace
    }

    pub(crate) fn location(&self) -> Location {
        self.location
    }

    pub(crate) fn quiesce(&self) {
        self.online.set(false);
        self.registers.write(CC, 0);
    }

    fn wait_ready(&self, ready: bool) -> Result<(), Status> {
        let mut waits = self.ready_waits;
        loop {
            let status = self.registers.read(CSTS);
            if ready && status & CFS != 0 {
                return Err(Status::DEVICE_ERROR);
            }
            if (status & RDY != 0) == ready {
                return Ok(());
            }
            if waits == 0 {
                return Err(Status::TIMEOUT);
            }
            let status = unsafe { stall(POLL_US) };
            if status != Status::SUCCESS {
                return Err(status);
            }
            waits -= 1;
        }
    }

    fn start(&self) -> Result<Namespace, Status> {
        self.quiesce();
        self.wait_ready(false)?;
        // SAFETY: both queues are private DMA pages and the disabled controller is idle.
        unsafe {
            self.admin.reset();
            self.io.reset();
        }
        self.registers.write(AQA, (DEPTH - 1) | ((DEPTH - 1) << 16));
        self.registers.write_address(ASQ, self.admin.submission);
        self.registers.write_address(ACQ, self.admin.completion);
        self.registers.write(CC, ENABLE);
        self.wait_ready(true)?;

        self.send(
            &self.admin,
            Command {
                opcode: 0x06,
                data: self.data,
                cdw10: 1,
                ..Command::default()
            },
        )?;
        let data = self.buffer();
        // SAFETY: Identify has completed; the bounce page is exclusively owned by the BSP.
        let namespaces = unsafe { data.add(516).cast::<u32>().read_volatile() };
        if namespaces < NAMESPACE_ID {
            return Err(Status::NO_MEDIA);
        }
        self.send(
            &self.admin,
            Command {
                opcode: 0x06,
                namespace: NAMESPACE_ID,
                data: self.data,
                ..Command::default()
            },
        )?;
        let (sectors, format, lbaf, eui) = unsafe {
            (
                data.cast::<u64>().read_volatile(),
                data.add(26).read_volatile(),
                data.add(128).cast::<u32>().read_volatile(),
                data.add(120).cast::<u64>().read_volatile(),
            )
        };
        if sectors == 0 {
            return Err(Status::NO_MEDIA);
        }
        if format != 0
            || lbaf & 0x00ff_ffff != 9 << 16
            || sectors.checked_mul(BLOCK_BYTES as u64).is_none()
        {
            return Err(Status::UNSUPPORTED);
        }

        let queue_size = 1 | ((DEPTH - 1) << 16);
        self.send(
            &self.admin,
            Command {
                opcode: 0x05,
                data: self.io.completion,
                cdw10: queue_size,
                cdw11: 1,
                ..Command::default()
            },
        )?;
        self.send(
            &self.admin,
            Command {
                opcode: 0x01,
                data: self.io.submission,
                cdw10: queue_size,
                cdw11: 1 | (1 << 16),
                ..Command::default()
            },
        )?;
        self.online.set(true);
        Ok(Namespace { sectors, eui })
    }

    fn send(&self, queue: &Queue, command: Command) -> Result<(), Status> {
        // SAFETY: only initialization or a caller holding Busy can own the queue.
        match unsafe { queue.submit(self.registers, command) } {
            Ok(0) => Ok(()),
            Ok(0x82) => Err(Status::NO_MEDIA),
            Ok(_) => Err(Status::DEVICE_ERROR),
            Err(error) => {
                self.quiesce();
                Err(error)
            }
        }
    }

    /// Only one complete user operation may use the DMA pages and rings, including during Stall.
    pub(crate) fn exclusive<T>(
        &self,
        operation: impl FnOnce(&Self) -> Result<T, Status>,
    ) -> Result<T, Status> {
        if self.busy.replace(true) {
            return Err(Status::NOT_READY);
        }
        let _busy = Busy(&self.busy);
        operation(self)
    }

    pub(crate) fn reset(&self) -> Result<(), Status> {
        let status = match self.start() {
            Ok(namespace) if namespace == self.namespace => return Ok(()),
            Ok(_) => Status::MEDIA_CHANGED,
            Err(status) => status,
        };
        self.quiesce();
        Err(status)
    }

    pub(crate) fn buffer(&self) -> *mut u8 {
        ptr::with_exposed_provenance_mut(self.data as usize)
    }

    pub(crate) fn transfer(&self, write: bool, lba: u64, sectors: u32) -> Result<(), Status> {
        if sectors == 0
            || sectors as usize > PAGE / BLOCK_BYTES
            || lba
                .checked_add(u64::from(sectors))
                .is_none_or(|end| end > self.namespace.sectors)
        {
            return Err(Status::INVALID_PARAMETER);
        }
        self.send_io(Command {
            opcode: if write { 0x01 } else { 0x02 },
            namespace: NAMESPACE_ID,
            data: self.data,
            cdw10: lba as u32,
            cdw11: (lba >> 32) as u32,
            cdw12: sectors - 1,
        })
    }

    pub(crate) fn flush(&self) -> Result<(), Status> {
        self.send_io(Command {
            namespace: NAMESPACE_ID,
            ..Command::default()
        })
    }

    fn send_io(&self, command: Command) -> Result<(), Status> {
        if !self.online.get() {
            return Err(Status::NOT_READY);
        }
        self.send(&self.io, command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pci::{COMMAND_REGISTER, ConfigSpace, fake::Fake};

    const MSE_BME: u32 = (1 << 1) | (1 << 2);

    #[test]
    fn the_board_nvme_function_gets_memory_decoding_and_bus_mastering() {
        let fake = Fake::board();
        fake.write(4, 0, COMMAND_REGISTER, 0x0400 | MSE_BME);
        assert_eq!(
            enable_pci_function(&fake, 4, 0xfc00_4000, 0x4000),
            Ok(0x0400 | MSE_BME)
        );
        assert_eq!(fake.read(4, 0, COMMAND_REGISTER), 0x0400 | MSE_BME);
        assert_eq!(
            fake.read(4, 0, crate::pci::BAR0_LOW),
            0xfc00_4004,
            "BAR0 restored"
        );
        // The probe wrote BAR0 four times (all ones and back, both halves), each time with
        // decoding and bus mastering off.
        let writes = fake.bar_write_commands.borrow();
        assert_eq!(writes.len(), 4);
        assert!(
            writes
                .iter()
                .all(|&(device, command)| device == 4 && command & MSE_BME == 0)
        );
    }

    #[test]
    fn another_class_a_missing_function_or_another_bar_is_refused_and_command_restored() {
        let fake = Fake::board();
        // Device 5 is the xHCI function, device 6 is absent.
        assert_eq!(
            enable_pci_function(&fake, 5, 0xfc00_8000, 0x4000),
            Err(PciNvmeError::Class)
        );
        assert_eq!(
            enable_pci_function(&fake, 6, 0xfc00_4000, 0x4000),
            Err(PciNvmeError::Class)
        );
        assert!(
            fake.bar_write_commands.borrow().is_empty(),
            "no probe without the class"
        );
        // The entry names another base or another size than BAR0.
        fake.write(4, 0, COMMAND_REGISTER, 0x0400);
        assert_eq!(
            enable_pci_function(&fake, 4, 0xfc00_8000, 0x4000),
            Err(PciNvmeError::Bar)
        );
        assert_eq!(
            enable_pci_function(&fake, 4, 0xfc00_4000, 0x2000),
            Err(PciNvmeError::Bar)
        );
        assert_eq!(
            fake.read(4, 0, COMMAND_REGISTER),
            0x0400,
            "command restored"
        );
        // A 32-bit memory BAR.
        fake.functions.borrow_mut()[0].1[4] = 0xfc00_4000;
        assert_eq!(
            enable_pci_function(&fake, 4, 0xfc00_4000, 0x4000),
            Err(PciNvmeError::Bar)
        );
    }

    #[test]
    fn a_refused_function_gets_one_com1_line_in_the_xhci_format() {
        let mut line = [0u8; PCI_REFUSED_LINE_BYTES];
        let length = format_pci_refusal(PciNvmeError::Bar, 4, &mut line);
        assert_eq!(
            &line[..length],
            b"OD-FW nvme: PCI function refused step=bar device=0x04\r\n"
        );
        let length = format_pci_refusal(PciNvmeError::Class, 0x1f, &mut line);
        assert_eq!(
            &line[..length],
            b"OD-FW nvme: PCI function refused step=class device=0x1F\r\n"
        );
    }
}
