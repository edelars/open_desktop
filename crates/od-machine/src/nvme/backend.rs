//! Disk backend interface for the I/O command set (slice B).
//!
//! `od-machine` does not read files: the machine owner supplies an
//! implementation over the `od-storage` contracts (`STORAGE_FORMAT.md`).
//! Slice A defines the interface only; no controller code calls it yet and
//! `od-machine` provides no implementation.

use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendError {
    /// The sector range lies outside the namespace.
    OutOfRange,
    /// The backing store failed.
    Io,
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NVMe backend error: {self:?}")
    }
}
impl std::error::Error for BackendError {}

/// A namespace of 512-byte sectors.
pub trait NvmeBackend {
    fn sector_count(&self) -> u64;
    fn read_sectors(&mut self, lba: u64, buffer: &mut [u8]) -> Result<(), BackendError>;
    fn write_sectors(&mut self, lba: u64, data: &[u8]) -> Result<(), BackendError>;
    fn flush(&mut self) -> Result<(), BackendError>;
}
