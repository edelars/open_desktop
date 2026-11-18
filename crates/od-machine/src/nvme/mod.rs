//! Minimal NVMe controller as an MMIO device (S7-16, slice A: admin command
//! set; slice B: I/O command set over a disk backend). Contract: `docs/specs/NVME_DEVICE.md`.

mod admin;
mod backend;
mod controller;
mod io;
mod queues;
#[cfg(test)]
mod tests;

pub use backend::{BackendError, InMemoryBackend, NvmeBackend};
pub use controller::{
    NvmeAttachError, NvmeConfig, NvmeConfigError, NvmeDevice, NvmeHandle, NvmeServiceReport,
};

/// Fixed MMIO window of the controller until the PCI binding (S4-30):
/// registers `0x0000..0x1000`, doorbells from `0x1000` (stride 4 bytes).
pub const NVME_MMIO_BASE: u64 = 0xFC00_1000;
/// Window length: the register block plus the doorbells of the admin queue
/// and four I/O queue pairs, rounded to two pages.
pub const NVME_MMIO_BYTES: u64 = 0x2000;

/// The Linux PCI profile reserves one aligned, non-prefetchable BAR0/1 aperture.
/// Fixed-MMIO firmware uses the separate handoff address above.
pub(crate) const NVME_PCI_BAR_BASE: u64 = 0xFC00_4000;
pub(crate) const NVME_PCI_SLOT: u8 = 4;

pub(crate) mod irq_source;
