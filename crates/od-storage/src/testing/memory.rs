//! In-memory [`StorageBackend`] with scripted faults.

use crate::io::{BackendError, StorageBackend, StorageCapabilities};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io::ErrorKind;

/// Default size limit of the memory backend (keeps tests from allocating
/// unbounded memory).
pub const MEMORY_MAX_LEN: u64 = 1 << 24;

/// One scripted fault consumed by the next read or write call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    /// Return `ErrorKind::Interrupted` without transferring anything.
    Interrupted,
    /// Return `Ok(0)`.
    ZeroProgress,
    /// Return an I/O error without transferring anything.
    Error(ErrorKind),
    /// Transfer at most this many bytes in this call (still a normal outcome).
    Limit(usize),
    /// Transfer the data but claim `requested + 1` bytes moved.
    ReportTooMany,
}

#[derive(Debug)]
pub struct MemoryStorage {
    data: Vec<u8>,
    capabilities: StorageCapabilities,
    faults: RefCell<VecDeque<Fault>>,
    max_chunk: Option<usize>,
    interrupt_every: Option<u32>,
    calls: Cell<u32>,
    reported_size: Option<u64>,
    flush_file_ok: bool,
}

impl MemoryStorage {
    pub fn new(initial: &[u8]) -> Self {
        Self::with_capabilities(initial, StorageCapabilities::read_write(MEMORY_MAX_LEN))
    }

    pub fn new_read_only(initial: &[u8]) -> Self {
        Self::with_capabilities(initial, StorageCapabilities::read_only(MEMORY_MAX_LEN))
    }

    /// Lets tests declare arbitrary (possibly false or inconsistent) claims.
    pub fn with_capabilities(initial: &[u8], capabilities: StorageCapabilities) -> Self {
        Self {
            data: initial.to_vec(),
            capabilities,
            faults: RefCell::new(VecDeque::new()),
            max_chunk: None,
            interrupt_every: None,
            calls: Cell::new(0),
            reported_size: None,
            flush_file_ok: false,
        }
    }

    /// Limits every read and write to at most `chunk` bytes (short transfers).
    pub fn set_max_chunk(&mut self, chunk: usize) {
        assert!(chunk > 0);
        self.max_chunk = Some(chunk);
    }

    /// Makes every `n`-th read or write call return `Interrupted` first.
    pub fn set_interrupt_every(&mut self, n: u32) {
        assert!(n >= 2, "n == 1 would never make progress");
        self.interrupt_every = Some(n);
    }

    pub fn push_fault(&mut self, fault: Fault) {
        self.faults.get_mut().push_back(fault);
    }

    /// Makes `size()` report a value different from the stored data length.
    pub fn set_reported_size(&mut self, size: u64) {
        self.reported_size = Some(size);
    }

    /// Makes a durable file flush return `Ok` (only meaningful together with a
    /// declared `file_durability` claim; the memory itself is not durable).
    pub fn set_flush_file_ok(&mut self) {
        self.flush_file_ok = true;
    }

    pub fn bytes(&self) -> &[u8] {
        &self.data
    }

    fn clamp(&self, len: usize) -> usize {
        self.max_chunk.map_or(len, |c| c.min(len))
    }

    fn next_fault(&self) -> Option<Fault> {
        let call = self.calls.get() + 1;
        self.calls.set(call);
        if self.interrupt_every.is_some_and(|n| call.is_multiple_of(n)) {
            return Some(Fault::Interrupted);
        }
        self.faults.borrow_mut().pop_front()
    }

    fn limit(&self, fault: Option<Fault>, len: usize) -> usize {
        let len = self.clamp(len);
        match fault {
            Some(Fault::Limit(n)) => len.min(n),
            _ => len,
        }
    }
}

impl StorageBackend for MemoryStorage {
    fn capabilities(&self) -> StorageCapabilities {
        self.capabilities
    }

    fn size(&self) -> Result<u64, BackendError> {
        Ok(self.reported_size.unwrap_or(self.data.len() as u64))
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, BackendError> {
        let fault = self.next_fault();
        match fault {
            Some(Fault::Interrupted) => return Err(BackendError::interrupted()),
            Some(Fault::ZeroProgress) => return Ok(0),
            Some(Fault::Error(kind)) => {
                return Err(BackendError::Io {
                    kind,
                    os_error: None,
                });
            }
            Some(Fault::ReportTooMany | Fault::Limit(_)) | None => {}
        }
        let start = match usize::try_from(offset) {
            Ok(s) if s < self.data.len() => s,
            _ => return Ok(0),
        };
        let n = self.limit(fault, buf.len().min(self.data.len() - start));
        buf[..n].copy_from_slice(&self.data[start..start + n]);
        Ok(if fault == Some(Fault::ReportTooMany) {
            buf.len() + 1
        } else {
            n
        })
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<usize, BackendError> {
        let fault = self.next_fault();
        match fault {
            Some(Fault::Interrupted) => return Err(BackendError::interrupted()),
            Some(Fault::ZeroProgress) => return Ok(0),
            Some(Fault::Error(kind)) => {
                return Err(BackendError::Io {
                    kind,
                    os_error: None,
                });
            }
            Some(Fault::ReportTooMany | Fault::Limit(_)) | None => {}
        }
        let n = self.limit(fault, buf.len());
        let end = offset
            .checked_add(n as u64)
            .filter(|e| *e <= self.capabilities.max_len)
            .ok_or(BackendError::Io {
                kind: ErrorKind::StorageFull,
                os_error: None,
            })?;
        let start = offset as usize;
        if self.data.len() < end as usize {
            self.data.resize(end as usize, 0);
        }
        self.data[start..start + n].copy_from_slice(&buf[..n]);
        Ok(if fault == Some(Fault::ReportTooMany) {
            buf.len() + 1
        } else {
            n
        })
    }

    fn set_len(&mut self, len: u64) -> Result<(), BackendError> {
        if len > self.capabilities.max_len {
            return Err(BackendError::Io {
                kind: ErrorKind::StorageFull,
                os_error: None,
            });
        }
        self.data.resize(len as usize, 0);
        Ok(())
    }

    fn flush_file_durable(&mut self) -> Result<(), BackendError> {
        if self.flush_file_ok {
            Ok(())
        } else {
            Err(BackendError::Unsupported)
        }
    }
}
