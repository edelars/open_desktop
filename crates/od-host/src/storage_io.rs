//! Host file implementation of the `od-storage` I/O contract.
//!
//! [`HostFileStorage`] adapts one regular file to
//! [`od_storage::io::StorageBackend`] using positioned reads and writes. The
//! operating-system file handle is a private runtime field: it is not
//! exposed, not serializable, and must not be copied into persistent
//! structures. Persistent state identifies storage by logical identity, never
//! by this value.
//!
//! This backend declares no durability and no locking:
//!
//! * `file_durability` and `namespace_durability` are `false`. Writing and
//!   closing a file is completion, not a power-loss durable acknowledgement;
//!   the macOS/APFS guarantee is unconfirmed (ISSUE-055) and the durable path
//!   belongs to a later task. `sync_all` is intentionally not called.
//! * `exclusive_lock` is `false`; single-writer enforcement is a later task.
//!
//! Only Unix targets are supported because the adapter uses positioned I/O
//! (`pread`/`pwrite`).

use od_storage::io::{BackendError, StorageBackend, StorageCapabilities};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::FileExt;
use std::path::Path;

/// Largest size representable by the host `off_t`.
const HOST_MAX_LEN: u64 = i64::MAX as u64;

/// A regular host file used as a storage backend.
#[derive(Debug)]
pub struct HostFileStorage {
    file: File,
    writable: bool,
}

impl HostFileStorage {
    /// Creates a new file of `len` zero bytes. Refuses to touch an existing
    /// path. If sizing fails the empty file created by this call is left in
    /// place and the error is returned: it is not removed, so that this call
    /// never deletes a path that another party may have replaced meanwhile.
    pub fn create_new(path: &Path, len: u64) -> io::Result<Self> {
        if len > HOST_MAX_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "length exceeds host file size limit",
            ));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)?;
        file.set_len(len)?;
        Ok(Self {
            file,
            writable: true,
        })
    }

    /// Opens an existing regular file for reading and writing.
    pub fn open_read_write(path: &Path) -> io::Result<Self> {
        Self::open(path, true)
    }

    /// Opens an existing regular file for reading only.
    pub fn open_read_only(path: &Path) -> io::Result<Self> {
        Self::open(path, false)
    }

    fn open(path: &Path, writable: bool) -> io::Result<Self> {
        // Check the kind before open: opening a FIFO can block forever and
        // opening a terminal or device can have side effects. Symlinks are
        // followed, so a link to a regular file is accepted.
        if !std::fs::metadata(path)?.is_file() {
            return Err(not_regular_file());
        }
        let file = OpenOptions::new().read(true).write(writable).open(path)?;
        // Re-check the opened object in case the path changed after the check.
        if !file.metadata()?.is_file() {
            return Err(not_regular_file());
        }
        Ok(Self { file, writable })
    }
}

fn not_regular_file() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "storage path is not a regular file",
    )
}

fn backend_error(error: io::Error) -> BackendError {
    BackendError::Io {
        kind: error.kind(),
        os_error: error.raw_os_error(),
    }
}

impl StorageBackend for HostFileStorage {
    fn capabilities(&self) -> StorageCapabilities {
        if self.writable {
            StorageCapabilities::read_write(HOST_MAX_LEN)
        } else {
            StorageCapabilities::read_only(HOST_MAX_LEN)
        }
    }

    fn size(&self) -> Result<u64, BackendError> {
        self.file
            .metadata()
            .map(|metadata| metadata.len())
            .map_err(backend_error)
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, BackendError> {
        self.file.read_at(buf, offset).map_err(backend_error)
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<usize, BackendError> {
        self.file.write_at(buf, offset).map_err(backend_error)
    }

    fn set_len(&mut self, len: u64) -> Result<(), BackendError> {
        self.file.set_len(len).map_err(backend_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_storage::io::{CheckedStorage, StorageError};
    use od_storage::testing::contract::{ContractFixture, run_contract};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    /// Temporary directory removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "od-host-storage-io-{}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&dir).expect("create temp dir");
            Self(dir)
        }

        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct HostFixture {
        dir: TempDir,
        count: u32,
        last: Option<PathBuf>,
    }

    impl HostFixture {
        fn new() -> Self {
            Self {
                dir: TempDir::new(),
                count: 0,
                last: None,
            }
        }

        fn seed(&mut self, initial: &[u8]) -> PathBuf {
            self.count += 1;
            let path = self.dir.path(&format!("disk-{}.raw", self.count));
            std::fs::write(&path, initial).expect("seed file");
            self.last = Some(path.clone());
            path
        }
    }

    impl ContractFixture for HostFixture {
        type Backend = HostFileStorage;

        fn create(&mut self, initial: &[u8]) -> HostFileStorage {
            let path = self.seed(initial);
            HostFileStorage::open_read_write(&path).expect("open read-write")
        }

        fn create_read_only(&mut self, initial: &[u8]) -> HostFileStorage {
            let path = self.seed(initial);
            HostFileStorage::open_read_only(&path).expect("open read-only")
        }

        fn external_bytes(&mut self) -> Option<Vec<u8>> {
            // Independent observation: plain filesystem read, not the backend.
            Some(std::fs::read(self.last.as_ref()?).expect("read file"))
        }
    }

    #[test]
    fn host_file_backend_passes_contract() {
        run_contract(&mut HostFixture::new());
    }

    #[test]
    fn host_backend_declares_no_durability_and_no_lock() {
        let mut fixture = HostFixture::new();
        let caps = fixture.create(&[1]).capabilities();
        assert!(caps.writable && caps.resize);
        assert!(!caps.file_durability);
        assert!(!caps.namespace_durability);
        assert!(!caps.exclusive_lock);
        assert_eq!(caps.max_len, i64::MAX as u64);
        let mut storage = CheckedStorage::new(fixture.create(&[1])).unwrap();
        storage.write_all_at(0, &[2]).expect("write completion");
        assert!(matches!(
            storage.flush_file_durable(),
            Err(StorageError::UnsupportedDurability { .. })
        ));
        assert!(matches!(
            storage.flush_namespace_durable(),
            Err(StorageError::UnsupportedDurability { .. })
        ));
    }

    #[test]
    fn create_new_makes_zeroed_file_and_refuses_existing_path() {
        let dir = TempDir::new();
        let path = dir.path("new.raw");
        let storage =
            CheckedStorage::new(HostFileStorage::create_new(&path, 4096).unwrap()).unwrap();
        assert_eq!(storage.size(), Ok(4096));
        let mut buf = vec![0xFFu8; 4096];
        storage.read_exact_at(0, &mut buf).unwrap();
        assert!(buf.iter().all(|b| *b == 0));
        drop(storage);

        std::fs::write(&path, b"keep").unwrap();
        let error = HostFileStorage::create_new(&path, 10).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"keep",
            "existing file untouched"
        );
    }

    #[test]
    fn create_new_rejects_oversized_length_without_creating_a_file() {
        let dir = TempDir::new();
        let path = dir.path("huge.raw");
        let error = HostFileStorage::create_new(&path, u64::MAX).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!path.exists());
    }

    #[test]
    fn open_rejects_fifo_without_blocking() {
        let dir = TempDir::new();
        let fifo = dir.path("pipe");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("run mkfifo");
        assert!(status.success());
        // Would block forever on open(2) if the kind were not checked first.
        for open in [
            HostFileStorage::open_read_only,
            HostFileStorage::open_read_write,
        ] {
            let error = open(&fifo).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn open_rejects_missing_path_and_directory() {
        let dir = TempDir::new();
        let missing = HostFileStorage::open_read_only(&dir.path("missing.raw")).unwrap_err();
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
        let directory = HostFileStorage::open_read_only(&dir.0).unwrap_err();
        assert_eq!(directory.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn file_truncated_by_another_party_is_out_of_range_not_zero_filled() {
        let mut fixture = HostFixture::new();
        let path = fixture.seed(&[7u8; 64]);
        let storage = CheckedStorage::new(HostFileStorage::open_read_only(&path).unwrap()).unwrap();
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(16)
            .unwrap();
        let mut buf = [0u8; 32];
        assert_eq!(
            storage.read_exact_at(0, &mut buf),
            Err(StorageError::OutOfRange {
                offset: 0,
                len: 32,
                size: 16
            })
        );
    }
}
