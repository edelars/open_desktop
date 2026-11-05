//! Host file implementation of the `od-storage` I/O contract.
//!
//! [`HostFileStorage`] adapts one regular file to
//! [`od_storage::io::StorageBackend`] using positioned reads and writes. The
//! operating-system file handle is a private runtime field: it is not
//! exposed, not serializable, and must not be copied into persistent
//! structures. Persistent state identifies storage by logical identity, never
//! by this value.
//!
//! This backend declares no durability:
//!
//! * `file_durability` and `namespace_durability` are `false`. Writing and
//!   closing a file is completion, not a power-loss durable acknowledgement;
//!   the macOS/APFS guarantee is unconfirmed (ISSUE-055) and the durable path
//!   belongs to a later task. `sync_all` is intentionally not called.
//!
//! # Single writer
//!
//! [`HostFileStorage::open_read_write_exclusive`] takes a non-blocking
//! `flock(2)` `LOCK_EX` on the opened file. The lock belongs to the open file
//! description, so:
//!
//! * it is enforced between processes and between independent opens in one
//!   process (macOS and Linux both conflict across separate open file
//!   descriptions), not only inside one Rust ownership graph;
//! * it lives on the inode, so a symlink or hard link alias of the same file
//!   conflicts with the holder exactly like the original path;
//! * the kernel releases it when the descriptor is closed, including when the
//!   holder process crashes or is killed. There is no PID file and no lock
//!   file age: a live holder can never be displaced by a timeout.
//!
//! The held lock is the only reason `exclusive_lock` is reported `true`; every
//! other open reports `false`. Read-only opens take no lock and neither block
//! nor are blocked by a writer; read leases are a later task. `flock` is an
//! advisory lock: it does not stop a process that never calls `flock` and it
//! is not supported on network file systems.
//!
//! The file identity `(st_dev, st_ino)` is recorded from the locked
//! descriptor. Before every mutation the path is resolved again; if it no
//! longer names the same object the mutation is refused with
//! [`od_storage::io::StorageError::WriterIdentityLost`]. This check is
//! best-effort: the path can change in the window between `stat(path)` and
//! the `pwrite`, and such a write is not refused. It still goes through the
//! locked descriptor, so it can never reach a different file; it can only
//! modify the locked inode after its name was replaced or removed.
//!
//! There is deliberately no public way to obtain a writable instance without
//! this lock: [`HostFileStorage::create_new_exclusive`] and
//! [`HostFileStorage::open_read_write_exclusive`] are the only writable
//! constructors. Read leases for readers and VMHost lifecycle integration
//! are not part of this module yet (later S9 work).
//!
//! Only Unix targets are supported because the adapter uses positioned I/O
//! (`pread`/`pwrite`) and `flock(2)`.

use od_storage::io::{BackendError, StorageBackend, StorageCapabilities};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::{Path, PathBuf};

/// Largest size representable by the host `off_t`.
const HOST_MAX_LEN: u64 = i64::MAX as u64;

// `flock(2)`: see `man 2 flock`. Operation values are from
// `<sys/file.h>` / `<sys/fcntl.h>` (macOS SDK, Linux glibc agree).
const LOCK_EX: i32 = 0x02;
const LOCK_NB: i32 = 0x04;

unsafe extern "C" {
    // SAFETY contract: `fd` is an open descriptor owned by the caller;
    // `flock` reads no memory and only changes lock state in the kernel.
    fn flock(fd: i32, operation: i32) -> i32;
}

/// Stable identity of an open file: device and inode from `fstat`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    dev: u64,
    ino: u64,
}

impl FileIdentity {
    fn of(metadata: &std::fs::Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
        }
    }
}

/// Runtime-only state of an acquired exclusive writer lock. The lock itself
/// is held by the open file description inside [`HostFileStorage::file`] and
/// is released by the kernel when that descriptor closes; there is no unlock
/// step and no persistent representation.
#[derive(Debug)]
struct WriterLock {
    path: PathBuf,
    identity: FileIdentity,
}

/// Failure to acquire an exclusive writer.
#[derive(Debug)]
pub enum WriterOpenError {
    /// Another open file description (this or another process) holds the
    /// lock on the same file.
    WriterBusy,
    /// The path does not name the object that was opened and locked (it was
    /// replaced between `open` and `flock`).
    IdentityMismatch,
    Io(io::Error),
}

impl fmt::Display for WriterOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WriterBusy => f.write_str("backing file already has a writer"),
            Self::IdentityMismatch => f.write_str("backing path no longer names the opened file"),
            Self::Io(error) => write!(f, "backing file open failed: {error}"),
        }
    }
}

impl std::error::Error for WriterOpenError {}

impl From<io::Error> for WriterOpenError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// A regular host file used as a storage backend.
#[derive(Debug)]
pub struct HostFileStorage {
    file: File,
    writable: bool,
    /// `Some` only while this instance holds `LOCK_EX`.
    lock: Option<WriterLock>,
}

impl HostFileStorage {
    /// Creates a new file of `len` zero bytes and takes the exclusive writer
    /// lock on the created descriptor before sizing it. Refuses to touch an
    /// existing path. If locking or sizing fails the empty file created by
    /// this call is left in place and the error is returned: it is not
    /// removed, so that this call never deletes a path that another party may
    /// have replaced meanwhile.
    pub fn create_new_exclusive(path: &Path, len: u64) -> Result<Self, WriterOpenError> {
        // Known limits: (1) if `flock` or `set_len` fails below, the empty
        // file stays on disk (not removed, so a path replaced meanwhile is
        // never touched); (2) between `create_new` and `flock` another
        // process can open the new file and take the lock first, in which
        // case this call returns `WriterBusy`.
        if len > HOST_MAX_LEN {
            return Err(WriterOpenError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "length exceeds host file size limit",
            )));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)?;
        let storage = Self {
            file,
            writable: true,
            lock: None,
        }
        .acquire_writer_lock(path)?;
        storage.file.set_len(len)?;
        Ok(storage)
    }

    /// Opens an existing regular file for reading and writing and takes the
    /// exclusive writer lock (`flock(LOCK_EX | LOCK_NB)`) on the opened file.
    ///
    /// Returns [`WriterOpenError::WriterBusy`] if any other open file
    /// description, in this or another process and through any path, symlink
    /// or hard link to the same inode, holds the lock. Never waits.
    pub fn open_read_write_exclusive(path: &Path) -> Result<Self, WriterOpenError> {
        Self::open(path, true)?.acquire_writer_lock(path)
    }

    /// Takes `LOCK_EX | LOCK_NB` on the already opened file and records its
    /// identity. The result is the only way a writable instance is returned.
    fn acquire_writer_lock(mut self, path: &Path) -> Result<Self, WriterOpenError> {
        loop {
            // SAFETY: `file` is open for the duration of the call; see
            // `man 2 flock`. `LOCK_NB` guarantees the call never blocks.
            let rc = unsafe { flock(self.file.as_raw_fd(), LOCK_EX | LOCK_NB) };
            if rc == 0 {
                break;
            }
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::Interrupted => continue,
                io::ErrorKind::WouldBlock => return Err(WriterOpenError::WriterBusy),
                _ => return Err(WriterOpenError::Io(error)),
            }
        }
        // Identity of the locked object, then check the path still names it.
        // If it was swapped after `open`, we locked an orphan: refuse.
        let identity = FileIdentity::of(&self.file.metadata()?);
        match std::fs::metadata(path) {
            Ok(current) if FileIdentity::of(&current) == identity => {}
            Ok(_) => return Err(WriterOpenError::IdentityMismatch),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(WriterOpenError::IdentityMismatch);
            }
            Err(error) => return Err(WriterOpenError::Io(error)),
        }
        self.lock = Some(WriterLock {
            path: path.to_path_buf(),
            identity,
        });
        Ok(self)
    }

    /// Opens an existing regular file for reading only. Takes no lock: it
    /// does not wait for or block a writer (read leases are a later task).
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
        Ok(Self {
            file,
            writable,
            lock: None,
        })
    }

    /// Refuses a mutation when the exclusive writer's path no longer names
    /// the locked object. No-op for instances without a writer lock.
    fn check_writer_identity(&self) -> Result<(), BackendError> {
        let Some(lock) = &self.lock else {
            return Ok(());
        };
        match std::fs::metadata(&lock.path) {
            Ok(current) if FileIdentity::of(&current) == lock.identity => Ok(()),
            Ok(_) => Err(BackendError::WriterIdentityLost),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Err(BackendError::WriterIdentityLost)
            }
            Err(error) => Err(backend_error(error)),
        }
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
            let caps = StorageCapabilities::read_write(HOST_MAX_LEN);
            if self.lock.is_some() {
                caps.with_exclusive_lock()
            } else {
                caps
            }
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
        self.check_writer_identity()?;
        self.file.write_at(buf, offset).map_err(backend_error)
    }

    fn set_len(&mut self, len: u64) -> Result<(), BackendError> {
        self.check_writer_identity()?;
        self.file.set_len(len).map_err(backend_error)
    }
}

/// Serializes tests that hold a writer lock or spawn processes. `fork` copies
/// every open descriptor into the child until it `exec`s, so a child spawned
/// by one test thread briefly co-owns the open file description (and with it
/// the `flock`) of a writer in another test thread.
#[cfg(test)]
static LOCK_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn process_spawn_guard() -> std::sync::MutexGuard<'static, ()> {
    LOCK_TESTS.lock().unwrap_or_else(|e| e.into_inner())
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
            HostFileStorage::open_read_write_exclusive(&path).expect("open exclusive writer")
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
        let _serial = serial();
        run_contract(&mut HostFixture::new());
    }

    #[test]
    fn host_backend_declares_no_durability_and_lock_only_for_writer() {
        let _serial = serial();
        let mut fixture = HostFixture::new();
        let caps = fixture.create(&[1]).capabilities();
        assert!(caps.writable && caps.resize);
        assert!(!caps.file_durability);
        assert!(!caps.namespace_durability);
        assert!(
            caps.exclusive_lock,
            "writable instances always hold the lock"
        );
        assert_eq!(caps.max_len, i64::MAX as u64);
        let read_only = fixture.create_read_only(&[1]).capabilities();
        assert!(!read_only.writable && !read_only.exclusive_lock);
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
    fn create_new_makes_zeroed_locked_file_and_refuses_existing_path() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = dir.path("new.raw");
        let storage =
            CheckedStorage::new(HostFileStorage::create_new_exclusive(&path, 4096).unwrap())
                .unwrap();
        assert!(storage.capabilities().exclusive_lock);
        // The lock is held from creation: nobody else can become the writer.
        assert!(matches!(
            HostFileStorage::open_read_write_exclusive(&path),
            Err(WriterOpenError::WriterBusy)
        ));
        assert_eq!(storage.size(), Ok(4096));
        let mut buf = vec![0xFFu8; 4096];
        storage.read_exact_at(0, &mut buf).unwrap();
        assert!(buf.iter().all(|b| *b == 0));
        drop(storage);

        std::fs::write(&path, b"keep").unwrap();
        assert!(matches!(
            HostFileStorage::create_new_exclusive(&path, 10),
            Err(WriterOpenError::Io(e)) if e.kind() == io::ErrorKind::AlreadyExists
        ));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"keep",
            "existing file untouched"
        );
    }

    #[test]
    fn create_new_rejects_oversized_length_without_creating_a_file() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = dir.path("huge.raw");
        assert!(matches!(
            HostFileStorage::create_new_exclusive(&path, u64::MAX),
            Err(WriterOpenError::Io(e)) if e.kind() == io::ErrorKind::InvalidInput
        ));
        assert!(!path.exists());
    }

    #[test]
    fn open_rejects_fifo_without_blocking() {
        let _serial = serial();
        let dir = TempDir::new();
        let fifo = dir.path("pipe");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("run mkfifo");
        assert!(status.success());
        // Would block forever on open(2) if the kind were not checked first.
        let error = HostFileStorage::open_read_only(&fifo).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(matches!(
            HostFileStorage::open_read_write_exclusive(&fifo),
            Err(WriterOpenError::Io(e)) if e.kind() == io::ErrorKind::InvalidInput
        ));
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

    // ---- single writer (S9-04) ----

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        process_spawn_guard()
    }

    fn make_file(dir: &TempDir, name: &str) -> PathBuf {
        let path = dir.path(name);
        std::fs::write(&path, [0u8; 64]).unwrap();
        path
    }

    #[test]
    fn exclusive_open_declares_lock_and_read_only_does_not() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = make_file(&dir, "a.raw");
        let writer = HostFileStorage::open_read_write_exclusive(&path).unwrap();
        let caps = writer.capabilities();
        assert!(caps.exclusive_lock && caps.writable);
        assert!(!caps.file_durability && !caps.namespace_durability);
        // Read-only opens never claim a lock, even while a writer holds one.
        assert!(
            !HostFileStorage::open_read_only(&path)
                .unwrap()
                .capabilities()
                .exclusive_lock
        );
    }

    #[test]
    fn second_exclusive_open_is_busy_until_first_is_dropped() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = make_file(&dir, "a.raw");
        let first = HostFileStorage::open_read_write_exclusive(&path).unwrap();
        // Separate open file description in the same process still conflicts.
        assert!(matches!(
            HostFileStorage::open_read_write_exclusive(&path),
            Err(WriterOpenError::WriterBusy)
        ));
        // A second attempt does not disturb the holder.
        assert!(first.capabilities().exclusive_lock);
        drop(first);
        let second = HostFileStorage::open_read_write_exclusive(&path).unwrap();
        assert!(second.capabilities().exclusive_lock);
    }

    #[test]
    fn read_only_open_neither_blocks_nor_is_blocked_by_writer() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = make_file(&dir, "a.raw");
        let reader = HostFileStorage::open_read_only(&path).unwrap();
        let _writer = HostFileStorage::open_read_write_exclusive(&path).unwrap();
        let _reader2 = HostFileStorage::open_read_only(&path).unwrap();
        assert_eq!(reader.size(), Ok(64));
    }

    #[test]
    fn symlink_and_hard_link_aliases_hit_the_same_lock() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = make_file(&dir, "a.raw");
        let sym = dir.path("sym.raw");
        let hard = dir.path("hard.raw");
        std::os::unix::fs::symlink(&path, &sym).unwrap();
        std::fs::hard_link(&path, &hard).unwrap();

        // Holder via the real path: both aliases are busy.
        let holder = HostFileStorage::open_read_write_exclusive(&path).unwrap();
        for alias in [&sym, &hard] {
            assert!(matches!(
                HostFileStorage::open_read_write_exclusive(alias),
                Err(WriterOpenError::WriterBusy)
            ));
        }
        drop(holder);

        // Holder via an alias: the original path and the other alias are busy.
        let holder = HostFileStorage::open_read_write_exclusive(&hard).unwrap();
        for other in [&path, &sym] {
            assert!(matches!(
                HostFileStorage::open_read_write_exclusive(other),
                Err(WriterOpenError::WriterBusy)
            ));
        }
        drop(holder);
        HostFileStorage::open_read_write_exclusive(&sym).unwrap();
    }

    #[test]
    fn replaced_path_refuses_writes_instead_of_touching_either_file() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = make_file(&dir, "a.raw");
        let mut storage =
            CheckedStorage::new(HostFileStorage::open_read_write_exclusive(&path).unwrap())
                .unwrap();
        storage.write_all_at(0, &[1]).expect("write before swap");

        // Replace the path with a different file (rename over it).
        let intruder = dir.path("intruder.raw");
        std::fs::write(&intruder, [9u8; 64]).unwrap();
        std::fs::rename(&intruder, &path).unwrap();

        assert_eq!(
            storage.write_all_at(0, &[2]),
            Err(StorageError::WriterIdentityLost { completed: 0 })
        );
        assert_eq!(
            storage.set_len(8),
            Err(StorageError::WriterIdentityLost { completed: 0 })
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            vec![9u8; 64],
            "replacement file untouched"
        );
        // The path now names an unlocked object, so a new writer may open it;
        // the stale writer still cannot mutate anything.
        let fresh = HostFileStorage::open_read_write_exclusive(&path).unwrap();
        assert!(fresh.capabilities().exclusive_lock);
    }

    #[test]
    fn unlinked_path_refuses_writes() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = make_file(&dir, "a.raw");
        let mut storage =
            CheckedStorage::new(HostFileStorage::open_read_write_exclusive(&path).unwrap())
                .unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            storage.write_all_at(0, &[1]),
            Err(StorageError::WriterIdentityLost { completed: 0 })
        );
    }

    #[test]
    fn exclusive_open_of_missing_path_is_io_error() {
        let _serial = serial();
        let dir = TempDir::new();
        assert!(matches!(
            HostFileStorage::open_read_write_exclusive(&dir.path("missing.raw")),
            Err(WriterOpenError::Io(e)) if e.kind() == io::ErrorKind::NotFound
        ));
    }

    // ---- real second process ----
    //
    // The current test binary is re-executed with `OD_LOCK_CHILD_PATH` set;
    // the `lock_probe_child` test then acts as the second process. Without
    // the variable it is a no-op.

    const CHILD_PATH_ENV: &str = "OD_LOCK_CHILD_PATH";
    const CHILD_MODE_ENV: &str = "OD_LOCK_CHILD_MODE";
    const MARK: &str = "OD_LOCK_RESULT:";

    #[test]
    fn lock_probe_child() {
        use std::io::{BufRead, Write};
        let Ok(path) = std::env::var(CHILD_PATH_ENV) else {
            return;
        };
        let mode = std::env::var(CHILD_MODE_ENV).unwrap_or_default();
        let result = HostFileStorage::open_read_write_exclusive(Path::new(&path));
        let tag = match &result {
            Ok(_) => "LOCKED",
            Err(WriterOpenError::WriterBusy) => "BUSY",
            Err(_) => "ERROR",
        };
        let mut out = io::stdout().lock();
        writeln!(out, "{MARK}{tag}").unwrap();
        out.flush().unwrap();
        drop(out);
        if mode == "hold" && result.is_ok() {
            // Hold the lock until the parent kills this process (crash).
            let mut line = String::new();
            let _ = io::stdin().lock().read_line(&mut line);
        }
    }

    fn spawn_child(path: &Path, mode: &str) -> std::process::Child {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "storage_io::tests::lock_probe_child",
                "--nocapture",
            ])
            .env(CHILD_PATH_ENV, path)
            .env(CHILD_MODE_ENV, mode)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn child test process")
    }

    fn child_result(child: &mut std::process::Child) -> String {
        use std::io::BufRead;
        let stdout = child.stdout.take().unwrap();
        for line in io::BufReader::new(stdout).lines() {
            let line = line.unwrap();
            if let Some(tag) = line.strip_prefix(MARK) {
                return tag.to_string();
            }
        }
        panic!("child produced no result");
    }

    #[test]
    fn writer_in_another_process_blocks_us_and_crash_releases_it() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = make_file(&dir, "a.raw");

        // Child process takes the lock and holds it.
        let mut child = spawn_child(&path, "hold");
        assert_eq!(child_result(&mut child), "LOCKED");
        assert!(matches!(
            HostFileStorage::open_read_write_exclusive(&path),
            Err(WriterOpenError::WriterBusy)
        ));
        // Alias path is busy across processes as well.
        let alias = dir.path("alias.raw");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        assert!(matches!(
            HostFileStorage::open_read_write_exclusive(&alias),
            Err(WriterOpenError::WriterBusy)
        ));

        // Crash: SIGKILL, no destructor, no unlock call.
        child.kill().unwrap();
        child.wait().unwrap();
        HostFileStorage::open_read_write_exclusive(&path)
            .expect("lock released by the kernel after holder crash");
    }

    #[test]
    fn writer_in_this_process_blocks_another_process() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = make_file(&dir, "a.raw");
        let holder = HostFileStorage::open_read_write_exclusive(&path).unwrap();
        let mut child = spawn_child(&path, "probe");
        assert_eq!(child_result(&mut child), "BUSY");
        child.wait().unwrap();
        drop(holder);
        let mut child = spawn_child(&path, "probe");
        assert_eq!(child_result(&mut child), "LOCKED");
        child.wait().unwrap();
    }
}
