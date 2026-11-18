//! S9-38 / S9-06, level 1 of `docs/testing/STORAGE_RECOVERY.md`: the process
//! that runs a mutating storage operation on real host files is killed with
//! SIGKILL immediately before each of its mutating backend calls (and once
//! after the operation returned, before it could report). The parent then
//! checks, on the files the dead process left:
//!
//! * the writer lock was released by the kernel;
//! * the result is the state before the operation or the whole state after it
//!   (a plain model, not the implementation), never a mix; after the
//!   operation returned it is the state after it;
//! * a new target (branch, compaction, import, salvage) either does not open
//!   as an image or is complete; an exported RAW is complete once returned;
//! * the source of a copy never changes.
//!
//! A process kill leaves the host page cache alive: this is not a power-loss
//! test (level 3) and says nothing about what reaches the medium.

#![cfg(target_os = "macos")]

use od_host::storage_io::{HostFileStorage, ReadLease};
use od_host::storage_policy::{ParentLease, ParentLeaseStorage};
use od_storage::cow_export::{ExportOptions, StateSelection, export_raw, import_raw};
use od_storage::cow_proto::{BLOCK_SIZE, CowError};
use od_storage::cow_salvage::{SalvageOptions, salvage_to_copy};
use od_storage::cow_snapshot::{
    NoParents, ParentRef, ParentResolver, ResolveError, SnapError, SnapFile,
};
use od_storage::io::{BackendError, CheckedStorage, StorageBackend, StorageCapabilities};
use od_storage::raw::RawImage;
use od_storage::testing::rng::SplitMix64;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

const BS: usize = BLOCK_SIZE;
const OP_ENV: &str = "OD_S9_KILL_OP";
const DIR_ENV: &str = "OD_S9_KILL_DIR";
const AT_ENV: &str = "OD_S9_KILL_AT";
const RAW_LEN: usize = 8 * BS;
const RAW_AT: usize = 700;
const RAW_DATA_LEN: usize = 1100;
const GROW_AT: usize = 5 * BS + 3;
const GROW_LEN: usize = 900;

// ------------------------------------------------------------ the dead process

/// Counts mutating calls; before call number `kill_at` (0-based) it reports a
/// kill point and waits for the SIGKILL the parent sends.
struct KillAt<B> {
    inner: B,
    calls: u64,
    kill_at: Option<u64>,
}

impl<B> KillAt<B> {
    fn before(&mut self) {
        if self.kill_at == Some(self.calls) {
            wait_for_kill();
        }
        self.calls += 1;
    }
}

fn wait_for_kill() -> ! {
    let mut out = std::io::stdout().lock();
    writeln!(out, "S9KILL=POINT").expect("stdout");
    out.flush().expect("flush");
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    // The parent closes stdin only if it gave up; never continue the operation.
    std::process::abort();
}

impl<B: StorageBackend> StorageBackend for KillAt<B> {
    fn capabilities(&self) -> StorageCapabilities {
        self.inner.capabilities()
    }
    fn size(&self) -> Result<u64, BackendError> {
        self.inner.size()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, BackendError> {
        self.inner.read_at(offset, buf)
    }
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<usize, BackendError> {
        self.before();
        self.inner.write_at(offset, buf)
    }
    fn set_len(&mut self, len: u64) -> Result<(), BackendError> {
        self.before();
        self.inner.set_len(len)
    }
    fn flush_file_durable(&mut self) -> Result<(), BackendError> {
        self.before();
        self.inner.flush_file_durable()
    }
    fn flush_namespace_durable(&mut self) -> Result<(), BackendError> {
        self.before();
        self.inner.flush_namespace_durable()
    }
}

/// Parents by uid under a read lease, as the offline CLI resolves them.
struct Paths(BTreeMap<u64, PathBuf>);

impl ParentResolver for Paths {
    type Backend = ParentLeaseStorage;
    fn open_parent(
        &mut self,
        p: &ParentRef,
    ) -> Result<CheckedStorage<ParentLeaseStorage>, ResolveError> {
        let path = self.0.get(&p.file_uid).ok_or(ResolveError::Missing)?;
        let lease = ParentLease::open(path).map_err(|_| ResolveError::Busy)?;
        CheckedStorage::new(ParentLeaseStorage::new(lease)).map_err(ResolveError::Storage)
    }
}

fn writer(path: &Path) -> HostFileStorage {
    HostFileStorage::open_read_write_exclusive(path).expect("child writer")
}

fn new_file(path: &Path) -> HostFileStorage {
    HostFileStorage::create_new_exclusive(path, 0).expect("child target")
}

fn lease(path: &Path) -> CheckedStorage<ReadLease> {
    CheckedStorage::new(ReadLease::open(path).expect("lease")).expect("caps")
}

fn counted<B: StorageBackend>(inner: B, kill_at: Option<u64>) -> CheckedStorage<KillAt<B>> {
    CheckedStorage::new(KillAt {
        inner,
        calls: 0,
        kill_at,
    })
    .expect("caps")
}

/// Runs `op` once, killed before mutating call `kill_at` (or after the
/// operation returned for `u64::MAX`), or to completion for `None`.
fn child_main(op: &str, dir: &Path, kill_at: Option<u64>) {
    let base = dir.join("base.img");
    let raw = dir.join("raw.img");
    let target = dir.join("target.img");
    let mut data_rng = SplitMix64(0x5938);
    match op {
        "raw" => {
            let mut image = RawImage::open_read_write(
                KillAt {
                    inner: writer(&raw),
                    calls: 0,
                    kill_at,
                },
                Some(RAW_LEN as u64),
                BS as u64,
            )
            .expect("raw open");
            image
                .write_at(RAW_AT as u64, &data_rng.bytes(RAW_DATA_LEN))
                .expect("raw write");
            let mut storage = image.into_storage();
            storage.flush_file_durable().expect("raw flush");
        }
        "commit" | "snapshot" | "delete" => {
            let storage = counted(writer(&base), kill_at);
            let (mut f, _) = SnapFile::open(storage, &mut NoParents).expect("open base");
            match op {
                "commit" => {
                    f.write_at(GROW_AT as u64, &data_rng.bytes(GROW_LEN))
                        .expect("commit");
                }
                "snapshot" => {
                    f.create_snapshot().expect("snapshot");
                }
                _ => {
                    f.delete_snapshot(1).expect("delete");
                }
            }
            drop(f);
        }
        "branch" => {
            let parent = {
                let (f, _) = SnapFile::open(
                    CheckedStorage::new(writer(&base)).expect("caps"),
                    &mut NoParents,
                )
                .expect("open base");
                f.snapshot_ref(2).expect("snapshot 2")
            };
            let mut resolver = Paths(BTreeMap::from([(1, base.clone())]));
            SnapFile::create_child(
                counted(new_file(&target), kill_at),
                &mut resolver,
                &parent,
                7,
            )
            .expect("branch");
        }
        "compact" => {
            let (f, _) = SnapFile::open(
                CheckedStorage::new(writer(&base)).expect("caps"),
                &mut NoParents,
            )
            .expect("open base");
            f.compact_to(counted(new_file(&target), kill_at), 8)
                .expect("compact");
        }
        "export" => {
            export_raw(
                lease(&base),
                &mut NoParents,
                StateSelection::Head,
                || Ok(counted(new_file(&target), kill_at)),
                &ExportOptions::default(),
                &|| false,
            )
            .expect("export");
        }
        "import" => {
            import_raw(
                &lease(&raw),
                || Ok(counted(new_file(&target), kill_at)),
                9,
                &|| false,
            )
            .expect("import");
        }
        "salvage" => {
            let out = salvage_to_copy(
                lease(&base),
                &mut NoParents,
                &SalvageOptions {
                    selection: StateSelection::Head,
                    publish_partial: false,
                },
                10,
                || Ok(counted(new_file(&target), kill_at)),
            )
            .expect("salvage");
            assert!(out.copy.is_some(), "salvage did not publish");
        }
        other => panic!("unknown op {other}"),
    }
    // After the operation returned: one more kill point, before any report.
    if kill_at == Some(u64::MAX) {
        wait_for_kill();
    }
    let mut out = std::io::stdout().lock();
    writeln!(out, "S9KILL=RETURNED").expect("stdout");
    out.flush().expect("flush");
}

/// Child mode only (run by the parent with `--ignored`); not a test itself.
#[test]
#[ignore = "child process of killing_the_writer_process_at_every_mutating_call_leaves_old_or_new"]
fn stage9_kill_child() {
    let (Ok(op), Ok(dir), Ok(at)) = (
        std::env::var(OP_ENV),
        std::env::var(DIR_ENV),
        std::env::var(AT_ENV),
    ) else {
        return;
    };
    let kill_at = match at.as_str() {
        "none" => None,
        "returned" => Some(u64::MAX),
        n => Some(n.parse().expect("kill index")),
    };
    child_main(&op, Path::new(&dir), kill_at);
}

// ------------------------------------------------------------------- parent

static NEXT: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "od-s9-kill-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&p).expect("temp dir");
        Self(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Plain model of the prepared files.
#[derive(Clone)]
struct Model {
    head: Vec<u8>,
    snaps: BTreeMap<u64, Vec<u8>>,
    raw: Vec<u8>,
    /// Bytes of base.img and raw.img as prepared (sources of copies).
    base_file: Vec<u8>,
    raw_file: Vec<u8>,
}

fn apply(v: &mut Vec<u8>, at: usize, data: &[u8]) {
    let end = at + data.len();
    if v.len() < end {
        v.resize(end.next_multiple_of(BS), 0);
    }
    v[at..end].copy_from_slice(data);
}

/// Base image with snapshots 1 and 2 and a head written after them, plus a
/// RAW image. Everything is written by this process and closed.
fn prepare(dir: &Path) -> Model {
    let mut rng = SplitMix64(0x5939);
    let base = dir.join("base.img");
    let storage =
        CheckedStorage::new(HostFileStorage::create_new_exclusive(&base, 0).expect("base"))
            .expect("caps");
    let (mut f, _) = SnapFile::create_base(storage, 1).expect("create base");
    let mut head = Vec::new();
    let mut snaps = BTreeMap::new();
    for (at, len) in [(0usize, 3 * BS), (2 * BS + 10, 600)] {
        let data = rng.bytes(len);
        f.write_at(at as u64, &data).expect("write");
        apply(&mut head, at, &data);
        let (id, _) = f.create_snapshot().expect("snapshot");
        snaps.insert(id, head.clone());
    }
    let data = rng.bytes(BS);
    f.write_at(0, &data).expect("write head");
    apply(&mut head, 0, &data);
    drop(f);
    let raw = rng.bytes(RAW_LEN);
    let mut r =
        HostFileStorage::create_new_exclusive(&dir.join("raw.img"), RAW_LEN as u64).expect("raw");
    let mut at = 0;
    while at < raw.len() {
        at += r.write_at(at as u64, &raw[at..]).expect("raw write");
    }
    r.flush_file_durable().expect("raw flush");
    drop(r);
    Model {
        head,
        snaps,
        raw,
        base_file: std::fs::read(&base).expect("base bytes"),
        raw_file: std::fs::read(dir.join("raw.img")).expect("raw bytes"),
    }
}

enum Outcome {
    Killed,
    Returned,
}

/// Spawns the child for `op` with the kill point `at`; returns whether it was
/// killed at that point or ran to completion (`at = "none"`).
fn run_child(op: &str, dir: &Path, at: &str) -> Outcome {
    let mut child = Command::new(std::env::current_exe().expect("exe"))
        .args([
            "--exact",
            "stage9_kill_child",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(OP_ENV, op)
        .env(DIR_ENV, dir)
        .env(AT_ENV, at)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn child");
    let stdout = child.stdout.take().expect("stdout");
    let mut lines = BufReader::new(stdout).lines();
    let event = lines
        .by_ref()
        .map(|l| l.expect("line"))
        // libtest may print "test stage9_kill_child ... " on the same line.
        .find_map(|l| l.find("S9KILL=").map(|i| l[i..].to_string()))
        .unwrap_or_else(|| panic!("{op} at {at}: child ended without an event"));
    match event.as_str() {
        "S9KILL=POINT" => {
            child.kill().expect("SIGKILL");
            let status = child.wait().expect("wait");
            assert_eq!(status.signal(), Some(9), "{op} at {at}: {status:?}");
            Outcome::Killed
        }
        "S9KILL=RETURNED" => {
            let status = child.wait().expect("wait");
            assert!(status.success(), "{op} at {at}: {status:?}");
            Outcome::Returned
        }
        other => panic!("{op} at {at}: {other}"),
    }
}

fn image<B: StorageBackend>(f: &SnapFile<B>) -> Vec<u8> {
    let mut v = vec![0u8; f.disk_size() as usize];
    f.read_at(0, &mut v).expect("read");
    v
}

fn snap_image<B: StorageBackend>(f: &SnapFile<B>, id: u64) -> Vec<u8> {
    let blocks = f.snapshot_disk_blocks(id).expect("snapshot");
    let mut v = vec![0u8; blocks as usize * BS];
    f.read_snapshot_at(id, 0, &mut v).expect("read snapshot");
    v
}

/// Checks the files after the child died (`returned = false`) or completed;
/// returns whether the state after the operation is visible.
fn verify(op: &str, dir: &Path, m: &Model, returned: bool, label: &str) -> bool {
    let base = dir.join("base.img");
    let target = dir.join("target.img");
    let mut rng = SplitMix64(0x5938);
    // The kernel released every lock of the dead process.
    for path in [&base, &dir.join("raw.img")] {
        drop(
            HostFileStorage::open_read_write_exclusive(path).unwrap_or_else(|e| {
                panic!("{label}: lock of {} not released: {e:?}", path.display())
            }),
        );
    }
    match op {
        "raw" => {
            let data = rng.bytes(RAW_DATA_LEN);
            let now = std::fs::read(dir.join("raw.img")).expect("raw");
            assert_eq!(now.len(), RAW_LEN, "{label}");
            assert_eq!(now[..RAW_AT], m.raw[..RAW_AT], "{label}: before the write");
            assert_eq!(
                now[RAW_AT + RAW_DATA_LEN..],
                m.raw[RAW_AT + RAW_DATA_LEN..],
                "{label}: after the write"
            );
            let region = &now[RAW_AT..RAW_AT + RAW_DATA_LEN];
            let is_new = region == &data[..];
            // Killed between backend calls, a write is either issued or not:
            // no mix of old and new bytes in the range.
            assert!(
                is_new || region == &m.raw[RAW_AT..RAW_AT + RAW_DATA_LEN],
                "{label}: mixed RAW range"
            );
            if returned {
                assert!(is_new, "{label}: lost");
            }
            is_new
        }
        "commit" | "snapshot" | "delete" => {
            let storage = CheckedStorage::new(
                HostFileStorage::open_read_write_exclusive(&base).expect("reopen"),
            )
            .expect("caps");
            let (f, report) = SnapFile::open(storage, &mut NoParents)
                .unwrap_or_else(|e| panic!("{label}: base does not open: {e:?}"));
            // A kill between calls tears no write: recovery needs no repair.
            assert!(
                !report.rolled_forward && !report.possible_lost_commit,
                "{label}: {report:?}"
            );
            let old_ids: Vec<u64> = m.snaps.keys().copied().collect();
            let (now_ids, now_head) = (f.snapshot_ids(), image(&f));
            let is_new = match op {
                "commit" => {
                    let mut new = m.head.clone();
                    apply(&mut new, GROW_AT, &rng.bytes(GROW_LEN));
                    assert_eq!(now_ids, old_ids, "{label}");
                    assert!(now_head == m.head || now_head == new, "{label}: mixed head");
                    now_head == new
                }
                "snapshot" => {
                    assert_eq!(now_head, m.head, "{label}: head changed");
                    let new_ids = [old_ids.clone(), vec![3]].concat();
                    assert!(
                        now_ids == old_ids || now_ids == new_ids,
                        "{label}: ids {now_ids:?}"
                    );
                    if now_ids == new_ids {
                        assert_eq!(snap_image(&f, 3), m.head, "{label}: snapshot 3");
                    }
                    now_ids == new_ids
                }
                _ => {
                    assert_eq!(now_head, m.head, "{label}: head changed");
                    assert!(
                        now_ids == old_ids || now_ids == [2],
                        "{label}: ids {now_ids:?}"
                    );
                    now_ids == [2]
                }
            };
            for id in &now_ids {
                if let Some(img) = m.snaps.get(id) {
                    assert_eq!(&snap_image(&f, *id), img, "{label}: snapshot {id}");
                }
            }
            if returned {
                assert!(is_new, "{label}: returned {op} lost");
            }
            is_new
        }
        _ => {
            // Copies: the source files never change, byte for byte.
            assert!(
                std::fs::read(&base).expect("base") == m.base_file,
                "{label}: base.img changed"
            );
            assert!(
                std::fs::read(dir.join("raw.img")).expect("raw") == m.raw_file,
                "{label}: raw.img changed"
            );
            let (f, _) = SnapFile::open(
                CheckedStorage::new(
                    HostFileStorage::open_read_write_exclusive(&base).expect("reopen"),
                )
                .expect("caps"),
                &mut NoParents,
            )
            .expect("base");
            assert_eq!(image(&f), m.head, "{label}: source head changed");
            assert_eq!(
                f.snapshot_ids(),
                m.snaps.keys().copied().collect::<Vec<_>>(),
                "{label}"
            );
            drop(f);
            if op == "export" {
                if target.exists() {
                    drop(
                        HostFileStorage::open_read_write_exclusive(&target).unwrap_or_else(|e| {
                            panic!("{label}: export target lock not released: {e:?}")
                        }),
                    );
                }
                let is_new = std::fs::read(&target).is_ok_and(|b| b == m.head);
                if returned {
                    assert!(is_new, "{label}: export");
                }
                return is_new;
            }
            let expected = match op {
                "branch" => m.snaps[&2].clone(),
                "import" => m.raw.clone(),
                _ => m.head.clone(),
            };
            // The target is created before the first kill point; its lock must
            // be released and only a missing file means "no target".
            let opened = match HostFileStorage::open_read_write_exclusive(&target) {
                Ok(s) => {
                    let mut resolver = Paths(BTreeMap::from([(1, base.clone())]));
                    Some(SnapFile::open(
                        CheckedStorage::new(s).expect("caps"),
                        &mut resolver,
                    ))
                }
                Err(od_host::storage_io::WriterOpenError::Io(e))
                    if e.kind() == std::io::ErrorKind::NotFound =>
                {
                    None
                }
                Err(e) => panic!("{label}: target lock not released or unreadable: {e:?}"),
            };
            match opened {
                Some(Ok((t, _))) => {
                    assert_eq!(image(&t), expected, "{label}: partial {op} target");
                    if op == "compact" {
                        for (id, img) in &m.snaps {
                            assert_eq!(&snap_image(&t, *id), img, "{label}: snapshot {id}");
                        }
                    }
                    true
                }
                Some(Err(e)) => {
                    assert!(!returned, "{label}: returned {op} does not open: {e:?}");
                    assert!(
                        is_absent(&e),
                        "{label}: target damaged, not unfinished: {e:?}"
                    );
                    false
                }
                None => {
                    assert!(!returned, "{label}: returned {op} has no target");
                    false
                }
            }
        }
    }
}

/// An image that never finished: no header, no valid root, or a building
/// target.
fn is_absent(e: &SnapError) -> bool {
    matches!(
        e,
        SnapError::NotSnapFile(_)
            | SnapError::BuildInProgress
            | SnapError::Cow(CowError::NoValidRoot { .. })
            | SnapError::Cow(CowError::IncompleteCreate { .. })
    )
}

/// Kills `op` before each of its mutating calls (the count is found by
/// killing at increasing indices until the child runs to completion) and once
/// after it returned. Returns (kill points, kills showing the old state, kills
/// showing the new state).
fn kill_sweep(op: &str) -> (u64, u64, u64) {
    let (mut killed, mut old_n, mut new_n) = (0u64, 0u64, 0u64);
    for n in 0u64.. {
        let dir = TempDir::new();
        let m = prepare(&dir.0);
        let label = format!("{op} kill before call {n}");
        match run_child(op, &dir.0, &n.to_string()) {
            Outcome::Killed => {
                killed += 1;
                if verify(op, &dir.0, &m, false, &label) {
                    new_n += 1;
                } else {
                    old_n += 1;
                }
            }
            Outcome::Returned => {
                verify(op, &dir.0, &m, true, &format!("{op} ran to completion"));
                break;
            }
        }
        assert!(n < 10_000, "{op}: no end");
    }
    // After the operation returned, before it could report.
    let dir = TempDir::new();
    let m = prepare(&dir.0);
    assert!(matches!(run_child(op, &dir.0, "returned"), Outcome::Killed));
    verify(op, &dir.0, &m, true, &format!("{op} killed after return"));
    (killed, old_n, new_n)
}

#[test]
fn killing_the_writer_process_at_every_mutating_call_leaves_old_or_new() {
    let mut summary = Vec::new();
    for op in [
        "raw", "commit", "snapshot", "delete", "branch", "compact", "export", "import", "salvage",
    ] {
        let (killed, old_n, new_n) = kill_sweep(op);
        // Both outcomes occur, so the checks above are not vacuous.
        assert!(
            killed >= 2 && old_n > 0 && new_n > 0,
            "{op}: {killed} kill points, {old_n} old, {new_n} new"
        );
        summary.push(format!("{op}={killed}(old {old_n}, new {new_n})"));
    }
    eprintln!("S9-38 level 1 kill points: {}", summary.join(" "));
}

// ------------------------------------------- S9-06: protocol order on a host file

/// Records the order of mutating calls that reach the host file.
struct Journal {
    inner: HostFileStorage,
    log: std::rc::Rc<std::cell::RefCell<Vec<(&'static str, u64, usize)>>>,
}

impl StorageBackend for Journal {
    fn capabilities(&self) -> StorageCapabilities {
        self.inner.capabilities()
    }
    fn size(&self) -> Result<u64, BackendError> {
        self.inner.size()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, BackendError> {
        self.inner.read_at(offset, buf)
    }
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<usize, BackendError> {
        let n = self.inner.write_at(offset, buf)?;
        self.log.borrow_mut().push(("write", offset, n));
        Ok(n)
    }
    fn set_len(&mut self, len: u64) -> Result<(), BackendError> {
        self.inner.set_len(len)?;
        self.log.borrow_mut().push(("set_len", len, 0));
        Ok(())
    }
    fn flush_file_durable(&mut self) -> Result<(), BackendError> {
        self.inner.flush_file_durable()?;
        self.log.borrow_mut().push(("flush", 0, 0));
        Ok(())
    }
}

/// One CoW commit through the real host backend reaches the file in the
/// order of STORAGE_FORMAT §9.4: every append (blocks, map, root copy in the
/// last sector) before barrier 1, then exactly one 512-byte root write into
/// the slot of the new generation, then barrier 2, and nothing after it before
/// the acknowledgement. `F_FULLFSYNC` itself is the host's; this checks our
/// order of calls, not what the medium does with them.
#[test]
fn a_commit_reaches_the_host_file_in_protocol_order() {
    use od_storage::cow_proto::{CowFile, HEADER_LEN, SLOT_SIZE, slot_offset};
    let dir = TempDir::new();
    let path = dir.0.join("order.img");
    let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let storage = CheckedStorage::new(Journal {
        inner: HostFileStorage::create_new_exclusive(&path, 0).expect("create"),
        log: std::rc::Rc::clone(&log),
    })
    .expect("caps");
    let (mut f, _) = CowFile::create(storage).expect("create cow");
    let mut rng = SplitMix64(0x5940);
    let blocks: Vec<Vec<u8>> = (0..5).map(|_| rng.bytes(BS)).collect();
    let writes: Vec<(u64, &[u8])> = blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (i as u64, b.as_slice()))
        .collect();
    let old_end = f.end_offset();
    log.borrow_mut().clear();
    f.commit(&writes).expect("commit");
    let generation = f.generation();
    let new_end = f.end_offset();
    let calls = log.borrow().clone();
    let flushes: Vec<usize> = calls
        .iter()
        .enumerate()
        .filter(|(_, c)| c.0 == "flush")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(flushes.len(), 2, "two barriers: {calls:?}");
    let (b1, b2) = (flushes[0], flushes[1]);
    assert_eq!(b2, calls.len() - 1, "nothing after barrier 2: {calls:?}");
    // Before barrier 1: only appends at or after the old end, ending with the
    // root copy in the last sector of the new end.
    let before = &calls[..b1];
    assert!(!before.is_empty(), "{calls:?}");
    for c in before {
        match *c {
            ("write", off, _) => assert!(off >= old_end && off >= HEADER_LEN, "{c:?}: {calls:?}"),
            ("set_len", len, _) => assert!(len >= old_end, "{c:?}"),
            other => panic!("unexpected {other:?}"),
        }
    }
    let last_write = before
        .iter()
        .rev()
        .find(|c| c.0 == "write")
        .expect("root copy");
    assert_eq!(
        last_write.1 + last_write.2 as u64,
        new_end,
        "root copy ends the file: {calls:?}"
    );
    let file = std::fs::read(&path).expect("file");
    let slot = slot_offset(generation) as usize;
    assert_eq!(
        last_write.2, SLOT_SIZE,
        "root copy is one sector: {calls:?}"
    );
    assert_eq!(
        file[new_end as usize - SLOT_SIZE..new_end as usize],
        file[slot..slot + SLOT_SIZE],
        "root copy equals the published root"
    );
    // Between the barriers: exactly the root of the new generation into its
    // slot.
    let between = &calls[b1 + 1..b2];
    assert_eq!(
        between,
        &[("write", slot_offset(generation), SLOT_SIZE)],
        "{calls:?}"
    );
    drop(f);
    // The acknowledged commit reopens from the file with the new generation.
    let (back, _) = CowFile::open(
        CheckedStorage::new(HostFileStorage::open_read_write_exclusive(&path).expect("reopen"))
            .expect("caps"),
    )
    .expect("open");
    assert_eq!(back.generation(), generation);
    for (i, b) in blocks.iter().enumerate() {
        assert_eq!(&back.read_block(i as u64).expect("read"), b);
    }
}
