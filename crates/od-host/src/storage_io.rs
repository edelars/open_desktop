//! Host file implementation of the `od-storage` I/O contract.
//!
//! [`HostFileStorage`] adapts one regular file to
//! [`od_storage::io::StorageBackend`] using positioned reads and writes. The
//! operating-system file handle is a private runtime field: it is not
//! exposed, not serializable, and must not be copied into persistent
//! structures. Persistent state identifies storage by logical identity, never
//! by this value.
//!
//! # Durability
//!
//! Writing and closing a file is completion, not a power-loss durable
//! acknowledgement.
//!
//! * `file_durability` is `true` only for a writable instance holding the
//!   exclusive lock on macOS. [`StorageBackend::flush_file_durable`] there
//!   calls `fcntl(fd, F_FULLFSYNC)` on the locked descriptor: per `man fcntl`
//!   (macOS) it is `fsync` followed by a request to the drive to flush all
//!   buffered data to the permanent storage device, implemented on HFS, FAT,
//!   UDF and APFS. Volumes or devices that ignore the request (the man page
//!   mentions FireWire drives) are not detectable here. Only `ENOTSUP` is
//!   mapped to [`BackendError::DurabilityUnsupported`] (explicit refusal,
//!   never an acknowledgement, no reconcile state). This is a defensive
//!   assumption: `man fcntl` on the development machine does not describe
//!   `ENOTSUP` for `F_FULLFSYNC`, so the branch is not confirmed by a source.
//!   Every other failure, `EINVAL` included, is an ordinary I/O error with an
//!   ambiguous outcome, after which the checked layer demands reconciliation. `sync_all`/`fsync` is intentionally never used as a
//!   substitute: it does not carry the drive-flush request.
//! * On every other platform `file_durability` is `false`.
//! * `namespace_durability` is `false` everywhere: no source confirms how to
//!   make the creation or rename of a directory entry durable (ISSUE-055), so
//!   `flush_namespace_durable` stays unsupported.
//! * Read-only instances never claim durability.
//!
//! Power loss is not exercised by tests; only the call sequence and that the
//! written bytes are visible afterwards are checked.
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
//! other open reports `false`. [`HostFileStorage::open_read_only`] takes no
//! lock and neither blocks nor is blocked by a writer: it is **not** a lease
//! and gives no protection against a concurrent writer (it is kept for
//! callers that deliberately read a possibly changing file). A reader that
//! needs a stable view of a file without a writer takes a [`ReadLease`]
//! instead. `flock` is an advisory lock: it does not stop a process that never
//! calls `flock` and it is not supported on network file systems.
//!
//! # Read leases
//!
//! [`ReadLease::open`] opens a file read-only and takes a non-blocking shared
//! `flock(2)` lock (`LOCK_SH | LOCK_NB`) on it. Shared locks coexist, so any
//! number of leases (in this or other processes) may be held at once, but a
//! shared lock conflicts with `LOCK_EX`: while at least one lease is alive
//! every writer constructor returns [`WriterOpenError::WriterBusy`], and while
//! a writer holds `LOCK_EX` a lease request returns
//! [`LeaseOpenError::LeaseBusy`]. The lease lives exactly as long as the
//! descriptor: `Drop`, process exit and `SIGKILL` all release it through the
//! kernel, with no unlock call, PID file or timeout. A lease only blocks
//! writers that take the `flock` (all writers of this module do); plain
//! `open_read_only` readers and foreign tools are neither blocked nor blocking.
//! The `(st_dev, st_ino)` identity is taken from the locked descriptor and the
//! path must still name it, otherwise [`LeaseOpenError::IdentityMismatch`].
//! The lease offers read access only (`read_at`, `size`).
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
//! constructors. The export/repair busy policy and the parent-lease primitive
//! live in [`crate::storage_policy`]; VMHost lifecycle integration is not part
//! of this module yet (later S9 work).
//!
//! Only Unix targets are supported because the adapter uses positioned I/O
//! (`pread`/`pwrite`) and `flock(2)`.

use od_storage::io::{BackendError, DurabilityScope, StorageBackend, StorageCapabilities};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::{Path, PathBuf};

/// Largest size representable by the host `off_t`.
pub(crate) const HOST_MAX_LEN: u64 = i64::MAX as u64;

// `flock(2)`: see `man 2 flock`. Operation values are from
// `<sys/file.h>` / `<sys/fcntl.h>` (macOS SDK, Linux glibc agree).
const LOCK_SH: i32 = 0x01;
const LOCK_EX: i32 = 0x02;
const LOCK_NB: i32 = 0x04;

unsafe extern "C" {
    // SAFETY contract: `fd` is an open descriptor owned by the caller;
    // `flock` reads no memory and only changes lock state in the kernel.
    fn flock(fd: i32, operation: i32) -> i32;
}

/// `fcntl(2)` command `F_FULLFSYNC` from the macOS SDK `<sys/fcntl.h>`.
#[cfg(target_os = "macos")]
const F_FULLFSYNC: i32 = 51;
/// macOS `errno` value (`<sys/errno.h>`). Defensive assumption: it is treated
/// as an explicit "durability unsupported" refusal for `F_FULLFSYNC`, but
/// `man fcntl` on this machine does not describe it for that command; the
/// mapping is not confirmed by a source. `EINVAL` is deliberately not mapped:
/// its meaning here is ambiguous, so it is a plain I/O error (sticky).
#[cfg(target_os = "macos")]
const ENOTSUP: i32 = 45;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    // `fcntl` is variadic in C (`int fcntl(int, int, ...)`). It must be
    // declared variadic: on Apple arm64 variadic arguments are passed on the
    // stack, so a fixed-arity declaration would be a different ABI. For
    // `F_FULLFSYNC` the third argument is ignored by the kernel; a `c_int`
    // zero is passed anyway.
    // SAFETY contract: `fd` is an open descriptor owned by the caller;
    // `F_FULLFSYNC` reads no caller memory.
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
}

/// Takes a non-blocking `flock` and retries on `EINTR`. `Ok(false)` means the
/// lock is held incompatibly by another open file description.
fn flock_nonblocking(file: &File, operation: i32) -> io::Result<bool> {
    loop {
        // SAFETY: `file` is open for the duration of the call; see
        // `man 2 flock`. `LOCK_NB` guarantees the call never blocks.
        let rc = unsafe { flock(file.as_raw_fd(), operation | LOCK_NB) };
        if rc == 0 {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        match error.kind() {
            io::ErrorKind::Interrupted => continue,
            io::ErrorKind::WouldBlock => return Ok(false),
            _ => return Err(error),
        }
    }
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
        use std::os::unix::fs::OpenOptionsExt as _;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
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

    /// `(st_dev, st_ino)` of the file this instance holds the exclusive writer
    /// lock on (from `fstat` on the locked descriptor); `None` for an instance
    /// without a writer lock.
    pub fn file_identity(&self) -> Option<(u64, u64)> {
        self.lock
            .as_ref()
            .map(|lock| (lock.identity.dev, lock.identity.ino))
    }

    /// Opens an existing regular file for reading and writing and takes the
    /// exclusive writer lock (`flock(LOCK_EX | LOCK_NB)`) on the opened file.
    ///
    /// Returns [`WriterOpenError::WriterBusy`] if any other open file
    /// description, in this or another process and through any path, symlink
    /// or hard link to the same inode, holds the lock. Never waits.
    pub fn open_read_write_exclusive(path: &Path) -> Result<Self, WriterOpenError> {
        Self::open_exclusive_checking(path, path)
    }

    /// Opens `open_path` and, after locking, checks that `check_path` still
    /// names the locked object. The public constructor passes the same path
    /// twice; a distinct `check_path` is an internal hook that simulates a
    /// path swap between `open` and `flock` in tests.
    pub(crate) fn open_exclusive_checking(
        open_path: &Path,
        check_path: &Path,
    ) -> Result<Self, WriterOpenError> {
        Self::open(open_path, true)?.acquire_writer_lock(check_path)
    }

    /// Takes `LOCK_EX | LOCK_NB` on the already opened file and records its
    /// identity. The result is the only way a writable instance is returned.
    fn acquire_writer_lock(mut self, path: &Path) -> Result<Self, WriterOpenError> {
        if !flock_nonblocking(&self.file, LOCK_EX)? {
            return Err(WriterOpenError::WriterBusy);
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

    /// Opens an existing regular file for reading only. Takes no lock and is
    /// **not** a lease: it neither waits for nor blocks a writer, so the file
    /// may change under the reader. Use [`ReadLease::open`] for a read that a
    /// writer must not overlap.
    pub fn open_read_only(path: &Path) -> io::Result<Self> {
        Self::open(path, false)
    }

    fn open(path: &Path, writable: bool) -> io::Result<Self> {
        Ok(Self {
            file: open_regular_file(path, writable)?,
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

/// Opens `path` after checking that it names a regular file.
fn open_regular_file(path: &Path, writable: bool) -> io::Result<File> {
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
    Ok(file)
}

/// Failure to acquire a read lease.
#[derive(Debug)]
pub enum LeaseOpenError {
    /// A writer holds the exclusive lock on the same file (this or another
    /// process, through any alias of the inode).
    LeaseBusy,
    /// The path does not name the object that was opened and locked.
    IdentityMismatch,
    Io(io::Error),
}

impl fmt::Display for LeaseOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LeaseBusy => f.write_str("backing file is held by a writer"),
            Self::IdentityMismatch => f.write_str("backing path no longer names the opened file"),
            Self::Io(error) => write!(f, "backing file open failed: {error}"),
        }
    }
}

impl std::error::Error for LeaseOpenError {}

impl From<io::Error> for LeaseOpenError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// A shared read lease on a regular host file.
///
/// Holds `flock(LOCK_SH)` on its own open file description until dropped (or
/// until the process dies); see the module docs. It implements
/// [`StorageBackend`] as a read-only backend: `capabilities` reports
/// read-only with no lock or durability claim (a shared lease is not the
/// exclusive writer lock), and every mutating method is refused with
/// [`BackendError::Unsupported`]. The descriptor is private runtime state and
/// is not exposed.
#[derive(Debug)]
pub struct ReadLease {
    file: File,
    identity: FileIdentity,
}

impl ReadLease {
    /// Opens an existing regular file read-only and takes
    /// `flock(LOCK_SH | LOCK_NB)`. Never waits.
    ///
    /// Returns [`LeaseOpenError::LeaseBusy`] while a writer holds `LOCK_EX`,
    /// and [`LeaseOpenError::IdentityMismatch`] when the path no longer names
    /// the locked inode (replaced or removed between `open` and `flock`).
    pub fn open(path: &Path) -> Result<Self, LeaseOpenError> {
        Self::open_checking(path, path)
    }

    /// Opens `open_path` and, after locking, checks that `check_path` still
    /// names the locked object (see `HostFileStorage::open_exclusive_checking`).
    pub(crate) fn open_checking(
        open_path: &Path,
        check_path: &Path,
    ) -> Result<Self, LeaseOpenError> {
        Self::lock_opened(open_regular_file(open_path, false)?, check_path)
    }

    /// Locks an already opened file and checks that `path` still names it.
    fn lock_opened(file: File, path: &Path) -> Result<Self, LeaseOpenError> {
        if !flock_nonblocking(&file, LOCK_SH)? {
            return Err(LeaseOpenError::LeaseBusy);
        }
        let identity = FileIdentity::of(&file.metadata()?);
        match std::fs::metadata(path) {
            Ok(current) if FileIdentity::of(&current) == identity => {}
            Ok(_) => return Err(LeaseOpenError::IdentityMismatch),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(LeaseOpenError::IdentityMismatch);
            }
            Err(error) => return Err(LeaseOpenError::Io(error)),
        }
        Ok(Self { file, identity })
    }

    /// `(st_dev, st_ino)` of the leased file, from `fstat` on the locked
    /// descriptor.
    pub fn file_identity(&self) -> (u64, u64) {
        (self.identity.dev, self.identity.ino)
    }
}

impl StorageBackend for ReadLease {
    fn capabilities(&self) -> StorageCapabilities {
        StorageCapabilities::read_only(HOST_MAX_LEN)
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

    fn write_at(&mut self, _offset: u64, _buf: &[u8]) -> Result<usize, BackendError> {
        Err(BackendError::Unsupported)
    }

    // `set_len`, `flush_file_durable` and `flush_namespace_durable` keep the
    // trait defaults: `Unsupported`.
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
                let mut caps = caps.with_exclusive_lock();
                // File data durability only for the locked writer on macOS
                // (`F_FULLFSYNC`); never namespace durability.
                caps.file_durability = cfg!(target_os = "macos");
                caps
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

    #[cfg(target_os = "macos")]
    fn flush_file_durable(&mut self) -> Result<(), BackendError> {
        // Only the exclusive writer declares file durability; refuse anything
        // else, and refuse to acknowledge a file whose path no longer names
        // the locked object.
        if !self.writable || self.lock.is_none() {
            return Err(BackendError::Unsupported);
        }
        self.check_writer_identity()?;
        loop {
            // SAFETY: the descriptor is owned by `self.file` and open for the
            // call; `F_FULLFSYNC` ignores the variadic argument and touches no
            // caller memory (see `man 2 fcntl`).
            let rc = unsafe { fcntl(self.file.as_raw_fd(), F_FULLFSYNC, 0 as std::ffi::c_int) };
            if rc != -1 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                // Defensive assumption, not confirmed by `man fcntl` (see the
                // errno constants above and the module docs).
                Some(ENOTSUP) => {
                    return Err(BackendError::DurabilityUnsupported {
                        scope: DurabilityScope::File,
                    });
                }
                _ if error.kind() == io::ErrorKind::Interrupted => continue,
                _ => return Err(backend_error(error)),
            }
        }
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
    fn host_backend_declares_file_durability_only_on_macos_writer_and_lock_only_for_writer() {
        let _serial = serial();
        let mut fixture = HostFixture::new();
        let caps = fixture.create(&[1]).capabilities();
        assert!(caps.writable && caps.resize);
        assert_eq!(caps.file_durability, cfg!(target_os = "macos"));
        assert!(!caps.namespace_durability);
        assert!(
            caps.exclusive_lock,
            "writable instances always hold the lock"
        );
        assert_eq!(caps.max_len, i64::MAX as u64);
        let read_only = fixture.create_read_only(&[1]).capabilities();
        assert!(!read_only.writable && !read_only.exclusive_lock);
        assert!(!read_only.file_durability && !read_only.namespace_durability);
        let mut storage = CheckedStorage::new(fixture.create(&[1])).unwrap();
        storage.write_all_at(0, &[2]).expect("write completion");
        assert!(matches!(
            storage.flush_namespace_durable(),
            Err(StorageError::UnsupportedDurability { .. })
        ));
        let mut read_only = CheckedStorage::new(fixture.create_read_only(&[1])).unwrap();
        assert!(matches!(
            read_only.flush_file_durable(),
            Err(StorageError::UnsupportedDurability {
                scope: od_storage::io::DurabilityScope::File
            })
        ));
        assert!(matches!(
            read_only.flush_namespace_durable(),
            Err(StorageError::UnsupportedDurability { .. })
        ));
    }

    /// Order write -> flush -> reopen read-only, compared with `std::fs::read`.
    /// This checks the call sequence and that the bytes are visible; it does
    /// NOT test power loss (not reproducible here).
    #[test]
    fn flush_file_durable_after_write_on_exclusive_writer() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = make_file(&dir, "durable.raw");
        let mut storage =
            CheckedStorage::new(HostFileStorage::open_read_write_exclusive(&path).unwrap())
                .unwrap();
        storage.write_all_at(3, b"durable").expect("write");
        let ack = storage.flush_file_durable();
        if cfg!(target_os = "macos") {
            assert_eq!(
                ack.map(|a| a.scope()),
                Ok(od_storage::io::DurabilityScope::File)
            );
        } else {
            assert!(matches!(
                ack,
                Err(StorageError::UnsupportedDurability { .. })
            ));
        }
        assert!(matches!(
            storage.flush_namespace_durable(),
            Err(StorageError::UnsupportedDurability { .. })
        ));
        drop(storage);
        let reopened =
            CheckedStorage::new(HostFileStorage::open_read_only(&path).unwrap()).unwrap();
        let mut via_backend = vec![0u8; 64];
        reopened.read_exact_at(0, &mut via_backend).unwrap();
        let via_fs = std::fs::read(&path).unwrap();
        assert_eq!(via_backend, via_fs);
        assert_eq!(&via_fs[3..10], b"durable");
    }

    /// A flush on a backend whose path was replaced is refused, not acked.
    #[cfg(target_os = "macos")]
    #[test]
    fn flush_file_durable_refuses_lost_writer_identity() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = make_file(&dir, "lost.raw");
        let mut backend = HostFileStorage::open_read_write_exclusive(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            backend.flush_file_durable(),
            Err(BackendError::WriterIdentityLost)
        );
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
        assert_eq!(caps.file_durability, cfg!(target_os = "macos"));
        assert!(!caps.namespace_durability);
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
        let (tag, acquired) = if mode.starts_with("lease") {
            match ReadLease::open(Path::new(&path)) {
                Ok(lease) => ("LEASED", Some(Box::new(lease) as Box<dyn std::any::Any>)),
                Err(LeaseOpenError::LeaseBusy) => ("BUSY", None),
                Err(_) => ("ERROR", None),
            }
        } else {
            match HostFileStorage::open_read_write_exclusive(Path::new(&path)) {
                Ok(writer) => ("LOCKED", Some(Box::new(writer) as Box<dyn std::any::Any>)),
                Err(WriterOpenError::WriterBusy) => ("BUSY", None),
                Err(_) => ("ERROR", None),
            }
        };
        let mut out = io::stdout().lock();
        writeln!(out, "{MARK}{tag}").unwrap();
        out.flush().unwrap();
        drop(out);
        if (mode == "hold" || mode == "lease_hold") && acquired.is_some() {
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

    // ---- read leases (S9-04) ----

    fn lease(path: &Path) -> ReadLease {
        ReadLease::open(path).expect("take read lease")
    }

    #[test]
    fn lease_reads_bytes_and_size_through_the_backend() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = dir.path("a.raw");
        std::fs::write(&path, b"0123456789").unwrap();
        let lease = lease(&path);
        let caps = lease.capabilities();
        assert!(caps.validate().is_ok());
        assert!(!caps.writable && !caps.resize && !caps.exclusive_lock);
        assert!(!caps.file_durability && !caps.namespace_durability);
        let storage = CheckedStorage::new(lease).unwrap();
        assert_eq!(storage.size(), Ok(10));
        let mut buf = [0u8; 4];
        storage.read_exact_at(3, &mut buf).unwrap();
        assert_eq!(&buf, b"3456");
    }

    #[test]
    fn lease_refuses_every_mutation() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = make_file(&dir, "a.raw");
        let mut lease = lease(&path);
        assert_eq!(lease.write_at(0, &[1]), Err(BackendError::Unsupported));
        assert_eq!(lease.set_len(8), Err(BackendError::Unsupported));
        assert_eq!(lease.flush_file_durable(), Err(BackendError::Unsupported));
        assert_eq!(
            lease.flush_namespace_durable(),
            Err(BackendError::Unsupported)
        );
        let mut storage = CheckedStorage::new(lease).unwrap();
        assert!(storage.write_all_at(0, &[1]).is_err());
        assert!(storage.set_len(8).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), vec![0u8; 64]);
    }

    #[test]
    fn lease_blocks_writer_in_this_process_until_dropped() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = make_file(&dir, "a.raw");
        let held = lease(&path);
        assert!(matches!(
            HostFileStorage::open_read_write_exclusive(&path),
            Err(WriterOpenError::WriterBusy)
        ));
        // Aliases of the inode are busy as well.
        let sym = dir.path("sym.raw");
        let hard = dir.path("hard.raw");
        std::os::unix::fs::symlink(&path, &sym).unwrap();
        std::fs::hard_link(&path, &hard).unwrap();
        for alias in [&sym, &hard] {
            assert!(matches!(
                HostFileStorage::open_read_write_exclusive(alias),
                Err(WriterOpenError::WriterBusy)
            ));
        }
        // The refused attempts did not disturb the lease.
        assert_eq!(held.size(), Ok(64));
        // A second independent exclusive attempt using raw flock is refused too.
        let probe = OpenOptions::new().read(true).open(&path).unwrap();
        assert!(!flock_nonblocking(&probe, LOCK_EX).expect("flock result"));
        drop(probe);
        drop(held);
        let writer = HostFileStorage::open_read_write_exclusive(&path)
            .expect("writer can open after the lease is dropped");
        assert!(writer.capabilities().exclusive_lock);
    }

    #[test]
    fn lease_is_busy_while_writer_holds_the_lock_and_works_after() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = make_file(&dir, "a.raw");
        let writer = HostFileStorage::open_read_write_exclusive(&path).unwrap();
        assert!(matches!(
            ReadLease::open(&path),
            Err(LeaseOpenError::LeaseBusy)
        ));
        let alias = dir.path("alias.raw");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        assert!(matches!(
            ReadLease::open(&alias),
            Err(LeaseOpenError::LeaseBusy)
        ));
        drop(writer);
        let _ = lease(&path);
        // A fresh writer created by create_new_exclusive blocks leases too.
        let created = dir.path("created.raw");
        let writer = HostFileStorage::create_new_exclusive(&created, 16).unwrap();
        assert!(matches!(
            ReadLease::open(&created),
            Err(LeaseOpenError::LeaseBusy)
        ));
        drop(writer);
        assert_eq!(lease(&created).size(), Ok(16));
    }

    #[test]
    fn several_leases_coexist_and_writer_waits_for_the_last() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = make_file(&dir, "a.raw");
        let first = lease(&path);
        let second = lease(&path);
        let third = lease(&path);
        assert_eq!(first.file_identity(), second.file_identity());
        assert_eq!(second.file_identity(), third.file_identity());
        drop(first);
        drop(second);
        assert!(matches!(
            HostFileStorage::open_read_write_exclusive(&path),
            Err(WriterOpenError::WriterBusy)
        ));
        drop(third);
        HostFileStorage::open_read_write_exclusive(&path).expect("writer after last lease");
    }

    #[test]
    fn lease_does_not_block_plain_read_only_opens_and_open_read_only_gives_no_lease() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = make_file(&dir, "a.raw");
        // A lease does not stop a plain read-only open.
        let held = lease(&path);
        let plain = HostFileStorage::open_read_only(&path).unwrap();
        assert_eq!(plain.size(), Ok(64));
        assert!(!plain.capabilities().exclusive_lock);
        drop(held);
        // A plain read-only open gives no lease: a writer still opens.
        let writer = HostFileStorage::open_read_write_exclusive(&path)
            .expect("plain read-only open does not hold a lease");
        drop(writer);
        drop(plain);
    }

    #[test]
    fn lease_identity_matches_fstat_and_alias_resolves_to_same_inode() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = make_file(&dir, "a.raw");
        let sym = dir.path("sym.raw");
        std::os::unix::fs::symlink(&path, &sym).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        let via_path = lease(&path);
        assert_eq!(via_path.file_identity(), (meta.dev(), meta.ino()));
        let via_alias = lease(&sym);
        assert_eq!(via_alias.file_identity(), via_path.file_identity());
    }

    #[test]
    fn lease_refuses_path_that_names_another_or_no_inode() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = make_file(&dir, "a.raw");
        let other = make_file(&dir, "b.raw");
        // Simulates a swap between `open` and `flock`: the descriptor names
        // `a.raw` while the path names `b.raw`.
        let file = open_regular_file(&path, false).unwrap();
        assert!(matches!(
            ReadLease::lock_opened(file, &other),
            Err(LeaseOpenError::IdentityMismatch)
        ));
        // The refused attempt released its lock (descriptor dropped).
        HostFileStorage::open_read_write_exclusive(&path).expect("no lease left behind");
        // Path removed after open.
        let file = open_regular_file(&path, false).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(
            ReadLease::lock_opened(file, &path),
            Err(LeaseOpenError::IdentityMismatch)
        ));
    }

    #[test]
    fn lease_open_rejects_missing_path_directory_and_fifo() {
        let _serial = serial();
        let dir = TempDir::new();
        assert!(matches!(
            ReadLease::open(&dir.path("missing.raw")),
            Err(LeaseOpenError::Io(e)) if e.kind() == io::ErrorKind::NotFound
        ));
        assert!(matches!(
            ReadLease::open(&dir.0),
            Err(LeaseOpenError::Io(e)) if e.kind() == io::ErrorKind::InvalidInput
        ));
        let fifo = dir.path("pipe");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("run mkfifo");
        assert!(status.success());
        assert!(matches!(
            ReadLease::open(&fifo),
            Err(LeaseOpenError::Io(e)) if e.kind() == io::ErrorKind::InvalidInput
        ));
    }

    #[test]
    fn lease_in_another_process_blocks_writer_allows_readers_and_sigkill_releases_it() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = make_file(&dir, "a.raw");
        let mut child = spawn_child(&path, "lease_hold");
        assert_eq!(child_result(&mut child), "LEASED");
        assert!(matches!(
            HostFileStorage::open_read_write_exclusive(&path),
            Err(WriterOpenError::WriterBusy)
        ));
        // A second reader in this process coexists with the foreign lease.
        let ours = lease(&path);
        drop(ours);
        // A writer process is refused too.
        let mut probe = spawn_child(&path, "probe");
        assert_eq!(child_result(&mut probe), "BUSY");
        probe.wait().unwrap();
        // Crash: SIGKILL, no destructor, no unlock call.
        child.kill().unwrap();
        child.wait().unwrap();
        HostFileStorage::open_read_write_exclusive(&path)
            .expect("lease released by the kernel after reader crash");
    }

    #[test]
    fn writer_in_this_process_makes_lease_busy_in_another_process() {
        let _serial = serial();
        let dir = TempDir::new();
        let path = make_file(&dir, "a.raw");
        let writer = HostFileStorage::open_read_write_exclusive(&path).unwrap();
        let mut child = spawn_child(&path, "lease_probe");
        assert_eq!(child_result(&mut child), "BUSY");
        child.wait().unwrap();
        drop(writer);
        let mut child = spawn_child(&path, "lease_probe");
        assert_eq!(child_result(&mut child), "LEASED");
        child.wait().unwrap();
    }
}
