//! xHCI controller model for the board's PCI function 00:05.0 (S8-42, slice A:
//! registers, ports, Enable/Disable Slot, No Op, command and event rings).
//! Contract: `docs/specs/XHCI_DEVICE.md`.

mod commands;
mod controller;
mod ports;
mod regs;
mod ring;
#[cfg(test)]
mod tests;
mod trb;

pub use controller::{
    HseCause, XhciConfig, XhciDevice, XhciHandle, XhciPciFunction, XhciServiceReport,
};

/// BAR0 of PCI function 00:05.0: 64-bit non-prefetchable memory, 16 KiB, programmed at board
/// creation (contract section 2).
pub const XHCI_PCI_BAR_BASE: u64 = 0xFC00_8000;
pub const XHCI_PCI_BAR_BYTES: u64 = 0x4000;
/// PCI device number of the controller on bus 0 (function 0).
pub const XHCI_PCI_SLOT: u8 = 5;
/// Fixed route of INTA to the IO-APIC (the NVMe function uses GSI 16).
pub const XHCI_INTX_GSI: u32 = 17;
/// Duration of a root port reset: USB 2.0 7.1.7.5, TDRSTR of a root port is at least 50 ms.
pub const XHCI_USB_PORT_RESET_NS: u64 = 50_000_000;
