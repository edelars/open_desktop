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
//! * There is no notion of a parent inside a snapshot chain, no chain
//!   verification and no VMHost lifecycle integration here: that is later
//!   S9 work (S9-14, S9-16 and the VMHost slices).

use crate::storage_io::{HostFileStorage, LeaseOpenError, ReadLease, WriterOpenError};
use od_storage::io::{BackendError, StorageBackend};
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
