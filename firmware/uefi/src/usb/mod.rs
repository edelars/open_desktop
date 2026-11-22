//! Pre-boot USB: xHCI controller and HID boot keyboard (S7-34, slice A).
//!
//! Slice A enumerates the board's xHCI function through ECAM, resets the controller, brings the
//! keyboard on a root-hub port to boot protocol and polls its Interrupt IN endpoint into a key
//! queue. An ExitBootServices handler stops the controller and restores PCI Command
//! (XHCI_DEVICE.md sections 12.3 and 15, item 6). Not done yet: the queue is not connected to
//! `ConIn` (slice B), and the register-level check of section 12.3 is slice C (ISSUE-648).
//! Contract: XHCI_DEVICE.md section 15.

use core::{ffi::c_void, ptr};

use crate::{
    acpi::manifest::device_type,
    bs::event::{EVT_SIGNAL_EXIT_BOOT_SERVICES, Event, TPL_NOTIFY},
    console::text_in::InputKey,
    handoff::Handoff,
    paging::PAGE_BYTES,
    uefi::{status::Status, system_table::SystemTable},
};

mod desc;
mod hid_keyboard;
mod pci;
mod trb;
mod xhci;

use pci::{COMMAND_BUS_MASTER, COMMAND_INTX_DISABLE, COMMAND_MEMORY_SPACE, COMMAND_REGISTER};
use xhci::Xhci;

/// The stage of bring-up that failed; its name is printed on COM1.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Step {
    /// The handoff ECAM entry cannot hold bus 0 (detail: its length).
    Ecam,
    /// BAR0 is not the 64-bit 16 KiB memory BAR of the contract (detail: probed size, or 0).
    Bar,
    /// DMA page allocation (detail: EFI_STATUS).
    Allocate,
    /// HCIVERSION, CSZ/AC64, ports, slots or register blocks (detail: the register value).
    Capabilities,
    /// R/S = 0 or HCRST handshake timed out (detail: USBSTS).
    ControllerReset,
    /// R/S = 1 did not clear HCH (detail: USBSTS).
    Run,
    /// Port reset did not finish (detail: PORTSC).
    PortReset,
    /// Enable Slot (detail: completion code, or USBSTS after a timeout).
    EnableSlot,
    /// Address Device (same detail).
    AddressDevice,
    /// GET_DESCRIPTOR(device) or an unusable device descriptor (detail: completion code,
    /// bMaxPacketSize0 when it is not 64, or 0xFFFFFFFF for a malformed descriptor).
    DeviceDescriptor,
    /// GET_DESCRIPTOR(configuration) or an unusable set (detail: completion code, total length,
    /// wMaxPacketSize, 0xFFFFFFFF or 0xFFFFFFFE for a malformed set).
    ConfigurationDescriptor,
    /// Disable Slot for a device that is not a keyboard.
    DisableSlot,
    /// No port holds a boot keyboard (detail: the number of ports).
    NoKeyboard,
    SetConfiguration,
    ConfigureEndpoint,
    SetProtocol,
    SetIdle,
    SetReport,
    /// Registering the ExitBootServices handler (detail: EFI_STATUS).
    ExitHandler,
}

impl Step {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ecam => "ecam",
            Self::Bar => "bar0",
            Self::Allocate => "allocate",
            Self::Capabilities => "capabilities",
            Self::ControllerReset => "controller-reset",
            Self::Run => "run",
            Self::PortReset => "port-reset",
            Self::EnableSlot => "enable-slot",
            Self::AddressDevice => "address-device",
            Self::DeviceDescriptor => "device-descriptor",
            Self::ConfigurationDescriptor => "configuration-descriptor",
            Self::DisableSlot => "disable-slot",
            Self::NoKeyboard => "no-keyboard",
            Self::SetConfiguration => "set-configuration",
            Self::ConfigureEndpoint => "configure-endpoint",
            Self::SetProtocol => "set-protocol",
            Self::SetIdle => "set-idle",
            Self::SetReport => "set-report",
            Self::ExitHandler => "exit-handler",
        }
    }
}

/// A failed bring-up step and its detail word (the meaning is per step, see [`Step`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UsbError {
    pub step: Step,
    pub detail: u32,
}

impl UsbError {
    pub const fn new(step: Step, detail: u32) -> Self {
        Self { step, detail }
    }
}

const ENUMERATED_LINE: &[u8] = b"OD-FW xhci: keyboard enumerated\r\n";
const FAILED_PREFIX: &[u8] = b"OD-FW xhci: enumeration failed step=";
#[cfg(any(target_os = "none", target_os = "uefi"))]
const STOP_FAILED_PREFIX: &[u8] = b"OD-FW xhci: stop at exit failed step=";
const DETAIL_INFIX: &[u8] = b" detail=0x";

const fn hex_digit(nibble: u32) -> u8 {
    b"0123456789ABCDEF"[(nibble & 0xf) as usize]
}

/// `OD-FW xhci: enumeration failed step=<name> detail=0xXXXXXXXX\r\n` into `out`; returns
/// the length.
fn format_failure(error: UsbError, out: &mut [u8; 128]) -> usize {
    format_line(FAILED_PREFIX, error, out)
}

/// `<prefix><step name> detail=0xXXXXXXXX\r\n` into `out`.
fn format_line(prefix: &[u8], error: UsbError, out: &mut [u8; 128]) -> usize {
    let mut length = 0;
    let mut put = |bytes: &[u8]| {
        out[length..length + bytes.len()].copy_from_slice(bytes);
        length += bytes.len();
    };
    put(prefix);
    put(error.step.name().as_bytes());
    put(DETAIL_INFIX);
    let mut digits = [0u8; 8];
    for (index, digit) in digits.iter_mut().enumerate() {
        *digit = hex_digit(error.detail >> (28 - 4 * index as u32));
    }
    put(&digits);
    put(b"\r\n");
    length
}

/// The driver after a successful bring-up (one controller, one keyboard).
static mut DRIVER: Option<Xhci> = None;

/// The ECAM base of the board, when the handoff carries the window.
fn ecam_window(handoff: &Handoff<'_>) -> Result<Option<u64>, UsbError> {
    let (devices, count) = handoff.device_entries();
    let Some(entry) = devices[..count]
        .iter()
        .find(|entry| entry.device_type == device_type::PCIE_ECAM && entry.is_present())
    else {
        return Ok(None);
    };
    if entry.is_pio() || entry.length < pci::ECAM_BUS_BYTES {
        return Err(UsbError::new(Step::Ecam, entry.length as u32));
    }
    Ok(Some(entry.base))
}

/// Discovers the xHCI function and brings the keyboard up. `Ok(None)`: the board has no
/// xHCI function (no ECAM window, or no function of class 0x0C0330 on bus 0).
///
/// # Safety
/// BSP only, after Boot Services (memory, stall clock) are initialized and the board's MMIO
/// windows are identity mapped.
unsafe fn start(handoff: &Handoff<'_>) -> Result<Option<Xhci>, UsbError> {
    use pci::ConfigSpace;
    let Some(base) = ecam_window(handoff)? else {
        return Ok(None);
    };
    // SAFETY: the handoff ECAM entry covers bus 0 and the window is mapped.
    let ecam = unsafe { pci::Ecam::new(base) };
    let Some(device) = pci::find_xhci(&ecam) else {
        return Ok(None);
    };
    let saved = ecam.read(device, 0, COMMAND_REGISTER) & 0xffff;
    // The BAR size probe needs decoding off.
    ecam.write(
        device,
        0,
        COMMAND_REGISTER,
        saved & !(COMMAND_MEMORY_SPACE | COMMAND_BUS_MASTER),
    );
    let restore = |ecam: &pci::Ecam| ecam.write(device, 0, COMMAND_REGISTER, saved);
    let bar0 = match pci::probe_bar0(&ecam, device) {
        Ok(address) => address,
        Err(pci::BarError::Size(size)) => {
            restore(&ecam);
            return Err(UsbError::new(Step::Bar, size as u32));
        }
        Err(_) => {
            restore(&ecam);
            return Err(UsbError::new(Step::Bar, 0));
        }
    };
    ecam.write(
        device,
        0,
        COMMAND_REGISTER,
        saved | COMMAND_MEMORY_SPACE | COMMAND_BUS_MASTER | COMMAND_INTX_DISABLE,
    );
    // SAFETY: BSP, Boot Services initialized.
    let dma = match unsafe { xhci::allocate_dma() } {
        Ok(dma) => dma,
        Err(error) => {
            restore(&ecam);
            return Err(error);
        }
    };
    // SAFETY: `bar0` passed the size and type checks and Command.MSE is set; `dma` is the
    // driver's zeroed allocation; `device` sits in the ECAM window at `base`.
    let created = unsafe { Xhci::new(bar0, dma, base, device, saved) };
    let mut controller = match created {
        Ok(controller) => controller,
        Err(error) => {
            release(&ecam, device, saved, None, dma);
            return Err(error);
        }
    };
    if let Err(error) = controller
        .initialize()
        .and_then(|()| controller.enumerate_keyboard())
    {
        release(&ecam, device, saved, Some(&controller), dma);
        return Err(error);
    }
    Ok(Some(controller))
}

/// ExitBootServices handler (XHCI_DEVICE.md section 12.3).
unsafe extern "efiapi" fn exit_boot_services(_: Event, _: *mut c_void) {
    // SAFETY: BSP; the handler runs at TPL_NOTIFY, so no other code holds the driver.
    if let Some(driver) = unsafe { (*ptr::addr_of_mut!(DRIVER)).as_mut() }
        && let Err(error) = driver.quiesce()
    {
        // The controller may still own its DMA pages when the OS takes the memory: say so.
        #[cfg(any(target_os = "none", target_os = "uefi"))]
        {
            let mut line = [0u8; 128];
            let length = format_line(STOP_FAILED_PREFIX, error, &mut line);
            crate::console::serial_term::write(&line[..length]);
        }
        #[cfg(not(any(target_os = "none", target_os = "uefi")))]
        let _ = error;
    }
}

/// # Safety
/// BSP only; `system` is the live System Table.
unsafe fn register_exit_handler(system: *mut SystemTable) -> Result<(), Status> {
    // SAFETY: the caller passes the live System Table and its Boot Services.
    let boot = unsafe { &*(*system).boot_services };
    let mut event = ptr::null_mut();
    // SAFETY: `event` is a writable local; the handler takes no context.
    let status = unsafe {
        (boot.create_event)(
            EVT_SIGNAL_EXIT_BOOT_SERVICES,
            TPL_NOTIFY,
            Some(exit_boot_services),
            ptr::null_mut(),
            &mut event,
        )
    };
    if status == Status::SUCCESS {
        Ok(())
    } else {
        Err(status)
    }
}

/// Failure path: stop the controller (when it was started), return the DMA pages only if it
/// stopped referencing them, and restore PCI Command.
fn release(ecam: &pci::Ecam, device: u8, saved: u32, controller: Option<&Xhci>, dma: u64) {
    use pci::ConfigSpace;
    let stopped = controller.is_none_or(Xhci::stop);
    if stopped {
        // SAFETY: the controller is in the reset state and holds no pointer into the pages.
        let _ = unsafe { crate::bs::memory::free_pages(dma, xhci::DMA_PAGES) };
    }
    ecam.write(device, 0, COMMAND_REGISTER, saved);
}

/// Result of [`install`]. `repr(u8)`: an explicit tag, not a niche in `Step` (the compiler turned
/// the niche test into `sub $imm8, r8` (`80 /5`), which the firmware CPU profile does not run).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Outcome {
    /// The board has no xHCI function: USB is absent, nothing is reported.
    Absent,
    /// The keyboard is enumerated and its Interrupt IN endpoint is armed.
    Enumerated,
    /// A bring-up step failed. The controller was stopped (R/S = 0, HCRST) and PCI Command
    /// restored; its DMA pages were returned only if the stop succeeded, otherwise they stay
    /// allocated (BootServicesData) because the controller may still reference them.
    Failed(UsbError),
}

impl Outcome {
    /// The COM1 diagnostic line (`OD-FW xhci: keyboard enumerated` or the exact failing step
    /// and detail), empty for [`Outcome::Absent`] so that a board without the function keeps
    /// its COM1 transcript byte for byte.
    pub fn line<'a>(&self, buffer: &'a mut [u8; 128]) -> &'a [u8] {
        match *self {
            Self::Absent => &[],
            Self::Enumerated => ENUMERATED_LINE,
            Self::Failed(error) => {
                let length = format_failure(error, buffer);
                &buffer[..length]
            }
        }
    }
}

/// Brings up the board's USB keyboard when the board has an xHCI controller. A failure is not
/// fatal to the boot (the keyboard is an additional input source); the caller must report it,
/// never hide it.
///
/// # Safety
/// BSP only, after Boot Services are initialized (`system` is the live System Table) and
/// before any use of the USB key queue.
pub unsafe fn install(handoff: &Handoff<'_>, system: *mut SystemTable) -> Outcome {
    // SAFETY: forwarded.
    match unsafe { start(handoff) } {
        Ok(None) => Outcome::Absent,
        Ok(Some(mut controller)) => {
            // SAFETY: forwarded.
            if let Err(status) = unsafe { register_exit_handler(system) } {
                // Without the handler the controller could not be stopped at ExitBootServices:
                // stop it now and return the pages if it reached the reset state.
                if controller.quiesce().is_ok() {
                    // SAFETY: stopped above; the driver is dropped here.
                    unsafe { controller.free_dma() };
                }
                return Outcome::Failed(UsbError::new(Step::ExitHandler, status.raw() as u32));
            }
            // SAFETY: BSP, single writer; nothing else reads `DRIVER` yet.
            unsafe { *ptr::addr_of_mut!(DRIVER) = Some(controller) };
            Outcome::Enumerated
        }
        Err(error) => Outcome::Failed(error),
    }
}

/// Polls the keyboard's event ring and returns the next decoded key (slice B hooks this into
/// `ConIn`). `None` without a keyboard or without a pending key.
///
/// # Safety
/// BSP only.
#[allow(dead_code)]
pub(crate) unsafe fn next_key() -> Option<InputKey> {
    // SAFETY: BSP, no other reference to `DRIVER` is live.
    let driver = unsafe { (*ptr::addr_of_mut!(DRIVER)).as_mut()? };
    driver.poll();
    driver.decoder.pop()
}

const _: () = assert!(PAGE_BYTES == 4096);

#[cfg(test)]
mod tests {
    use super::*;

    fn line(error: UsbError) -> String {
        let mut out = [0u8; 128];
        let length = format_failure(error, &mut out);
        String::from_utf8(out[..length].to_vec()).unwrap()
    }

    #[test]
    fn failures_name_the_step_and_the_detail_word() {
        assert_eq!(
            line(UsbError::new(Step::AddressDevice, 0x13)),
            "OD-FW xhci: enumeration failed step=address-device detail=0x00000013\r\n"
        );
        assert_eq!(
            line(UsbError::new(Step::ConfigurationDescriptor, 0xffff_fffe)),
            "OD-FW xhci: enumeration failed step=configuration-descriptor detail=0xFFFFFFFE\r\n"
        );
    }

    #[test]
    fn outcome_lines_are_empty_for_absent_boards_and_exact_otherwise() {
        let mut buffer = [0u8; 128];
        assert_eq!(Outcome::Absent.line(&mut buffer), b"");
        assert_eq!(
            Outcome::Enumerated.line(&mut buffer),
            b"OD-FW xhci: keyboard enumerated\r\n"
        );
        assert_eq!(
            Outcome::Failed(UsbError::new(Step::PortReset, 0x1)).line(&mut buffer),
            b"OD-FW xhci: enumeration failed step=port-reset detail=0x00000001\r\n"
        );
    }

    #[test]
    fn every_step_has_a_distinct_name_that_fits_the_line_buffer() {
        let steps = [
            Step::Ecam,
            Step::Bar,
            Step::Allocate,
            Step::Capabilities,
            Step::ControllerReset,
            Step::Run,
            Step::PortReset,
            Step::EnableSlot,
            Step::AddressDevice,
            Step::DeviceDescriptor,
            Step::ConfigurationDescriptor,
            Step::DisableSlot,
            Step::NoKeyboard,
            Step::SetConfiguration,
            Step::ConfigureEndpoint,
            Step::SetProtocol,
            Step::SetIdle,
            Step::SetReport,
            Step::ExitHandler,
        ];
        let mut names: Vec<&str> = steps.iter().map(|step| step.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), steps.len());
        for step in steps {
            assert!(line(UsbError::new(step, 0)).len() < 128);
        }
    }
}
