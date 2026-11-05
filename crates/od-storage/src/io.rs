//! Narrow storage I/O contract injected into storage code by a host layer.
//!
//! This module is the only way `od-storage` talks to an operating system. It
//! deliberately describes guarantees (checked offset I/O, explicit logical
//! size, declared capabilities) and not a filesystem API: there are no paths,
//! directory operations, or raw handles here.
//!
//! Two layers exist:
//!
//! * [`StorageBackend`] is the raw primitive implemented by a backend. Reads
//!   and writes may transfer fewer bytes than requested, may be interrupted,
//!   and report failures as [`BackendError`].
//! * [`CheckedStorage`] wraps one backend and is what upper code uses. It
//!   rejects overflow and out-of-range access before touching the backend,
//!   loops over short transfers, retries a bounded number of interrupted calls,
//!   and turns every other outcome into a typed [`StorageError`]. It never
//!   substitutes zero bytes for data it failed to read.
//!
//! Durability is separate from completion. A successful
//! [`CheckedStorage::write_all_at`] only means the backend accepted the bytes;
//! it is never a durable acknowledgement. A [`DurableAck`] can only be obtained
//! from a backend that declares the matching durability capability, and every
//! backend defaults to declaring none.

use std::fmt;
use std::io::ErrorKind;

/// Maximum number of consecutive interrupted calls without progress that
/// [`CheckedStorage`] retries before reporting
/// [`StorageError::InterruptedNoProgress`].
pub const MAX_INTERRUPTED_RETRIES: u32 = 16;

/// Kind of operation an error or fault refers to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoOp {
    Size,
    Read,
    Write,
    Resize,
    FlushFile,
    FlushNamespace,
}

/// What a durable acknowledgement covers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DurabilityScope {
    /// Contents and size of the backing object itself.
    File,
    /// The directory entry (name) that makes the object reachable.
    Namespace,
}

/// Capabilities a backend declares. All of them are claims that
/// [`CheckedStorage`] cross-checks against observed behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StorageCapabilities {
    /// The backend accepts writes and resizes.
    pub writable: bool,
    /// `set_len` can grow and shrink the backing object.
    pub resize: bool,
    /// Largest logical size the backend can represent.
    pub max_len: u64,
    /// `flush_file_durable` provides a verified durable acknowledgement for
    /// file contents. Must stay `false` until a host durability path exists.
    pub file_durability: bool,
    /// `flush_namespace_durable` provides a verified durable acknowledgement
    /// for the directory entry. Requires `file_durability`.
    pub namespace_durability: bool,
    /// This backend instance currently holds an exclusive writer lock on its
    /// backing object, enforced by the host across processes. Must be `true`
    /// only for an instance that actually acquired the lock; the acquisition
    /// API belongs to the host backend, not to this contract.
    pub exclusive_lock: bool,
}

impl StorageCapabilities {
    /// Read-only capabilities with no optional guarantees.
    pub const fn read_only(max_len: u64) -> Self {
        Self {
            writable: false,
            resize: false,
            max_len,
            file_durability: false,
            namespace_durability: false,
            exclusive_lock: false,
        }
    }

    /// Writable, resizable capabilities with no durability or locking claim.
    pub const fn read_write(max_len: u64) -> Self {
        Self {
            writable: true,
            resize: true,
            max_len,
            file_durability: false,
            namespace_durability: false,
            exclusive_lock: false,
        }
    }

    /// Same capabilities, additionally declaring that the instance holds an
    /// exclusive writer lock. Call only after the lock was actually acquired.
    pub const fn with_exclusive_lock(mut self) -> Self {
        self.exclusive_lock = true;
        self
    }

    /// Rejects combinations that cannot be true together.
    pub fn validate(&self) -> Result<(), CapabilityInconsistency> {
        if self.namespace_durability && !self.file_durability {
            return Err(CapabilityInconsistency::NamespaceDurabilityWithoutFileDurability);
        }
        if self.resize && !self.writable {
            return Err(CapabilityInconsistency::ResizeWithoutWritable);
        }
        Ok(())
    }
}

/// A declared capability set that is self-contradictory or contradicted by
/// backend behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityInconsistency {
    NamespaceDurabilityWithoutFileDurability,
    ResizeWithoutWritable,
    /// The backend declared resize but reported the operation unsupported.
    ClaimedResizeUnsupported,
    /// The backend declared durability for a scope but reported it unsupported.
    ClaimedDurabilityUnsupported(DurabilityScope),
}

/// A backend broke the raw primitive contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContractViolation {
    /// A read or write reported more bytes than were requested.
    TransferExceedsRequest {
        op: IoOp,
        requested: usize,
        returned: usize,
    },
    /// After a successful resize the backend reported a different size.
    SizeAfterResize { expected: u64, actual: u64 },
}

/// Failure reported by a raw backend call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendError {
    Io {
        kind: ErrorKind,
        os_error: Option<i32>,
    },
    /// The backend does not implement this optional operation.
    Unsupported,
    /// The exclusive writer no longer owns the object its path names (the
    /// path was replaced, renamed away or unlinked). The mutation was refused.
    WriterIdentityLost,
}

impl BackendError {
    pub fn interrupted() -> Self {
        Self::Io {
            kind: ErrorKind::Interrupted,
            os_error: None,
        }
    }

    fn is_interrupted(&self) -> bool {
        matches!(
            self,
            Self::Io {
                kind: ErrorKind::Interrupted,
                ..
            }
        )
    }
}

/// Raw storage primitive implemented by a host or test backend.
///
/// Implementations report what the underlying object did; they do not need to
/// loop, retry, or bounds-check. Upper code must use [`CheckedStorage`].
/// Implementations must not hold a lock or durable claim they have not
/// declared in [`StorageCapabilities`].
pub trait StorageBackend {
    fn capabilities(&self) -> StorageCapabilities;

    /// Current logical size in bytes.
    fn size(&self) -> Result<u64, BackendError>;

    /// Reads at most `buf.len()` bytes at `offset`. `Ok(0)` for a non-empty
    /// buffer means end of data.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, BackendError>;

    /// Writes at most `buf.len()` bytes at `offset`. `Ok(0)` for a non-empty
    /// buffer means no progress.
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<usize, BackendError>;

    /// Sets the logical size. Growth must read back as zero bytes.
    fn set_len(&mut self, _len: u64) -> Result<(), BackendError> {
        Err(BackendError::Unsupported)
    }

    /// Durable flush of file contents. Unsupported unless declared.
    fn flush_file_durable(&mut self) -> Result<(), BackendError> {
        Err(BackendError::Unsupported)
    }

    /// Durable flush of the directory entry. Unsupported unless declared.
    fn flush_namespace_durable(&mut self) -> Result<(), BackendError> {
        Err(BackendError::Unsupported)
    }
}

/// Typed outcome of a checked storage call.
///
/// When a transfer fails after making progress the number of bytes already
/// transferred is reported; the affected range of the backing object is then
/// partially modified (write) or the caller buffer is unspecified (read).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageError {
    /// `offset + len` does not fit in `u64`.
    OffsetOverflow {
        offset: u64,
        len: u64,
    },
    /// The range is not inside the logical size.
    OutOfRange {
        offset: u64,
        len: u64,
        size: u64,
    },
    /// A requested size exceeds what the backend can represent.
    ExceedsBackendLimit {
        requested: u64,
        max: u64,
    },
    /// Data ended inside the logical size: the object is shorter than its
    /// reported size.
    ShortRead {
        offset: u64,
        requested: usize,
        completed: usize,
    },
    /// The backend accepted zero bytes of a non-empty write.
    ShortWrite {
        offset: u64,
        requested: usize,
        completed: usize,
    },
    /// Too many consecutive interrupted calls without progress.
    InterruptedNoProgress {
        op: IoOp,
        offset: u64,
        completed: usize,
        attempts: u32,
    },
    Io {
        op: IoOp,
        kind: ErrorKind,
        os_error: Option<i32>,
        completed: usize,
    },
    /// A mutating call on a backend that declares `writable == false`.
    ReadOnly,
    /// The exclusive writer's backing path no longer names the object whose
    /// lock this writer holds, so the mutation was refused. The identity
    /// check is a `stat(path)` before the transfer, so it is best-effort:
    /// the path may change between the check and the write. A write still
    /// goes through the locked descriptor and cannot reach another file.
    /// `completed` bytes of a multi-chunk write were transferred before the
    /// refusal; the affected range may be partially modified.
    WriterIdentityLost {
        completed: usize,
    },
    /// The backend does not declare resize.
    UnsupportedResize,
    /// The backend does not declare durability for this scope.
    UnsupportedDurability {
        scope: DurabilityScope,
    },
    InconsistentCapabilities(CapabilityInconsistency),
    BackendContract(ContractViolation),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "storage error: {self:?}")
    }
}

impl std::error::Error for StorageError {}

/// Proof that a backend completed a durable flush for `scope`. This is
/// distinct from the completion of a write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DurableAck {
    scope: DurabilityScope,
}

impl DurableAck {
    pub fn scope(&self) -> DurabilityScope {
        self.scope
    }
}

/// Checked wrapper over one [`StorageBackend`].
///
/// Holds no host handle of its own and is not serializable; persistent
/// structures must store logical identities, never this value.
pub struct CheckedStorage<B: StorageBackend> {
    backend: B,
    capabilities: StorageCapabilities,
}

impl<B: StorageBackend> CheckedStorage<B> {
    /// Fails if the backend's declared capabilities are self-contradictory.
    pub fn new(backend: B) -> Result<Self, StorageError> {
        let capabilities = backend.capabilities();
        capabilities
            .validate()
            .map_err(StorageError::InconsistentCapabilities)?;
        Ok(Self {
            backend,
            capabilities,
        })
    }

    pub fn capabilities(&self) -> StorageCapabilities {
        self.capabilities
    }

    pub fn into_backend(self) -> B {
        self.backend
    }

    pub fn size(&self) -> Result<u64, StorageError> {
        self.backend.size().map_err(|e| map_error(IoOp::Size, e, 0))
    }

    /// Fills `buf` with the bytes at `offset` or fails. On `Err` the contents
    /// of `buf` are unspecified and must not be used.
    pub fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), StorageError> {
        let requested = buf.len();
        let size = self.size()?;
        check_range(offset, requested, size)?;
        let mut completed = 0usize;
        let mut interrupted = 0u32;
        while completed < requested {
            let position = offset + completed as u64;
            match self.backend.read_at(position, &mut buf[completed..]) {
                Ok(0) => {
                    return Err(StorageError::ShortRead {
                        offset,
                        requested,
                        completed,
                    });
                }
                Ok(n) if n > requested - completed => {
                    return Err(StorageError::BackendContract(
                        ContractViolation::TransferExceedsRequest {
                            op: IoOp::Read,
                            requested: requested - completed,
                            returned: n,
                        },
                    ));
                }
                Ok(n) => {
                    completed += n;
                    interrupted = 0;
                }
                Err(e) if e.is_interrupted() => {
                    interrupted += 1;
                    if interrupted > MAX_INTERRUPTED_RETRIES {
                        return Err(StorageError::InterruptedNoProgress {
                            op: IoOp::Read,
                            offset,
                            completed,
                            attempts: interrupted,
                        });
                    }
                }
                Err(e) => return Err(map_error(IoOp::Read, e, completed)),
            }
        }
        Ok(())
    }

    /// Writes all of `buf` inside the current logical size. Growing the object
    /// is an explicit [`set_len`](Self::set_len). Success is write completion,
    /// not durability.
    pub fn write_all_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), StorageError> {
        if !self.capabilities.writable {
            return Err(StorageError::ReadOnly);
        }
        let requested = buf.len();
        let size = self.size()?;
        check_range(offset, requested, size)?;
        // The exclusive lock is advisory: the file can shrink between the
        // `size()` above and the backend write. A positioned write past the
        // new end then extends the file with zeros and reports no error. Full
        // protection needs a mandatory lock or a size check after the write;
        // both are outside this slice.
        let mut completed = 0usize;
        let mut interrupted = 0u32;
        while completed < requested {
            let position = offset + completed as u64;
            match self.backend.write_at(position, &buf[completed..]) {
                Ok(0) => {
                    return Err(StorageError::ShortWrite {
                        offset,
                        requested,
                        completed,
                    });
                }
                Ok(n) if n > requested - completed => {
                    return Err(StorageError::BackendContract(
                        ContractViolation::TransferExceedsRequest {
                            op: IoOp::Write,
                            requested: requested - completed,
                            returned: n,
                        },
                    ));
                }
                Ok(n) => {
                    completed += n;
                    interrupted = 0;
                }
                Err(e) if e.is_interrupted() => {
                    interrupted += 1;
                    if interrupted > MAX_INTERRUPTED_RETRIES {
                        return Err(StorageError::InterruptedNoProgress {
                            op: IoOp::Write,
                            offset,
                            completed,
                            attempts: interrupted,
                        });
                    }
                }
                Err(e) => return Err(map_error(IoOp::Write, e, completed)),
            }
        }
        Ok(())
    }

    /// Explicitly grows (zero-filled) or shrinks the logical size.
    pub fn set_len(&mut self, len: u64) -> Result<(), StorageError> {
        if !self.capabilities.writable {
            return Err(StorageError::ReadOnly);
        }
        if !self.capabilities.resize {
            return Err(StorageError::UnsupportedResize);
        }
        if len > self.capabilities.max_len {
            return Err(StorageError::ExceedsBackendLimit {
                requested: len,
                max: self.capabilities.max_len,
            });
        }
        match self.backend.set_len(len) {
            Ok(()) => {}
            Err(BackendError::Unsupported) => {
                return Err(StorageError::InconsistentCapabilities(
                    CapabilityInconsistency::ClaimedResizeUnsupported,
                ));
            }
            Err(e) => return Err(map_error(IoOp::Resize, e, 0)),
        }
        let actual = self.size()?;
        if actual != len {
            return Err(StorageError::BackendContract(
                ContractViolation::SizeAfterResize {
                    expected: len,
                    actual,
                },
            ));
        }
        Ok(())
    }

    /// Durable flush of file contents. `UnsupportedDurability` unless the
    /// backend declares `file_durability`; the backend is not called then.
    pub fn flush_file_durable(&mut self) -> Result<DurableAck, StorageError> {
        let scope = DurabilityScope::File;
        if !self.capabilities.file_durability {
            return Err(StorageError::UnsupportedDurability { scope });
        }
        match self.backend.flush_file_durable() {
            Ok(()) => Ok(DurableAck { scope }),
            Err(e) => Err(map_durability_error(IoOp::FlushFile, scope, e)),
        }
    }

    /// Durable flush of the directory entry. `UnsupportedDurability` unless the
    /// backend declares `namespace_durability`.
    pub fn flush_namespace_durable(&mut self) -> Result<DurableAck, StorageError> {
        let scope = DurabilityScope::Namespace;
        if !self.capabilities.namespace_durability {
            return Err(StorageError::UnsupportedDurability { scope });
        }
        match self.backend.flush_namespace_durable() {
            Ok(()) => Ok(DurableAck { scope }),
            Err(e) => Err(map_durability_error(IoOp::FlushNamespace, scope, e)),
        }
    }
}

fn check_range(offset: u64, len: usize, size: u64) -> Result<(), StorageError> {
    let len = len as u64;
    let end = offset
        .checked_add(len)
        .ok_or(StorageError::OffsetOverflow { offset, len })?;
    if end > size {
        return Err(StorageError::OutOfRange { offset, len, size });
    }
    Ok(())
}

fn map_error(op: IoOp, error: BackendError, completed: usize) -> StorageError {
    match error {
        BackendError::Io { kind, os_error } => StorageError::Io {
            op,
            kind,
            os_error,
            completed,
        },
        BackendError::WriterIdentityLost => StorageError::WriterIdentityLost { completed },
        // An unsupported primitive required for a declared operation.
        BackendError::Unsupported => StorageError::Io {
            op,
            kind: ErrorKind::Unsupported,
            os_error: None,
            completed,
        },
    }
}

fn map_durability_error(op: IoOp, scope: DurabilityScope, error: BackendError) -> StorageError {
    match error {
        BackendError::Unsupported => StorageError::InconsistentCapabilities(
            CapabilityInconsistency::ClaimedDurabilityUnsupported(scope),
        ),
        other => map_error(op, other, 0),
    }
}
