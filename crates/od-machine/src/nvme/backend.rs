//! Disk backend interface for the I/O command set (S7-16, slice B).
//!
//! `od-machine` does not read files: the machine owner supplies an
//! implementation over the `od-storage` contracts (`STORAGE_FORMAT.md`) and
//! attaches it with `NvmeHandle::attach_backend`. This module defines the
//! trait (the integration point) and [`InMemoryBackend`], a fixed-size vector
//! of bytes for tests and mini images. No file or CoW backend lives here
//! (`docs/specs/NVME_DEVICE.md`, "Slice B").

use std::fmt;

/// Size of one sector (the only LBA format, `LBADS = 9`).
pub const SECTOR_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendError {
    /// The sector range lies outside the namespace, or the buffer is not a
    /// whole number of sectors.
    OutOfRange,
    /// The backing store failed.
    Io,
    /// The requested size is zero or cannot be allocated.
    InvalidSize,
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NVMe backend error: {self:?}")
    }
}
impl std::error::Error for BackendError {}

/// A namespace of 512-byte sectors. The controller checks the LBA range
/// against `sector_count` before it calls the other methods, but an
/// implementation must still reject an out-of-range access with
/// [`BackendError::OutOfRange`] instead of panicking.
///
/// Durability requirement: the controller reports `VWC = 0` (no volatile
/// write cache), so guest drivers send no `Flush` and guest data must be
/// durable by the time `write_sectors` returns (write-through). Either the
/// backend guarantees that, or the integration must declare `VWC = 1` and
/// implement `Flush`. The choice is an open question for the `od-storage`
/// integration.
pub trait NvmeBackend {
    fn sector_count(&self) -> u64;
    /// Fills `buffer` (a whole number of sectors) from `lba` on.
    fn read_sectors(&mut self, lba: u64, buffer: &mut [u8]) -> Result<(), BackendError>;
    /// Stores `data` (a whole number of sectors) from `lba` on.
    fn write_sectors(&mut self, lba: u64, data: &[u8]) -> Result<(), BackendError>;
    /// Makes every completed write durable.
    fn flush(&mut self) -> Result<(), BackendError>;
}

/// Byte range of `buffer_len` bytes at sector `lba`, checked against `sectors`.
fn byte_range(sectors: u64, lba: u64, buffer_len: usize) -> Result<(usize, usize), BackendError> {
    if buffer_len == 0 || !buffer_len.is_multiple_of(SECTOR_BYTES) {
        return Err(BackendError::OutOfRange);
    }
    let count = (buffer_len / SECTOR_BYTES) as u64;
    let end = lba.checked_add(count).ok_or(BackendError::OutOfRange)?;
    if end > sectors {
        return Err(BackendError::OutOfRange);
    }
    let start = usize::try_from(lba).map_err(|_| BackendError::OutOfRange)? * SECTOR_BYTES;
    Ok((start, start + buffer_len))
}

/// A namespace held in RAM: a zero-filled vector of fixed size.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InMemoryBackend {
    bytes: Vec<u8>,
    flushes: u64,
}

impl InMemoryBackend {
    /// A zero-filled namespace of `sectors` sectors; zero or an unallocatable
    /// size is [`BackendError::InvalidSize`].
    pub fn new(sectors: u64) -> Result<Self, BackendError> {
        if sectors == 0 {
            return Err(BackendError::InvalidSize);
        }
        let len = usize::try_from(sectors)
            .ok()
            .and_then(|count| count.checked_mul(SECTOR_BYTES))
            .ok_or(BackendError::InvalidSize)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(len)
            .map_err(|_| BackendError::InvalidSize)?;
        bytes.resize(len, 0);
        Ok(Self { bytes, flushes: 0 })
    }

    /// A namespace holding `bytes`; empty or not a whole number of sectors is
    /// [`BackendError::InvalidSize`].
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, BackendError> {
        if bytes.is_empty() || !bytes.len().is_multiple_of(SECTOR_BYTES) {
            return Err(BackendError::InvalidSize);
        }
        Ok(Self { bytes, flushes: 0 })
    }

    /// The whole namespace content.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Number of `flush` calls so far.
    pub const fn flush_count(&self) -> u64 {
        self.flushes
    }
}

impl NvmeBackend for InMemoryBackend {
    fn sector_count(&self) -> u64 {
        (self.bytes.len() / SECTOR_BYTES) as u64
    }

    fn read_sectors(&mut self, lba: u64, buffer: &mut [u8]) -> Result<(), BackendError> {
        let (start, end) = byte_range(self.sector_count(), lba, buffer.len())?;
        buffer.copy_from_slice(&self.bytes[start..end]);
        Ok(())
    }

    fn write_sectors(&mut self, lba: u64, data: &[u8]) -> Result<(), BackendError> {
        let (start, end) = byte_range(self.sector_count(), lba, data.len())?;
        self.bytes[start..end].copy_from_slice(data);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), BackendError> {
        self.flushes += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_validated() {
        assert_eq!(InMemoryBackend::new(0), Err(BackendError::InvalidSize));
        assert_eq!(
            InMemoryBackend::from_bytes(vec![0; 513]),
            Err(BackendError::InvalidSize)
        );
        assert_eq!(
            InMemoryBackend::from_bytes(Vec::new()),
            Err(BackendError::InvalidSize)
        );
        assert_eq!(
            InMemoryBackend::new(u64::MAX),
            Err(BackendError::InvalidSize)
        );
        assert_eq!(InMemoryBackend::new(8).unwrap().sector_count(), 8);
    }

    #[test]
    fn write_then_read_round_trips_and_ranges_are_checked() {
        let mut backend = InMemoryBackend::new(4).unwrap();
        let data: Vec<u8> = (0..1024).map(|index| index as u8).collect();
        backend.write_sectors(2, &data).unwrap();
        let mut back = vec![0u8; 1024];
        backend.read_sectors(2, &mut back).unwrap();
        assert_eq!(back, data);
        assert_eq!(&backend.as_bytes()[..1024], &[0u8; 1024][..]);
        let mut one = [0u8; 512];
        assert_eq!(
            backend.read_sectors(4, &mut one),
            Err(BackendError::OutOfRange)
        );
        assert_eq!(
            backend.write_sectors(3, &data),
            Err(BackendError::OutOfRange)
        );
        assert_eq!(
            backend.read_sectors(u64::MAX, &mut one),
            Err(BackendError::OutOfRange)
        );
        let mut odd = [0u8; 100];
        assert_eq!(
            backend.read_sectors(0, &mut odd),
            Err(BackendError::OutOfRange)
        );
        backend.flush().unwrap();
        assert_eq!(backend.flush_count(), 1);
    }
}
