//! RAW disk image backend: a headerless image whose bytes are stored 1:1 in
//! the backing object, so the logical size equals the object size.
//!
//! [`RawImage`] sits on top of [`CheckedStorage`] and adds only what RAW
//! itself defines:
//!
//! * an explicit logical size, fixed when the image is created or opened (the
//!   image never grows or shrinks; there is no resize here);
//! * a declared `block_size` (power of two) that the logical size must be a
//!   multiple of;
//! * byte-granular access ([`RawImage::read_at`], [`RawImage::write_at`]):
//!   RAW permits unaligned ranges anywhere inside the logical size;
//! * a separate block-aligned mode ([`RawImage::read_aligned`],
//!   [`RawImage::write_aligned`]) for formats that need whole blocks.
//!
//! Failure classes are kept apart. A range outside the logical size is
//! [`RawError::OutOfRange`] (or [`RawError::OffsetOverflow`]) and is decided
//! before the backend is touched. A range inside the logical size that the
//! backing object cannot serve because it is shorter than promised is
//! [`RawError::IncompleteImage`]. Zero bytes are never substituted for either.
//!
//! This module is path-free: creating a file that must not already exist and
//! opening it for exclusive writing are host operations (`od-host`
//! `HostFileStorage::create_new_exclusive` / `open_read_write_exclusive`).
//! [`RawImage::create`] and [`RawImage::open_read_write`] accept only a
//! backend that declares the exclusive writer lock, so a RAW image can never
//! be written through an unlocked handle. A safe refusal to create over an
//! existing file therefore happens in the host constructor, before any byte of
//! the existing file is touched, and `RawImage` has no truncate path at all.
//!
//! Durability: a successful write is completion only. `RawImage` itself offers
//! no flush. A caller that needs a durable acknowledgement takes the checked
//! storage back ([`RawImage::into_storage`]), flushes it, and rebuilds the view
//! ([`RawImage::reopen_read_write_checked`]); the host NVMe backend
//! (`od-host` `persistent_nvme`) does this after every guest write. Whether the
//! backend can flush is declared by its capabilities (the macOS host file
//! backend declares file-data durability); power-loss behaviour is not
//! claimed here.

use crate::io::{CheckedStorage, StorageBackend, StorageCapabilities, StorageError};

/// Failure of a RAW image operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RawError {
    /// Logical size zero is not a valid image.
    ZeroSize,
    /// `block_size` is zero or not a power of two.
    InvalidBlockSize { block_size: u64 },
    /// Logical size is not a multiple of the block size.
    SizeNotBlockMultiple { size: u64, block_size: u64 },
    /// A writable open or create was given a backend that cannot write.
    NotWritable,
    /// A read-only open was given a backend that can write; pass a backend
    /// opened read-only so that the image cannot be modified by accident.
    NotReadOnly,
    /// A writable image requires a backend holding the exclusive writer lock.
    ExclusiveLockRequired,
    /// The object is smaller than the expected logical size. Never padded
    /// with zeros.
    Truncated { expected: u64, actual: u64 },
    /// The object size differs from the expected logical size (and is not
    /// smaller): the image would silently ignore trailing bytes.
    SizeMismatch { expected: u64, actual: u64 },
    /// `offset + len` does not fit in `u64`.
    OffsetOverflow { offset: u64, len: u64 },
    /// The range is not inside the logical size.
    OutOfRange { offset: u64, len: u64, size: u64 },
    /// Aligned mode: offset or length is not a multiple of the block size.
    Unaligned {
        offset: u64,
        len: u64,
        block_size: u64,
    },
    /// The range is inside the logical size but the backing object ended
    /// before it: the object is shorter than the image claims.
    IncompleteImage {
        offset: u64,
        len: u64,
        logical_size: u64,
    },
    /// Any other storage failure, unchanged.
    Storage(StorageError),
}

impl From<StorageError> for RawError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl std::fmt::Display for RawError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "raw image error: {self:?}")
    }
}

impl std::error::Error for RawError {}

/// A RAW image over one checked backend. See the module documentation.
pub struct RawImage<B: StorageBackend> {
    storage: CheckedStorage<B>,
    logical_size: u64,
    block_size: u64,
}

impl<B: StorageBackend> RawImage<B> {
    /// Wraps a backend of exactly `logical_size` bytes that holds the
    /// exclusive writer lock as a new image. This constructor checks only
    /// writability, the lock and the size; it does not check that the object
    /// is new or zero-filled, and nothing is written or truncated here. The
    /// zero contents are guaranteed only by a caller that obtained the backend
    /// from a create-new constructor that refuses an existing object (for
    /// example `HostFileStorage::create_new_exclusive(path, logical_size)`);
    /// they are not re-read.
    pub fn create(backend: B, logical_size: u64, block_size: u64) -> Result<Self, RawError> {
        check_geometry(logical_size, block_size)?;
        let storage = CheckedStorage::new(backend)?;
        require_writer(&storage.capabilities())?;
        if logical_size > storage.capabilities().max_len {
            return Err(StorageError::ExceedsBackendLimit {
                requested: logical_size,
                max: storage.capabilities().max_len,
            }
            .into());
        }
        let actual = storage.size()?;
        if actual != logical_size {
            return Err(size_mismatch(logical_size, actual));
        }
        Ok(Self {
            storage,
            logical_size,
            block_size,
        })
    }

    /// Opens an existing image for reading and writing. The logical size is
    /// the backend size; when `expected_size` is given (from metadata) a
    /// smaller object is [`RawError::Truncated`] and a larger one
    /// [`RawError::SizeMismatch`]. The backend must hold the exclusive lock.
    pub fn open_read_write(
        backend: B,
        expected_size: Option<u64>,
        block_size: u64,
    ) -> Result<Self, RawError> {
        let storage = CheckedStorage::new(backend)?;
        require_writer(&storage.capabilities())?;
        Self::open_checked(storage, expected_size, block_size)
    }

    /// Revalidates an existing checked writer as a RAW image without
    /// consuming its storage on failure. This is intended for operations that
    /// consume a `RawImage` to request a durable flush and then rebuild the
    /// view over the same locked backend. On validation or size errors the
    /// original `CheckedStorage` is returned so its writer lock and sticky
    /// reconcile state cannot be dropped accidentally.
    pub fn reopen_read_write_checked(
        storage: CheckedStorage<B>,
        expected_size: Option<u64>,
        block_size: u64,
    ) -> Result<Self, (CheckedStorage<B>, RawError)> {
        if let Err(error) = require_writer(&storage.capabilities()) {
            return Err((storage, error));
        }
        Self::open_checked_retaining(storage, expected_size, block_size)
    }

    /// Opens an existing image for reading only. The backend must be
    /// read-only; see [`RawImage::open_read_write`] for the size rules.
    pub fn open_read_only(
        backend: B,
        expected_size: Option<u64>,
        block_size: u64,
    ) -> Result<Self, RawError> {
        let storage = CheckedStorage::new(backend)?;
        if storage.capabilities().writable {
            return Err(RawError::NotReadOnly);
        }
        Self::open_checked(storage, expected_size, block_size)
    }

    fn open_checked(
        storage: CheckedStorage<B>,
        expected_size: Option<u64>,
        block_size: u64,
    ) -> Result<Self, RawError> {
        match Self::open_checked_retaining(storage, expected_size, block_size) {
            Ok(image) => Ok(image),
            Err((_storage, error)) => Err(error),
        }
    }

    fn open_checked_retaining(
        storage: CheckedStorage<B>,
        expected_size: Option<u64>,
        block_size: u64,
    ) -> Result<Self, (CheckedStorage<B>, RawError)> {
        let actual = match storage.size() {
            Ok(actual) => actual,
            Err(error) => return Err((storage, error.into())),
        };
        if let Some(expected) = expected_size
            && actual != expected
        {
            return Err((storage, size_mismatch(expected, actual)));
        }
        if let Err(error) = check_geometry(actual, block_size) {
            return Err((storage, error));
        }
        Ok(Self {
            storage,
            logical_size: actual,
            block_size,
        })
    }

    /// Image size in bytes, fixed for the life of this value.
    pub fn logical_size(&self) -> u64 {
        self.logical_size
    }

    pub fn block_size(&self) -> u64 {
        self.block_size
    }

    /// Number of whole blocks in the image.
    pub fn block_count(&self) -> u64 {
        self.logical_size / self.block_size
    }

    /// Backend capabilities. A RAW image declares no durability.
    pub fn capabilities(&self) -> StorageCapabilities {
        self.storage.capabilities()
    }

    /// Consumes the image and returns the checked storage (for example to ask
    /// for a durable flush and observe that it is unsupported).
    pub fn into_storage(self) -> CheckedStorage<B> {
        self.storage
    }

    /// Fills `buf` from `offset`; any byte range inside the image is allowed.
    /// On `Err` the contents of `buf` are unspecified.
    pub fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), RawError> {
        let len = buf.len() as u64;
        self.check_range(offset, len)?;
        self.storage
            .read_exact_at(offset, buf)
            .map_err(|e| self.map_transfer_error(e, offset, len))
    }

    /// Writes all of `buf` at `offset` inside the image (unaligned allowed).
    /// Success is write completion, not durability.
    pub fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), RawError> {
        self.check_range(offset, buf.len() as u64)?;
        let len = buf.len() as u64;
        let logical_size = self.logical_size;
        self.storage
            .write_all_at(offset, buf)
            .map_err(|e| map_transfer_error(e, offset, len, logical_size))
    }

    /// Like [`RawImage::read_at`] but offset and length must be multiples of
    /// the block size.
    pub fn read_aligned(&self, offset: u64, buf: &mut [u8]) -> Result<(), RawError> {
        self.check_aligned(offset, buf.len() as u64)?;
        self.read_at(offset, buf)
    }

    /// Like [`RawImage::write_at`] but offset and length must be multiples of
    /// the block size.
    pub fn write_aligned(&mut self, offset: u64, buf: &[u8]) -> Result<(), RawError> {
        self.check_aligned(offset, buf.len() as u64)?;
        self.write_at(offset, buf)
    }

    fn check_range(&self, offset: u64, len: u64) -> Result<(), RawError> {
        let end = offset
            .checked_add(len)
            .ok_or(RawError::OffsetOverflow { offset, len })?;
        if end > self.logical_size {
            return Err(RawError::OutOfRange {
                offset,
                len,
                size: self.logical_size,
            });
        }
        Ok(())
    }

    fn check_aligned(&self, offset: u64, len: u64) -> Result<(), RawError> {
        if !offset.is_multiple_of(self.block_size) || !len.is_multiple_of(self.block_size) {
            return Err(RawError::Unaligned {
                offset,
                len,
                block_size: self.block_size,
            });
        }
        Ok(())
    }

    fn map_transfer_error(&self, error: StorageError, offset: u64, len: u64) -> RawError {
        map_transfer_error(error, offset, len, self.logical_size)
    }
}

/// The range was already proven inside the logical size, so a backend that
/// still reports it out of range or ends early is shorter than the image.
fn map_transfer_error(error: StorageError, offset: u64, len: u64, logical_size: u64) -> RawError {
    match error {
        StorageError::OutOfRange { .. } | StorageError::ShortRead { .. } => {
            RawError::IncompleteImage {
                offset,
                len,
                logical_size,
            }
        }
        other => RawError::Storage(other),
    }
}

fn check_geometry(size: u64, block_size: u64) -> Result<(), RawError> {
    if block_size == 0 || !block_size.is_power_of_two() {
        return Err(RawError::InvalidBlockSize { block_size });
    }
    if size == 0 {
        return Err(RawError::ZeroSize);
    }
    if !size.is_multiple_of(block_size) {
        return Err(RawError::SizeNotBlockMultiple { size, block_size });
    }
    Ok(())
}

fn require_writer(caps: &StorageCapabilities) -> Result<(), RawError> {
    if !caps.writable {
        return Err(RawError::NotWritable);
    }
    if !caps.exclusive_lock {
        return Err(RawError::ExclusiveLockRequired);
    }
    Ok(())
}

fn size_mismatch(expected: u64, actual: u64) -> RawError {
    if actual < expected {
        RawError::Truncated { expected, actual }
    } else {
        RawError::SizeMismatch { expected, actual }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{DurabilityScope, StorageCapabilities};
    use crate::testing::memory::{MEMORY_MAX_LEN, MemoryStorage};

    const BS: u64 = 512;
    const SIZE: usize = 4096;

    fn locked_caps() -> StorageCapabilities {
        StorageCapabilities::read_write(MEMORY_MAX_LEN).with_exclusive_lock()
    }

    fn writer(initial: &[u8]) -> MemoryStorage {
        MemoryStorage::with_capabilities(initial, locked_caps())
    }

    fn new_image() -> RawImage<MemoryStorage> {
        RawImage::create(writer(&[0; SIZE]), SIZE as u64, BS).unwrap()
    }

    fn pattern(len: usize, seed: u8) -> Vec<u8> {
        (0..len)
            .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
            .collect()
    }

    fn read_all(image: &RawImage<MemoryStorage>) -> Vec<u8> {
        let mut out = vec![0xEE; image.logical_size() as usize];
        image.read_at(0, &mut out).unwrap();
        out
    }

    #[test]
    fn create_reports_geometry_and_reads_zeros() {
        let image = new_image();
        assert_eq!(image.logical_size(), SIZE as u64);
        assert_eq!(image.block_size(), BS);
        assert_eq!(image.block_count(), 8);
        assert_eq!(read_all(&image), vec![0u8; SIZE]);
    }

    #[test]
    fn start_end_middle_and_unaligned_writes_match_reference() {
        let mut image = new_image();
        let mut reference = vec![0u8; SIZE];
        // (offset, len): start, end, middle, unaligned inside a block, a
        // range straddling a block boundary, single bytes at both edges.
        for (n, (offset, len)) in [
            (0usize, 16usize),
            (SIZE - 16, 16),
            (1000, 700),
            (3, 5),
            (509, 6),
            (0, 1),
            (SIZE - 1, 1),
        ]
        .into_iter()
        .enumerate()
        {
            let data = pattern(len, n as u8 + 1);
            image.write_at(offset as u64, &data).unwrap();
            reference[offset..offset + len].copy_from_slice(&data);
            let mut back = vec![0u8; len];
            image.read_at(offset as u64, &mut back).unwrap();
            assert_eq!(back, data, "range {offset}+{len}");
        }
        assert_eq!(read_all(&image), reference);
    }

    #[test]
    fn range_check_at_boundary_and_overflow() {
        let mut image = new_image();
        let size = SIZE as u64;
        let mut buf = [0u8; 4];
        // offset + len == size is allowed; one byte further is not.
        image.read_at(size - 4, &mut buf).unwrap();
        image.write_at(size - 4, &buf).unwrap();
        assert_eq!(
            image.read_at(size - 3, &mut buf),
            Err(RawError::OutOfRange {
                offset: size - 3,
                len: 4,
                size
            })
        );
        assert_eq!(
            image.write_at(size - 3, &buf),
            Err(RawError::OutOfRange {
                offset: size - 3,
                len: 4,
                size
            })
        );
        // Zero-length access at the end is inside; past the end is not.
        image.read_at(size, &mut []).unwrap();
        assert!(matches!(
            image.read_at(size + 1, &mut []),
            Err(RawError::OutOfRange { .. })
        ));
        assert_eq!(
            image.read_at(u64::MAX, &mut buf),
            Err(RawError::OffsetOverflow {
                offset: u64::MAX,
                len: 4
            })
        );
        assert_eq!(
            image.write_at(u64::MAX - 1, &buf),
            Err(RawError::OffsetOverflow {
                offset: u64::MAX - 1,
                len: 4
            })
        );
        // Writing past the end does not grow the image or alter any byte.
        assert_eq!(image.logical_size(), size);
        assert_eq!(read_all(&image), vec![0u8; SIZE]);
    }

    #[test]
    fn aligned_mode_requires_whole_blocks_unaligned_mode_does_not() {
        let mut image = new_image();
        let block = pattern(BS as usize, 7);
        image.write_aligned(BS, &block).unwrap();
        image.write_aligned(7 * BS, &block).unwrap();
        let mut two = vec![0u8; 2 * BS as usize];
        image.read_aligned(0, &mut two).unwrap();
        assert_eq!(&two[BS as usize..], &block[..]);
        for (offset, len) in [(1u64, BS), (BS, BS - 1), (BS + 1, 2 * BS), (0, 1)] {
            let buf = vec![0u8; len as usize];
            let expected = RawError::Unaligned {
                offset,
                len,
                block_size: BS,
            };
            assert_eq!(image.write_aligned(offset, &buf), Err(expected));
            let mut out = vec![0u8; len as usize];
            assert_eq!(image.read_aligned(offset, &mut out), Err(expected));
            // The same unaligned range is fine in byte mode.
            image.write_at(offset, &buf).unwrap();
            image.read_at(offset, &mut out).unwrap();
        }
        // Aligned but out of range is still OutOfRange, not Unaligned.
        let mut out = vec![0u8; BS as usize];
        assert!(matches!(
            image.read_aligned(SIZE as u64, &mut out),
            Err(RawError::OutOfRange { .. })
        ));
    }

    #[test]
    fn rejects_bad_geometry() {
        for block_size in [0u64, 3, 1000] {
            assert_eq!(
                RawImage::create(writer(&[0; SIZE]), SIZE as u64, block_size).err(),
                Some(RawError::InvalidBlockSize { block_size })
            );
        }
        assert_eq!(
            RawImage::create(writer(&[]), 0, BS).err(),
            Some(RawError::ZeroSize)
        );
        assert_eq!(
            RawImage::create(writer(&[0; 1000]), 1000, BS).err(),
            Some(RawError::SizeNotBlockMultiple {
                size: 1000,
                block_size: BS
            })
        );
        assert_eq!(
            RawImage::open_read_write(writer(&[0; 1000]), None, BS).err(),
            Some(RawError::SizeNotBlockMultiple {
                size: 1000,
                block_size: BS
            })
        );
        // Byte-granular images are still expressible with block_size 1.
        let mut image = RawImage::create(writer(&[0; 1000]), 1000, 1).unwrap();
        image.write_aligned(3, &[1, 2, 3]).unwrap();
    }

    #[test]
    fn create_requires_exact_fresh_writer() {
        // Backend size must equal the requested logical size.
        assert_eq!(
            RawImage::create(writer(&[0; SIZE]), 2 * SIZE as u64, BS).err(),
            Some(RawError::Truncated {
                expected: 2 * SIZE as u64,
                actual: SIZE as u64
            })
        );
        assert_eq!(
            RawImage::create(writer(&[0; 2 * SIZE]), SIZE as u64, BS).err(),
            Some(RawError::SizeMismatch {
                expected: SIZE as u64,
                actual: 2 * SIZE as u64
            })
        );
        // Writable but not locked, and read-only, are refused.
        let unlocked = MemoryStorage::new(&[0; SIZE]);
        assert_eq!(
            RawImage::create(unlocked, SIZE as u64, BS).err(),
            Some(RawError::ExclusiveLockRequired)
        );
        let read_only = MemoryStorage::new_read_only(&[0; SIZE]);
        assert_eq!(
            RawImage::create(read_only, SIZE as u64, BS).err(),
            Some(RawError::NotWritable)
        );
        // Larger than the backend can represent.
        let small = MemoryStorage::with_capabilities(
            &[0; SIZE],
            StorageCapabilities::read_write(1024).with_exclusive_lock(),
        );
        assert!(matches!(
            RawImage::create(small, SIZE as u64, BS).err(),
            Some(RawError::Storage(StorageError::ExceedsBackendLimit { .. }))
        ));
    }

    #[test]
    fn open_checks_expected_size_without_padding() {
        let data = pattern(SIZE, 9);
        let image = RawImage::open_read_write(writer(&data), Some(SIZE as u64), BS).unwrap();
        assert_eq!(read_all(&image), data);
        // Smaller than metadata promises: explicit error, no zero padding.
        assert_eq!(
            RawImage::open_read_write(writer(&data[..SIZE - 512]), Some(SIZE as u64), BS).err(),
            Some(RawError::Truncated {
                expected: SIZE as u64,
                actual: SIZE as u64 - 512
            })
        );
        assert_eq!(
            RawImage::open_read_only(
                MemoryStorage::new_read_only(&data[..SIZE - 512]),
                Some(SIZE as u64),
                BS
            )
            .err(),
            Some(RawError::Truncated {
                expected: SIZE as u64,
                actual: SIZE as u64 - 512
            })
        );
        // Larger than metadata promises: also explicit.
        let mut longer = data.clone();
        longer.extend_from_slice(&[0; 512]);
        assert_eq!(
            RawImage::open_read_write(writer(&longer), Some(SIZE as u64), BS).err(),
            Some(RawError::SizeMismatch {
                expected: SIZE as u64,
                actual: SIZE as u64 + 512
            })
        );
        // Without metadata the size is the object size.
        let image = RawImage::open_read_write(writer(&longer), None, BS).unwrap();
        assert_eq!(image.logical_size(), SIZE as u64 + 512);
    }

    #[test]
    fn checked_reopen_returns_the_same_storage_when_size_validation_fails() {
        let checked = CheckedStorage::new(writer(&[0; SIZE])).unwrap();
        let (checked, error) = match RawImage::reopen_read_write_checked(
            checked,
            Some((SIZE + BS as usize) as u64),
            BS,
        ) {
            Ok(_) => panic!("mismatched size unexpectedly reopened"),
            Err(owner) => owner,
        };
        assert_eq!(
            error,
            RawError::Truncated {
                expected: (SIZE + BS as usize) as u64,
                actual: SIZE as u64,
            }
        );
        assert_eq!(checked.size(), Ok(SIZE as u64));
        assert!(checked.into_backend().is_ok());
    }

    #[test]
    fn checked_reopen_failure_does_not_clear_sticky_reconcile_state() {
        use crate::testing::fault::{Fault, FaultHandle, FlushFault};

        let handle = FaultHandle::new(&[0; SIZE], 0x503);
        let mut checked = CheckedStorage::new(handle.open()).unwrap();
        handle.script_next(Fault::Flush(FlushFault::IoNotDurable));
        assert!(matches!(
            checked.flush_file_durable(),
            Err(StorageError::Io {
                op: crate::io::IoOp::FlushFile,
                ..
            })
        ));

        let (mut checked, error) = match RawImage::reopen_read_write_checked(
            checked,
            Some((SIZE + BS as usize) as u64),
            BS,
        ) {
            Ok(_) => panic!("mismatched size unexpectedly reopened"),
            Err(owner) => owner,
        };
        assert!(matches!(error, RawError::Truncated { .. }));
        assert_eq!(
            checked.flush_file_durable(),
            Err(StorageError::ReconcileRequired)
        );
        assert!(checked.into_backend().is_err());
    }

    #[test]
    fn open_modes_demand_matching_backend() {
        assert_eq!(
            RawImage::open_read_write(MemoryStorage::new(&[0; SIZE]), None, BS).err(),
            Some(RawError::ExclusiveLockRequired)
        );
        assert_eq!(
            RawImage::open_read_write(MemoryStorage::new_read_only(&[0; SIZE]), None, BS).err(),
            Some(RawError::NotWritable)
        );
        assert_eq!(
            RawImage::open_read_only(writer(&[0; SIZE]), None, BS).err(),
            Some(RawError::NotReadOnly)
        );
    }

    #[test]
    fn read_only_image_refuses_writes_and_keeps_bytes() {
        let data = pattern(SIZE, 3);
        let mut image =
            RawImage::open_read_only(MemoryStorage::new_read_only(&data), None, BS).unwrap();
        assert_eq!(
            image.write_at(0, &[1]),
            Err(RawError::Storage(StorageError::ReadOnly))
        );
        assert_eq!(
            image.write_aligned(0, &[0; 512]),
            Err(RawError::Storage(StorageError::ReadOnly))
        );
        assert_eq!(read_all(&image), data);
        assert!(!image.capabilities().writable);
    }

    #[test]
    fn truncated_backing_object_is_incomplete_not_out_of_range() {
        // The backend claims SIZE bytes but only holds 1024: data ends inside
        // the logical size.
        let mut backend = writer(&pattern(1024, 5));
        backend.set_reported_size(SIZE as u64);
        let image = RawImage::open_read_write(backend, Some(SIZE as u64), BS).unwrap();
        let mut buf = [0u8; 8];
        image.read_at(1016, &mut buf).unwrap();
        let mut straddle = [0u8; 16];
        assert_eq!(
            image.read_at(1016, &mut straddle),
            Err(RawError::IncompleteImage {
                offset: 1016,
                len: 16,
                logical_size: SIZE as u64
            })
        );
        assert_eq!(
            image.read_at(2048, &mut buf),
            Err(RawError::IncompleteImage {
                offset: 2048,
                len: 8,
                logical_size: SIZE as u64
            })
        );
        // The same range beyond the logical size is plain OutOfRange.
        assert!(matches!(
            image.read_at(SIZE as u64, &mut buf),
            Err(RawError::OutOfRange { .. })
        ));
        // The object shrinks after open: backend size is now below the
        // logical size, which is incomplete too (for reads and writes).
        let mut backend = writer(&[0; SIZE]);
        backend.set_reported_size(1024);
        let mut image = RawImage {
            storage: CheckedStorage::new(backend).unwrap(),
            logical_size: SIZE as u64,
            block_size: BS,
        };
        assert!(matches!(
            image.read_at(1024, &mut buf),
            Err(RawError::IncompleteImage { .. })
        ));
        assert!(matches!(
            image.write_at(1024, &buf),
            Err(RawError::IncompleteImage { .. })
        ));
    }

    #[test]
    fn partial_transfers_are_completed_and_other_errors_pass_through() {
        use crate::testing::memory::Fault;
        use std::io::ErrorKind;
        let data = pattern(SIZE, 11);
        let mut backend = writer(&[0; SIZE]);
        backend.set_max_chunk(7);
        let mut image = RawImage::create(backend, SIZE as u64, BS).unwrap();
        image.write_at(5, &data[..1500]).unwrap();
        let mut back = vec![0u8; 1500];
        image.read_at(5, &mut back).unwrap();
        assert_eq!(back, &data[..1500]);

        let mut backend = writer(&data);
        backend.push_fault(Fault::Error(ErrorKind::PermissionDenied));
        let image = RawImage::open_read_write(backend, None, BS).unwrap();
        let mut buf = [0u8; 4];
        assert_eq!(
            image.read_at(0, &mut buf),
            Err(RawError::Storage(StorageError::Io {
                op: crate::io::IoOp::Read,
                kind: ErrorKind::PermissionDenied,
                os_error: None,
                completed: 0
            }))
        );
    }

    #[test]
    fn no_durability_is_declared_or_available() {
        let mut image = new_image();
        image.write_at(0, &[1, 2, 3]).unwrap();
        let caps = image.capabilities();
        assert!(!caps.file_durability && !caps.namespace_durability);
        let mut storage = image.into_storage();
        assert_eq!(
            storage.flush_file_durable(),
            Err(StorageError::UnsupportedDurability {
                scope: DurabilityScope::File
            })
        );
        assert_eq!(
            storage.flush_namespace_durable(),
            Err(StorageError::UnsupportedDurability {
                scope: DurabilityScope::Namespace
            })
        );
    }

    #[test]
    fn image_cannot_grow_by_writing() {
        let mut image = new_image();
        assert!(matches!(
            image.write_at(SIZE as u64, &[1]),
            Err(RawError::OutOfRange { .. })
        ));
        assert_eq!(image.into_storage().size(), Ok(SIZE as u64));
    }
}
