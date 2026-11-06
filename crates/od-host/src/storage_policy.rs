//! Busy policy for export and repair, and the parent-lease primitive (S9-04).
//!
//! Both policies are built on the `flock(2)` locks of [`crate::storage_io`]:
//!
//! * [`export_with_lease`] holds a [`ReadLease`] (shared lock) for the whole
//!   closure. While it is held no writer can open the file.
//! * [`repair_with_writer`] holds the exclusive writer lock
//!   ([`HostFileStorage::open_read_write_exclusive`]) for the whole closure.
//!   While it is held neither a writer nor an export lease can be taken.
//! * Neither function waits, retries or takes a lock away from a live owner:
//!   a conflicting holder yields an immediate [`PolicyError::ExportBusy`] /
//!   [`PolicyError::RepairBusy`] and the closure is not called.
//! * [`ParentLease`] is a [`ReadLease`] bound to its path that re-checks
//!   `(st_dev, st_ino)` on every open and every use.
//!
//! # Limits
//!
//! * `flock` is advisory: it only coordinates processes that call it (all
//!   writers and leases of [`crate::storage_io`] do). A foreign tool, or a
//!   plain `open_read_only`, ignores it and is not blocked.
//! * `flock` is not supported on network file systems (NFS and similar); on
//!   such a volume none of these guarantees is claimed.
//! * Identity checks are best-effort between a `stat(path)` and the following
//!   use (see the `storage_io` module docs): a swapped path is detected, but
//!   the check itself is not atomic with the use. Writes still go through the
//!   locked descriptor, so they never reach a different file.
//! * The repair lock cannot tell a writer from a read lease: both appear as
//!   one busy holder ([`PolicyError::RepairBusy`]).
//! * [`ParentLease`] itself has no notion of a parent inside a snapshot chain.
//!   [`ParentLeaseStorage`] adapts it to the storage contract so the S9-14
//!   chain code in `od-storage` can read ancestors through it. The chain
//!   itself is verified by `od-storage` before its writable open writes
//!   (S9-16); the tests here prove, over real files, that an open child keeps
//!   its ancestors leased and that a swap after validation yields a failed
//!   read, not foreign bytes. There is still no production `ParentResolver`
//!   (only the test `PathResolver`), no whole-run lock lifecycle and no VMHost
//!   integration, so no product code opens a chain yet. A failed identity
//!   check on a read reaches the chain code as a generic I/O error, not as
//!   `ParentIdentityMismatch` (that variant is reported at open time).

use crate::storage_io::{HostFileStorage, LeaseOpenError, ReadLease, WriterOpenError};
use od_storage::io::{BackendError, StorageBackend, StorageCapabilities};
use std::fmt;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Why an export or repair attempt did not run.
#[derive(Debug)]
pub enum PolicyError {
    /// Export refused: a writer (this or another process, through any alias
    /// of the inode) holds the exclusive lock. Nothing was waited for.
    ExportBusy,
    /// Repair refused: another writer or read lease holds a lock on the file.
    /// Nothing was waited for and nothing was taken from the holder.
    RepairBusy,
    /// The path no longer names the file that was opened and locked, or the
    /// leased file's identity changed.
    IdentityMismatch,
    Io(io::Error),
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExportBusy => f.write_str("export refused: backing file is held by a writer"),
            Self::RepairBusy => {
                f.write_str("repair refused: backing file is held by a writer or a read lease")
            }
            Self::IdentityMismatch => f.write_str("backing path no longer names the opened file"),
            Self::Io(error) => write!(f, "backing file open failed: {error}"),
        }
    }
}

impl std::error::Error for PolicyError {}

impl PolicyError {
    fn from_export(error: LeaseOpenError) -> Self {
        match error {
            LeaseOpenError::LeaseBusy => Self::ExportBusy,
            LeaseOpenError::IdentityMismatch => Self::IdentityMismatch,
            LeaseOpenError::Io(error) => Self::Io(error),
        }
    }

    fn from_repair(error: WriterOpenError) -> Self {
        match error {
            WriterOpenError::WriterBusy => Self::RepairBusy,
            WriterOpenError::IdentityMismatch => Self::IdentityMismatch,
            WriterOpenError::Io(error) => Self::Io(error),
        }
    }
}

/// Runs `f` while holding a read lease on `path` for the whole call. Never
/// waits: a writer yields [`PolicyError::ExportBusy`] without calling `f`.
/// `f` gets a read-only backend; it cannot change the file.
pub fn export_with_lease<R>(
    path: &Path,
    f: impl FnOnce(&mut ReadLease) -> R,
) -> Result<R, PolicyError> {
    export_checking(path, path, f)
}

/// Like [`export_with_lease`], with the path used for the post-lock identity
/// check separated so tests can simulate a swap between `open` and `flock`.
fn export_checking<R>(
    open_path: &Path,
    check_path: &Path,
    f: impl FnOnce(&mut ReadLease) -> R,
) -> Result<R, PolicyError> {
    let mut lease =
        ReadLease::open_checking(open_path, check_path).map_err(PolicyError::from_export)?;
    Ok(f(&mut lease))
}

/// Runs `f` while holding the exclusive writer lock on `path` for the whole
/// call. Never waits and never displaces a live holder: any other writer or
/// read lease yields [`PolicyError::RepairBusy`] without calling `f`.
pub fn repair_with_writer<R>(
    path: &Path,
    f: impl FnOnce(&mut HostFileStorage) -> R,
) -> Result<R, PolicyError> {
    repair_checking(path, path, f)
}

/// See [`export_checking`].
fn repair_checking<R>(
    open_path: &Path,
    check_path: &Path,
    f: impl FnOnce(&mut HostFileStorage) -> R,
) -> Result<R, PolicyError> {
    let mut writer = HostFileStorage::open_exclusive_checking(open_path, check_path)
        .map_err(PolicyError::from_repair)?;
    Ok(f(&mut writer))
}

/// Failure of a [`ParentLease`] operation.
#[derive(Debug)]
pub enum ParentLeaseError {
    /// A writer holds the file (the lease could not be taken).
    Busy,
    /// The path no longer names the leased file, or names a different file
    /// than the caller expected.
    IdentityMismatch,
    Io(io::Error),
    Backend(BackendError),
}

impl fmt::Display for ParentLeaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str("parent file is held by a writer"),
            Self::IdentityMismatch => f.write_str("parent path no longer names the leased file"),
            Self::Io(error) => write!(f, "parent file check failed: {error}"),
            Self::Backend(error) => write!(f, "parent file read failed: {error:?}"),
        }
    }
}

impl std::error::Error for ParentLeaseError {}

/// A [`ReadLease`] bound to its path whose identity is verified on every
/// open and every use.
///
/// While it is alive no writer can open the file (the shared `flock` conflicts
/// with `LOCK_EX`, also through hard-link and symlink aliases). The type
/// offers read access only. It knows nothing about snapshot chains.
#[derive(Debug)]
pub struct ParentLease {
    lease: ReadLease,
    path: PathBuf,
}

impl ParentLease {
    /// Takes the lease on `path`; never waits.
    pub fn open(path: &Path) -> Result<Self, ParentLeaseError> {
        Self::open_inner(path, path, None)
    }

    /// Takes the lease and requires the leased file to be exactly
    /// `expected` (`(st_dev, st_ino)`, as returned by [`Self::file_identity`]).
    pub fn open_expecting(path: &Path, expected: (u64, u64)) -> Result<Self, ParentLeaseError> {
        Self::open_inner(path, path, Some(expected))
    }

    fn open_inner(
        open_path: &Path,
        check_path: &Path,
        expected: Option<(u64, u64)>,
    ) -> Result<Self, ParentLeaseError> {
        let lease = ReadLease::open_checking(open_path, check_path).map_err(|e| match e {
            LeaseOpenError::LeaseBusy => ParentLeaseError::Busy,
            LeaseOpenError::IdentityMismatch => ParentLeaseError::IdentityMismatch,
            LeaseOpenError::Io(error) => ParentLeaseError::Io(error),
        })?;
        if let Some(expected) = expected
            && lease.file_identity() != expected
        {
            return Err(ParentLeaseError::IdentityMismatch);
        }
        Ok(Self {
            lease,
            path: check_path.to_path_buf(),
        })
    }

    /// `(st_dev, st_ino)` of the leased file.
    pub fn file_identity(&self) -> (u64, u64) {
        self.lease.file_identity()
    }

    /// Checks that the path still names the leased file.
    pub fn verify_identity(&self) -> Result<(), ParentLeaseError> {
        match std::fs::metadata(&self.path) {
            Ok(current) if (current.dev(), current.ino()) == self.lease.file_identity() => Ok(()),
            Ok(_) => Err(ParentLeaseError::IdentityMismatch),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Err(ParentLeaseError::IdentityMismatch)
            }
            Err(error) => Err(ParentLeaseError::Io(error)),
        }
    }

    /// Size of the leased file after an identity check.
    pub fn size(&self) -> Result<u64, ParentLeaseError> {
        self.verify_identity()?;
        self.lease.size().map_err(ParentLeaseError::Backend)
    }

    /// Positioned read after an identity check.
    pub fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, ParentLeaseError> {
        self.verify_identity()?;
        self.lease
            .read_at(offset, buf)
            .map_err(ParentLeaseError::Backend)
    }
}

/// [`StorageBackend`] over a [`ParentLease`]: read-only, and the path
/// identity is re-checked on **every** `size` and `read_at`, so a parent that
/// was swapped after the lease was taken delivers no bytes (the call fails;
/// [`CheckedStorage`](od_storage::io::CheckedStorage) reports it as an I/O
/// error). Every mutating method is `Unsupported`. A snapshot chain keeps one
/// of these per ancestor for as long as the child is open.
#[derive(Debug)]
pub struct ParentLeaseStorage(ParentLease);

impl ParentLeaseStorage {
    pub fn new(lease: ParentLease) -> Self {
        Self(lease)
    }

    pub fn file_identity(&self) -> (u64, u64) {
        self.0.file_identity()
    }
}

fn lease_backend_error(error: ParentLeaseError) -> BackendError {
    match error {
        ParentLeaseError::Io(e) => BackendError::Io {
            kind: e.kind(),
            os_error: e.raw_os_error(),
        },
        ParentLeaseError::Backend(e) => e,
        // Busy cannot happen on an already held lease; a changed identity is
        // reported as a failed call, never as data of the other file.
        ParentLeaseError::Busy | ParentLeaseError::IdentityMismatch => BackendError::Io {
            kind: io::ErrorKind::Other,
            os_error: None,
        },
    }
}

impl StorageBackend for ParentLeaseStorage {
    fn capabilities(&self) -> StorageCapabilities {
        StorageCapabilities::read_only(crate::storage_io::HOST_MAX_LEN)
    }

    fn size(&self) -> Result<u64, BackendError> {
        self.0.size().map_err(lease_backend_error)
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, BackendError> {
        self.0.read_at(offset, buf).map_err(lease_backend_error)
    }

    fn write_at(&mut self, _offset: u64, _buf: &[u8]) -> Result<usize, BackendError> {
        Err(BackendError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage_io::process_spawn_guard;
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "od-host-storage-policy-{}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&dir).expect("create temp dir");
            Self(dir)
        }

        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }

        fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.path(name);
            std::fs::write(&path, bytes).expect("seed file");
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const BYTES: &[u8] = b"0123456789abcdef";
    /// Generous bound: a non-blocking call returns in microseconds; this only
    /// catches a call that waits.
    const NO_WAIT: Duration = Duration::from_secs(2);

    #[test]
    fn export_during_active_writer_is_busy_without_waiting_or_calling_f() {
        let _serial = process_spawn_guard();
        let dir = TempDir::new();
        let path = dir.file("a.raw", BYTES);
        let writer = HostFileStorage::open_read_write_exclusive(&path).expect("writer");
        let called = Cell::new(false);
        let started = Instant::now();
        let result = export_with_lease(&path, |_| called.set(true));
        let elapsed = started.elapsed();
        assert!(matches!(result, Err(PolicyError::ExportBusy)), "{result:?}");
        assert!(!called.get(), "closure must not run while busy");
        assert!(elapsed < NO_WAIT, "export waited: {elapsed:?}");
        drop(writer);
        let value = export_with_lease(&path, |lease| lease.size().ok()).expect("export after drop");
        assert_eq!(value, Some(BYTES.len() as u64));
    }

    #[test]
    fn repair_during_active_lease_or_writer_is_busy_without_calling_f() {
        let _serial = process_spawn_guard();
        let dir = TempDir::new();
        let path = dir.file("a.raw", BYTES);
        let called = Cell::new(false);

        let lease = ReadLease::open(&path).expect("lease");
        let started = Instant::now();
        let result = repair_with_writer(&path, |_| called.set(true));
        assert!(matches!(result, Err(PolicyError::RepairBusy)), "{result:?}");
        assert!(started.elapsed() < NO_WAIT);
        drop(lease);

        let writer = HostFileStorage::open_read_write_exclusive(&path).expect("writer");
        let started = Instant::now();
        let result = repair_with_writer(&path, |_| called.set(true));
        assert!(matches!(result, Err(PolicyError::RepairBusy)), "{result:?}");
        assert!(started.elapsed() < NO_WAIT);
        assert!(!called.get(), "closure must not run while busy");
        drop(writer);

        repair_with_writer(&path, |_| called.set(true)).expect("repair after release");
        assert!(called.get());
    }

    #[test]
    fn export_lease_is_held_for_the_whole_closure_and_repair_writer_blocks_export() {
        let _serial = process_spawn_guard();
        let dir = TempDir::new();
        let path = dir.file("a.raw", BYTES);
        // While export runs, repair is busy.
        let during = export_with_lease(&path, |_| repair_with_writer(&path, |_| ()))
            .expect("export")
            .expect_err("repair during export");
        assert!(matches!(during, PolicyError::RepairBusy), "{during:?}");
        // While repair runs, export is busy.
        let during = repair_with_writer(&path, |_| export_with_lease(&path, |_| ()))
            .expect("repair")
            .expect_err("export during repair");
        assert!(matches!(during, PolicyError::ExportBusy), "{during:?}");
    }

    // ---- real second process ----

    const CHILD_PATH_ENV: &str = "OD_POLICY_CHILD_PATH";
    const MARK: &str = "OD_POLICY_RESULT:";

    /// Acts as the second process when `OD_POLICY_CHILD_PATH` is set: takes a
    /// read lease, reports, then holds it until its stdin closes or it is
    /// killed. A no-op otherwise.
    #[test]
    fn policy_probe_child() {
        use std::io::{BufRead, Write};
        let Ok(path) = std::env::var(CHILD_PATH_ENV) else {
            return;
        };
        let lease = ReadLease::open(Path::new(&path));
        let tag = if lease.is_ok() { "LEASED" } else { "ERROR" };
        let mut out = io::stdout().lock();
        let _ = writeln!(out, "{MARK}{tag}");
        let _ = out.flush();
        drop(out);
        if lease.is_ok() {
            let mut line = String::new();
            let _ = io::stdin().lock().read_line(&mut line);
        }
    }

    #[test]
    fn repair_is_busy_while_another_process_leases_and_works_after_sigkill() {
        use std::io::BufRead;
        let _serial = process_spawn_guard();
        let dir = TempDir::new();
        let path = dir.file("a.raw", BYTES);
        let exe = std::env::current_exe().expect("current exe");
        let mut child = std::process::Command::new(exe)
            .args([
                "--exact",
                "storage_policy::tests::policy_probe_child",
                "--nocapture",
            ])
            .env(CHILD_PATH_ENV, &path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn child");
        let stdout = child.stdout.take().expect("child stdout");
        let mut tag = None;
        for line in io::BufReader::new(stdout).lines() {
            let line = line.expect("child line");
            if let Some(found) = line.strip_prefix(MARK) {
                tag = Some(found.to_string());
                break;
            }
        }
        assert_eq!(tag.as_deref(), Some("LEASED"));

        let called = Cell::new(false);
        let started = Instant::now();
        let result = repair_with_writer(&path, |_| called.set(true));
        assert!(matches!(result, Err(PolicyError::RepairBusy)), "{result:?}");
        assert!(started.elapsed() < NO_WAIT);
        assert!(!called.get());
        // Readers coexist with the foreign lease.
        export_with_lease(&path, |_| ()).expect("shared export lease");

        child.kill().expect("SIGKILL child");
        child.wait().expect("reap child");
        repair_with_writer(&path, |_| called.set(true))
            .expect("kernel released the lease after SIGKILL");
        assert!(called.get());
    }

    #[test]
    fn hard_link_and_symlink_aliases_share_the_lease_conflict() {
        let _serial = process_spawn_guard();
        let dir = TempDir::new();
        let path = dir.file("a.raw", BYTES);
        let hard = dir.path("hard.raw");
        std::fs::hard_link(&path, &hard).expect("hard link");
        let sym = dir.path("sym.raw");
        std::os::unix::fs::symlink(&path, &sym).expect("symlink");

        for (leased, other) in [(&path, &hard), (&hard, &sym), (&sym, &path)] {
            let parent = ParentLease::open(leased).expect("parent lease");
            assert!(matches!(
                HostFileStorage::open_read_write_exclusive(other),
                Err(WriterOpenError::WriterBusy)
            ));
            assert!(matches!(
                repair_with_writer(other, |_| ()),
                Err(PolicyError::RepairBusy)
            ));
            drop(parent);
            repair_with_writer(other, |_| ()).expect("free after drop");
        }

        // Reverse direction: a writer via an alias blocks the lease.
        let writer = HostFileStorage::open_read_write_exclusive(&hard).expect("writer");
        assert!(matches!(
            ParentLease::open(&sym),
            Err(ParentLeaseError::Busy)
        ));
        assert!(matches!(
            export_with_lease(&path, |_| ()),
            Err(PolicyError::ExportBusy)
        ));
        drop(writer);
    }

    #[test]
    fn swap_between_check_and_use_is_identity_mismatch_and_never_writes_foreign_file() {
        let _serial = process_spawn_guard();
        let dir = TempDir::new();
        let path = dir.file("a.raw", BYTES);
        let other = dir.file("b.raw", b"FOREIGN-FOREIGN!");

        // Open-time hook: the descriptor names a.raw, the path names b.raw.
        let called = Cell::new(false);
        let result = repair_checking(&path, &other, |_| called.set(true));
        assert!(
            matches!(result, Err(PolicyError::IdentityMismatch)),
            "{result:?}"
        );
        let result = export_checking(&path, &other, |_| called.set(true));
        assert!(
            matches!(result, Err(PolicyError::IdentityMismatch)),
            "{result:?}"
        );
        assert!(!called.get(), "closure must not run on identity mismatch");
        assert!(matches!(
            ParentLease::open_inner(&path, &other, None),
            Err(ParentLeaseError::IdentityMismatch)
        ));
        // Refused attempts left no lock behind.
        repair_with_writer(&path, |_| ()).expect("no lock left behind");

        // Use-time swap: the path is replaced while the writer runs.
        let outcome = repair_with_writer(&path, |writer| {
            let swapped = dir.file("swap.raw", b"NEW-FILE-NEW-FILE");
            std::fs::rename(&swapped, &path).expect("replace path");
            writer.write_at(0, b"XXXX")
        })
        .expect("repair opened before the swap");
        assert!(
            matches!(outcome, Err(BackendError::WriterIdentityLost)),
            "{outcome:?}"
        );
        assert_eq!(
            std::fs::read(&path).expect("read replacement"),
            b"NEW-FILE-NEW-FILE",
            "replacement file must not be written"
        );
        assert_eq!(
            std::fs::read(&other).expect("read other"),
            b"FOREIGN-FOREIGN!"
        );
    }

    #[test]
    fn parent_lease_verifies_identity_on_every_use() {
        let _serial = process_spawn_guard();
        let dir = TempDir::new();
        let path = dir.file("a.raw", BYTES);
        let parent = ParentLease::open(&path).expect("parent lease");
        let mut buf = [0_u8; 4];
        assert_eq!(parent.size().expect("size"), BYTES.len() as u64);
        assert_eq!(parent.read_at(2, &mut buf).expect("read"), 4);
        assert_eq!(&buf, b"2345");
        let identity = parent.file_identity();

        // Expected identity is enforced at open time.
        drop(parent);
        let wrong = (identity.0, identity.1.wrapping_add(1));
        assert!(matches!(
            ParentLease::open_expecting(&path, wrong),
            Err(ParentLeaseError::IdentityMismatch)
        ));
        let parent = ParentLease::open_expecting(&path, identity).expect("expected identity");

        // Replace the path: every later use refuses instead of reading the
        // new file.
        let swapped = dir.file("swap.raw", b"NEW-FILE-NEW-FILE");
        std::fs::rename(&swapped, &path).expect("replace path");
        let mut buf = [0_u8; 4];
        assert!(matches!(
            parent.read_at(0, &mut buf),
            Err(ParentLeaseError::IdentityMismatch)
        ));
        assert_eq!(buf, [0; 4], "no bytes of the foreign file were delivered");
        assert!(matches!(
            parent.size(),
            Err(ParentLeaseError::IdentityMismatch)
        ));
        assert!(matches!(
            parent.verify_identity(),
            Err(ParentLeaseError::IdentityMismatch)
        ));

        // Removed path is a mismatch too.
        std::fs::remove_file(&path).expect("remove path");
        assert!(matches!(
            parent.verify_identity(),
            Err(ParentLeaseError::IdentityMismatch)
        ));
    }

    /// Test-only [`ParentResolver`] over real files: uid to path, optionally
    /// with the recorded `(st_dev, st_ino)`. Every parent is taken through
    /// [`ParentLease`] (shared lock, identity re-checked on every use).
    #[cfg(target_os = "macos")]
    mod chain_support {
        use super::super::*;
        use od_storage::cow_snapshot::{ParentRef, ParentResolver, ResolveError};
        use od_storage::io::CheckedStorage;
        use std::collections::BTreeMap;

        pub(super) struct PathResolver {
            pub(super) entries: BTreeMap<u64, (PathBuf, Option<(u64, u64)>)>,
        }

        impl ParentResolver for PathResolver {
            type Backend = ParentLeaseStorage;

            fn open_parent(
                &mut self,
                parent: &ParentRef,
            ) -> Result<CheckedStorage<ParentLeaseStorage>, ResolveError> {
                let (path, expected) = self
                    .entries
                    .get(&parent.file_uid)
                    .ok_or(ResolveError::Missing)?;
                let lease = match expected {
                    Some(identity) => ParentLease::open_expecting(path, *identity),
                    None => ParentLease::open(path),
                }
                .map_err(|e| match e {
                    ParentLeaseError::Busy => ResolveError::Busy,
                    ParentLeaseError::IdentityMismatch => ResolveError::IdentityMismatch,
                    ParentLeaseError::Io(e) if e.kind() == io::ErrorKind::NotFound => {
                        ResolveError::Missing
                    }
                    ParentLeaseError::Io(e) => ResolveError::Io(e.kind()),
                    ParentLeaseError::Backend(_) => ResolveError::Io(io::ErrorKind::Other),
                })?;
                CheckedStorage::new(ParentLeaseStorage::new(lease)).map_err(ResolveError::Storage)
            }
        }
    }
    #[cfg(target_os = "macos")]
    use chain_support::PathResolver;

    /// S9-14 over real files: a child reads its ancestors only through
    /// `ParentLease`, a parent cannot be written while the child is open, and
    /// a swapped parent is refused. macOS only: the CoW commit needs durable
    /// file flushes, which the host backend declares only there.
    #[cfg(target_os = "macos")]
    #[test]
    fn snapshot_chain_reads_through_parent_leases_and_refuses_a_swapped_parent() {
        use od_storage::cow_snapshot::{SnapError, SnapFile};
        use od_storage::io::CheckedStorage;
        use std::collections::BTreeMap;

        let _serial = process_spawn_guard();
        let dir = TempDir::new();
        let base_path = dir.path("base.cow");
        let child_path = dir.path("child.cow");
        let file = |p: &Path| {
            CheckedStorage::new(HostFileStorage::create_new_exclusive(p, 0).expect("create"))
                .expect("checked")
        };

        // Base: two blocks of data and one snapshot.
        let (mut base, _) = SnapFile::create_base(file(&base_path), 1).expect("base");
        base.write_at(0, &[0xAA; 1024]).expect("base write");
        let (id, _) = base.create_snapshot().expect("snapshot");
        let pref = base.snapshot_ref(id).expect("ref");
        drop(base);
        let identity = ParentLease::open(&base_path)
            .expect("lease")
            .file_identity();
        let mut resolver = PathResolver {
            entries: BTreeMap::from([(1, (base_path.clone(), Some(identity)))]),
        };

        let (mut child, _) =
            SnapFile::create_child(file(&child_path), &mut resolver, &pref, 2).expect("child");
        child.write_at(100, &[1, 2, 3]).expect("child write");
        let mut expected = vec![0xAAu8; 1024];
        expected[100..103].copy_from_slice(&[1, 2, 3]);
        let mut got = vec![0u8; 1024];
        child.read_at(0, &mut got).expect("child read");
        assert_eq!(got, expected);
        // The open child holds the parent's read lease: no writer can start.
        assert!(matches!(
            HostFileStorage::open_read_write_exclusive(&base_path),
            Err(WriterOpenError::WriterBusy)
        ));
        drop(child);

        let reopen = |resolver: &mut PathResolver| {
            let storage = CheckedStorage::new(
                HostFileStorage::open_read_write_exclusive(&child_path).expect("writer"),
            )
            .expect("checked");
            SnapFile::open(storage, resolver).map(|(f, _)| f)
        };
        let child = reopen(&mut resolver).expect("child reopens");
        let mut got = vec![0u8; 1024];
        child.read_at(0, &mut got).expect("read after reopen");
        assert_eq!(got, expected);
        drop(child);

        // Replace the parent path by a byte-identical copy (new inode).
        let copy = dir.path("copy.cow");
        std::fs::copy(&base_path, &copy).expect("copy");
        std::fs::rename(&copy, &base_path).expect("swap");
        assert!(matches!(
            reopen(&mut resolver).err(),
            Some(SnapError::ParentIdentityMismatch)
        ));
        // Without a recorded host identity only the content identity (file
        // uid, snapshot record, map CRC) decides: the identical copy passes,
        // a different file does not.
        resolver.entries.insert(1, (base_path.clone(), None));
        assert!(reopen(&mut resolver).is_ok());
        let other = dir.path("other.cow");
        let (mut o, _) = SnapFile::create_base(file(&other), 1).expect("other");
        o.write_at(0, &[0xBB; 1024]).expect("other write");
        o.create_snapshot().expect("other snapshot");
        drop(o);
        std::fs::rename(&other, &base_path).expect("swap for another file");
        assert!(matches!(
            reopen(&mut resolver).err(),
            Some(SnapError::ParentIdentityMismatch)
        ));
        // A removed parent is missing.
        std::fs::remove_file(&base_path).expect("remove parent");
        assert!(matches!(
            reopen(&mut resolver).err(),
            Some(SnapError::ParentMissing)
        ));
    }

    // ---- S9-15 / S9-16 over real files (macOS: durable flush) ----

    #[cfg(target_os = "macos")]
    mod chain_files {
        use super::*;
        use od_storage::cow_snapshot::{ParentRef, SnapError, SnapFile};
        use od_storage::io::CheckedStorage;
        use std::collections::BTreeMap;

        pub(super) type Host = SnapFile<HostFileStorage>;

        pub(super) fn created(path: &Path) -> CheckedStorage<HostFileStorage> {
            CheckedStorage::new(HostFileStorage::create_new_exclusive(path, 0).expect("create"))
                .expect("checked")
        }

        pub(super) fn writer(
            path: &Path,
        ) -> Result<CheckedStorage<HostFileStorage>, WriterOpenError> {
            HostFileStorage::open_read_write_exclusive(path)
                .map(|s| CheckedStorage::new(s).expect("checked"))
        }

        pub(super) fn identity(path: &Path) -> (u64, u64) {
            ParentLease::open(path).expect("lease").file_identity()
        }

        pub(super) fn open_child(path: &Path, r: &mut PathResolver) -> Result<Host, SnapError> {
            let storage = writer(path).map_err(|_| SnapError::ParentBusy)?;
            SnapFile::open(storage, r).map(|(f, _)| f)
        }

        /// Why a host-level delete or compaction did not run or failed.
        #[derive(Debug)]
        pub(super) enum Exclusive {
            /// Another holder (a writer or a child's parent lease) owns the
            /// file; nothing was opened for writing and nothing was written.
            Busy,
            Other,
            Snap(SnapError),
        }

        /// The exclusive writer lock is the precondition of every mutation of
        /// a file's reachability: opening the file for writing takes it, and a
        /// held lease or writer makes this `Busy` before any byte is read for
        /// writing.
        fn exclusive(path: &Path, r: &mut PathResolver) -> Result<Host, Exclusive> {
            let storage = writer(path).map_err(|e| match e {
                WriterOpenError::WriterBusy => Exclusive::Busy,
                _ => Exclusive::Other,
            })?;
            SnapFile::open(storage, r)
                .map(|(f, _)| f)
                .map_err(Exclusive::Snap)
        }

        /// Deletes the name of snapshot `id` of `path` under the exclusive
        /// writer lock.
        pub(super) fn delete_snapshot_exclusive(
            path: &Path,
            id: u64,
            r: &mut PathResolver,
        ) -> Result<(), Exclusive> {
            let mut f = exclusive(path, r)?;
            f.delete_snapshot(id).map(|_| ()).map_err(Exclusive::Snap)
        }

        /// Copy-compacts `path` into the new file `target` under the exclusive
        /// writer lock on `path`. The target path is only created after the
        /// lock is held, so a busy source leaves no file behind. The source is
        /// only read; swapping it for the target is not done here.
        pub(super) fn compact_exclusive(
            path: &Path,
            target: &Path,
            new_uid: u64,
            r: &mut PathResolver,
        ) -> Result<(), Exclusive> {
            let source = exclusive(path, r)?;
            source
                .compact_to(created(target), new_uid)
                .map(|_| ())
                .map_err(Exclusive::Snap)
        }

        pub(super) fn read_all(f: &Host) -> Vec<u8> {
            let mut v = vec![0u8; f.disk_size() as usize];
            f.read_at(0, &mut v).expect("read");
            v
        }

        /// base(uid 1, 0xAA x 1024, snapshot 1) <- mid(uid 2, bytes 100..103,
        /// snapshot 1) <- top(uid 3). All files closed. Returns paths, the
        /// resolver (host identities recorded) and the refs.
        pub(super) struct Chain {
            pub base: PathBuf,
            pub mid: PathBuf,
            pub top: PathBuf,
            pub resolver: PathResolver,
            pub mid_ref: ParentRef,
            pub expected: Vec<u8>,
        }

        pub(super) fn chain(dir: &TempDir) -> Chain {
            let (base, mid, top) = (
                dir.path("base.cow"),
                dir.path("mid.cow"),
                dir.path("top.cow"),
            );
            let (mut b, _) = SnapFile::create_base(created(&base), 1).expect("base");
            b.write_at(0, &[0xAA; 1024]).expect("base write");
            let (id, _) = b.create_snapshot().expect("base snapshot");
            let base_ref = b.snapshot_ref(id).expect("base ref");
            drop(b);
            let mut resolver = PathResolver {
                entries: BTreeMap::from([(1, (base.clone(), Some(identity(&base))))]),
            };
            let (mut m, _) =
                SnapFile::create_child(created(&mid), &mut resolver, &base_ref, 2).expect("mid");
            m.write_at(100, &[1, 2, 3]).expect("mid write");
            let (id, _) = m.create_snapshot().expect("mid snapshot");
            let mid_ref = m.snapshot_ref(id).expect("mid ref");
            drop(m);
            resolver
                .entries
                .insert(2, (mid.clone(), Some(identity(&mid))));
            let (t, _) =
                SnapFile::create_child(created(&top), &mut resolver, &mid_ref, 3).expect("top");
            drop(t);
            let mut expected = vec![0xAAu8; 1024];
            expected[100..103].copy_from_slice(&[1, 2, 3]);
            Chain {
                base,
                mid,
                top,
                resolver,
                mid_ref,
                expected,
            }
        }

        /// A standalone file of `uid` with `fill` data and one snapshot.
        pub(super) fn foreign(path: &Path, uid: u64, fill: u8) {
            let (mut f, _) = SnapFile::create_base(created(path), uid).expect("foreign");
            f.write_at(0, &[fill; 1024]).expect("foreign write");
            f.create_snapshot().expect("foreign snapshot");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn chain_is_validated_at_every_level_before_the_writable_open_writes() {
        use chain_files::*;
        use od_storage::cow_snapshot::SnapError;
        let _serial = process_spawn_guard();
        let dir = TempDir::new();
        let mut c = chain(&dir);
        let top_bytes = std::fs::read(&c.top).expect("top bytes");
        assert_eq!(
            read_all(&open_child(&c.top, &mut c.resolver).expect("opens")),
            c.expected
        );

        // Wrong parent ID, hash and generation, one level up (the nearest
        // parent): other uid, same uid with other content.
        let keep_mid = dir.path("mid.keep");
        std::fs::rename(&c.mid, &keep_mid).expect("move mid");
        foreign(&c.mid, 99, 0xCC);
        c.resolver.entries.insert(2, (c.mid.clone(), None));
        assert!(matches!(
            open_child(&c.top, &mut c.resolver),
            Err(SnapError::ParentIdentityMismatch)
        ));
        std::fs::remove_file(&c.mid).expect("rm foreign");
        foreign(&c.mid, 2, 0xCC);
        assert!(matches!(
            open_child(&c.top, &mut c.resolver),
            Err(SnapError::ParentIdentityMismatch)
        ));
        std::fs::remove_file(&c.mid).expect("rm foreign");
        std::fs::rename(&keep_mid, &c.mid).expect("restore mid");
        c.resolver
            .entries
            .insert(2, (c.mid.clone(), Some(identity(&c.mid))));
        assert!(open_child(&c.top, &mut c.resolver).is_ok());

        // The same one level further up: the nearest parent is intact but the
        // base is another file (uid 1, other content) or has another uid.
        let keep_base = dir.path("base.keep");
        std::fs::rename(&c.base, &keep_base).expect("move base");
        c.resolver.entries.insert(1, (c.base.clone(), None));
        for (uid, fill) in [(1, 0xDD), (55, 0xAA)] {
            foreign(&c.base, uid, fill);
            assert!(
                matches!(
                    open_child(&c.top, &mut c.resolver),
                    Err(SnapError::ParentIdentityMismatch)
                ),
                "base uid {uid}"
            );
            std::fs::remove_file(&c.base).expect("rm foreign");
        }
        // Base gone: missing, not a read of anything.
        assert!(matches!(
            open_child(&c.top, &mut c.resolver),
            Err(SnapError::ParentMissing)
        ));
        // No refusal wrote to the child or left a lock behind.
        assert_eq!(std::fs::read(&c.top).expect("top after"), top_bytes);
        drop(writer(&c.top).expect("top is not locked"));
        std::fs::rename(&keep_base, &c.base).expect("restore base");
        c.resolver
            .entries
            .insert(1, (c.base.clone(), Some(identity(&c.base))));
        assert!(open_child(&c.top, &mut c.resolver).is_ok());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn symlink_alias_and_swap_between_validation_and_first_read_are_refused() {
        use chain_files::*;
        use od_storage::cow_snapshot::SnapError;
        let _serial = process_spawn_guard();
        let dir = TempDir::new();
        let mut c = chain(&dir);

        // A symlink to the real parent is the same inode: accepted, and the
        // writer lock still conflicts through the alias.
        let alias = dir.path("alias.cow");
        std::os::unix::fs::symlink(&c.base, &alias).expect("symlink");
        c.resolver
            .entries
            .insert(1, (alias.clone(), Some(identity(&c.base))));
        let open = open_child(&c.top, &mut c.resolver).expect("alias to the same inode");
        assert!(matches!(
            HostFileStorage::open_read_write_exclusive(&c.base),
            Err(WriterOpenError::WriterBusy)
        ));

        // TOCTOU: the chain is validated and leased; now the alias is pointed
        // at a byte-identical copy (other inode). The first read through the
        // chain delivers no bytes of it.
        let copy = dir.path("copy.cow");
        std::fs::copy(&c.base, &copy).expect("copy");
        let alias_tmp = dir.path("alias.tmp");
        std::os::unix::fs::symlink(&copy, &alias_tmp).expect("symlink");
        std::fs::rename(&alias_tmp, &alias).expect("retarget alias");
        let mut buf = vec![0u8; 16];
        // Block 1 (offset 512) is a hole of top and mid: it resolves in the
        // base.
        let read = open.read_at(512, &mut buf);
        assert!(matches!(read, Err(SnapError::Cow(_))), "{read:?}");
        assert_eq!(buf, vec![0u8; 16], "no byte of the swapped file delivered");
        drop(open);

        // A symlink to a different inode than the recorded one is refused at
        // open already, before anything is read.
        assert!(matches!(
            open_child(&c.top, &mut c.resolver),
            Err(SnapError::ParentIdentityMismatch)
        ));

        // A plain replacement of the real path after validation behaves the
        // same (rename over the parent).
        c.resolver
            .entries
            .insert(1, (c.base.clone(), Some(identity(&c.base))));
        let open = open_child(&c.top, &mut c.resolver).expect("real path");
        std::fs::rename(&copy, &c.base).expect("replace parent");
        let mut buf = vec![0u8; 16];
        assert!(open.read_at(512, &mut buf).is_err());
        assert_eq!(buf, vec![0u8; 16]);
        drop(open);
        assert!(matches!(
            open_child(&c.top, &mut c.resolver),
            Err(SnapError::ParentIdentityMismatch)
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn open_children_hold_parent_leases_and_a_live_writer_refuses_a_child() {
        use chain_files::*;
        use od_storage::cow_snapshot::SnapError;
        let _serial = process_spawn_guard();
        let dir = TempDir::new();
        let mut c = chain(&dir);
        // Second child of the same parent snapshot.
        let top2 = dir.path("top2.cow");
        let (t2, _) = od_storage::cow_snapshot::SnapFile::create_child(
            created(&top2),
            &mut c.resolver,
            &c.mid_ref,
            4,
        )
        .expect("second child");
        drop(t2);

        // Two children open at once: shared leases coexist; no writer may
        // start on mid or base while any of them is open.
        let a = open_child(&c.top, &mut c.resolver).expect("first child");
        let b = open_child(&top2, &mut c.resolver).expect("second child");
        for p in [&c.mid, &c.base] {
            assert!(matches!(
                HostFileStorage::open_read_write_exclusive(p),
                Err(WriterOpenError::WriterBusy)
            ));
        }
        drop(a);
        assert!(matches!(
            HostFileStorage::open_read_write_exclusive(&c.mid),
            Err(WriterOpenError::WriterBusy)
        ));
        drop(b);
        drop(HostFileStorage::open_read_write_exclusive(&c.mid).expect("free after both close"));

        // Live-writer refusal: a writer holds the parent, a child cannot open.
        let top_bytes = std::fs::read(&c.top).expect("top bytes");
        let live = HostFileStorage::open_read_write_exclusive(&c.mid).expect("writer");
        assert!(matches!(
            open_child(&c.top, &mut c.resolver),
            Err(SnapError::ParentBusy)
        ));
        assert!(matches!(
            od_storage::cow_snapshot::SnapFile::create_child(
                created(&dir.path("late.cow")),
                &mut c.resolver,
                &c.mid_ref,
                5
            )
            .err(),
            Some(SnapError::ParentBusy)
        ));
        assert_eq!(std::fs::read(&c.top).expect("top after"), top_bytes);
        drop(live);
        assert!(open_child(&c.top, &mut c.resolver).is_ok());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn branch_from_a_closed_file_over_leases_and_live_source_is_busy() {
        use chain_files::*;
        use od_storage::cow_snapshot::{SnapError, SnapFile};
        use od_storage::io::CheckedStorage;
        let _serial = process_spawn_guard();
        let dir = TempDir::new();
        let mut c = chain(&dir);
        let before: Vec<Vec<u8>> = [&c.base, &c.mid, &c.top]
            .iter()
            .map(|p| std::fs::read(p).expect("bytes"))
            .collect();

        // Source open for writing: a branch from it is a live-writer refusal.
        let mut source = open_child(&c.top, &mut c.resolver).expect("source");
        source.write_at(0, &[9, 9]).expect("source write");
        let (sid, _) = source.create_snapshot().expect("source snapshot");
        c.resolver
            .entries
            .insert(3, (c.top.clone(), Some(identity_of_open(&c.top))));
        let refused = SnapFile::branch_from(
            created(&dir.path("b0.cow")),
            &mut c.resolver,
            &source,
            sid,
            7,
        );
        assert!(matches!(refused.err(), Some(SnapError::ParentBusy)));
        assert_eq!(
            std::fs::metadata(dir.path("b0.cow")).expect("b0").len(),
            0,
            "a refused branch wrote nothing"
        );
        let mut after_write = vec![0u8; 1024];
        source.read_at(0, &mut after_write).expect("read source");
        let rolled_back_to = after_write.clone();
        drop(source);

        // Closed source: reference read through a read lease, then a branch.
        let lease = ParentLeaseStorage::new(ParentLease::open(&c.top).expect("read lease"));
        let sref =
            SnapFile::inspect_snapshot_ref(CheckedStorage::new(lease).expect("checked"), sid)
                .expect("snapshot ref");
        let (mut branch, _) =
            SnapFile::create_child(created(&dir.path("b1.cow")), &mut c.resolver, &sref, 7)
                .expect("branch");
        assert_eq!(read_all(&branch), rolled_back_to);
        branch.write_at(500, &[5; 4]).expect("branch write");
        // While the branch is open its source cannot be written.
        assert!(matches!(
            HostFileStorage::open_read_write_exclusive(&c.top),
            Err(WriterOpenError::WriterBusy)
        ));
        drop(branch);
        // The base and mid files are byte-for-byte what they were; the source
        // changed only by its own write and snapshot, never by the branch.
        assert_eq!(std::fs::read(&c.base).expect("base"), before[0]);
        assert_eq!(std::fs::read(&c.mid).expect("mid"), before[1]);
    }

    /// S9-17 on real files: delete and compaction need the exclusive writer
    /// lock, an open child makes both `Busy` without a write, compaction
    /// leaves the source untouched and does not reconnect children.
    #[cfg(target_os = "macos")]
    #[test]
    fn snapshot_delete_and_compaction_need_the_exclusive_lock_and_do_not_reconnect_children() {
        use chain_files::*;
        use od_storage::cow_snapshot::{SnapError, SnapFile};
        let _serial = process_spawn_guard();
        let dir = TempDir::new();
        let mut c = chain(&dir);
        let new_base = dir.path("base2.cow");
        let bytes = |p: &Path| std::fs::read(p).expect("bytes");
        let (base_before, mid_before) = (bytes(&c.base), bytes(&c.mid));

        // A child is open: it leases mid and base. Nothing may write them.
        let child = open_child(&c.top, &mut c.resolver).expect("child");
        for p in [&c.base, &c.mid] {
            assert!(matches!(
                delete_snapshot_exclusive(p, 1, &mut c.resolver),
                Err(Exclusive::Busy)
            ));
            assert!(matches!(
                compact_exclusive(p, &new_base, 90, &mut c.resolver),
                Err(Exclusive::Busy)
            ));
        }
        assert!(!new_base.exists(), "a busy compaction created its target");
        assert_eq!(bytes(&c.base), base_before, "base written while leased");
        assert_eq!(bytes(&c.mid), mid_before, "mid written while leased");
        drop(child);

        // Once free, a delete of an unknown snapshot is a typed refusal that
        // writes nothing.
        assert!(matches!(
            delete_snapshot_exclusive(&c.base, 99, &mut c.resolver),
            Err(Exclusive::Snap(SnapError::SnapshotMissing { id: 99 }))
        ));
        assert_eq!(bytes(&c.base), base_before, "a refused delete wrote");

        // Closed: a second, unreferenced snapshot of base can be added and
        // deleted; the delete takes the exclusive lock itself.
        let mut b = open_child(&c.base, &mut c.resolver).expect("base writer");
        let (extra, _) = b.create_snapshot().expect("extra snapshot");
        drop(b);
        delete_snapshot_exclusive(&c.base, extra, &mut c.resolver).expect("delete");
        let base_before = bytes(&c.base);

        // Compaction under the lock writes only the new file.
        compact_exclusive(&c.base, &new_base, 90, &mut c.resolver).expect("compact");
        assert_eq!(bytes(&c.base), base_before, "compaction changed the source");
        let storage = writer(&new_base).expect("target writer");
        let (t, _) = SnapFile::open(storage, &mut c.resolver).expect("target opens");
        assert_eq!(t.file_uid(), 90);
        assert_eq!(t.snapshot_ids(), vec![1]);
        assert_eq!(read_all(&t), vec![0xAAu8; 1024]);
        drop(t);

        // The child of the old base is not reconnected to the compacted one.
        c.resolver
            .entries
            .insert(1, (new_base.clone(), Some(identity(&new_base))));
        assert!(matches!(
            open_child(&c.mid, &mut c.resolver).err(),
            Some(SnapError::ParentIdentityMismatch)
        ));
        c.resolver
            .entries
            .insert(1, (c.base.clone(), Some(identity(&c.base))));
        assert!(open_child(&c.mid, &mut c.resolver).is_ok());

        // Known limit: the file cannot see closed children. Deleting the
        // snapshot a closed child uses succeeds, and the child then refuses to
        // open (a catalogue must prevent this).
        delete_snapshot_exclusive(&c.base, 1, &mut c.resolver).expect("delete referenced");
        assert!(matches!(
            open_child(&c.mid, &mut c.resolver).err(),
            Some(SnapError::SnapshotMissing { id: 1 })
        ));
    }

    /// `(dev, ino)` of a file that may be locked by a writer (no lease).
    #[cfg(target_os = "macos")]
    fn identity_of_open(path: &Path) -> (u64, u64) {
        let m = std::fs::metadata(path).expect("metadata");
        (m.dev(), m.ino())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn incomplete_copy_bundle_is_refused_without_reading() {
        use chain_files::*;
        use od_storage::cow_snapshot::SnapError;
        use std::collections::BTreeMap;
        let _serial = process_spawn_guard();
        let dir = TempDir::new();
        let c = chain(&dir);
        // Bundle copy: top and mid copied, base not. No recorded identity (a
        // copy has other inodes), so only the files' own content decides.
        let bundle = TempDir::new();
        std::fs::copy(&c.top, bundle.path("top.cow")).expect("copy top");
        std::fs::copy(&c.mid, bundle.path("mid.cow")).expect("copy mid");
        let mut r = PathResolver {
            entries: BTreeMap::from([
                (1, (bundle.path("base.cow"), None)),
                (2, (bundle.path("mid.cow"), None)),
            ]),
        };
        let before = std::fs::read(bundle.path("top.cow")).expect("bytes");
        assert!(matches!(
            open_child(&bundle.path("top.cow"), &mut r),
            Err(SnapError::ParentMissing)
        ));
        // A truncated copy of the base (copy interrupted) is not a parent.
        let full = std::fs::read(&c.base).expect("base bytes");
        for keep in [0usize, 100, full.len() / 2, full.len() - 1] {
            std::fs::write(bundle.path("base.cow"), &full[..keep]).expect("partial");
            let err = open_child(&bundle.path("top.cow"), &mut r).err();
            assert!(
                matches!(
                    err,
                    Some(SnapError::ParentCorrupt(_) | SnapError::ParentDamaged)
                ),
                "keep={keep}: {err:?}"
            );
        }
        assert_eq!(
            std::fs::read(bundle.path("top.cow")).expect("after"),
            before
        );
        // The complete bundle opens.
        std::fs::copy(&c.base, bundle.path("base.cow")).expect("copy base");
        let f = open_child(&bundle.path("top.cow"), &mut r).expect("complete bundle");
        assert_eq!(read_all(&f), c.expected);
    }

    /// No host backend advertises discard (hole punching is not implemented).
    #[test]
    fn no_host_backend_advertises_discard() {
        let _serial = process_spawn_guard();
        let dir = TempDir::new();
        let path = dir.file("a.raw", BYTES);
        let writer = HostFileStorage::open_read_write_exclusive(&path).expect("writer");
        assert!(!writer.capabilities().discard);
        drop(writer);
        let lease = ReadLease::open(&path).expect("lease");
        assert!(!lease.capabilities().discard);
        drop(lease);
        let parent = ParentLeaseStorage::new(ParentLease::open(&path).expect("parent"));
        assert!(!parent.capabilities().discard);
    }

    #[test]
    fn export_does_not_change_file_bytes() {
        let _serial = process_spawn_guard();
        let dir = TempDir::new();
        let path = dir.file("a.raw", BYTES);
        let before = std::fs::read(&path).expect("read before");
        let copied = export_with_lease(&path, |lease| {
            let len = usize::try_from(lease.size().unwrap_or(0)).unwrap_or(0);
            let mut out = vec![0_u8; len];
            let read = lease.read_at(0, &mut out).unwrap_or(0);
            // The lease is read-only: a write attempt is refused.
            let refused = lease.write_at(0, b"ZZ");
            (out, read, refused)
        })
        .expect("export");
        assert_eq!(copied.0, before);
        assert_eq!(copied.1, before.len());
        assert!(matches!(copied.2, Err(BackendError::Unsupported)));
        assert_eq!(std::fs::read(&path).expect("read after"), before);
    }
}
