//! S9-37, storage slice (no VMHost): concurrent readers and operations on real
//! host files, see "S9-37, срез storage" in docs/testing/STORAGE_RECOVERY.md.
//!
//! What is checked, on real files with the real locks of
//! `od_host::storage_io` / `storage_policy` (advisory `flock`, never waiting):
//!
//! 1. an export lease makes delete and compaction `Busy`; a compaction makes export
//!    `Busy` (inside one process and with the holder in another process);
//! 2. a child open in another process blocks delete and compaction of its
//!    parents until that process is killed with SIGKILL; the parents' bytes
//!    do not change;
//! 3. a symlink or a hard link to a leased parent is the same inode: `Busy`;
//! 4. a cancelled export releases its lease at once and publishes nothing;
//! 5. a lease pins the whole file (v1), so the generation and both root slots
//!    stay put while it is held; this is not a lease on one generation;
//! 6. two processes taking two files each, in opposite order get `Busy` or
//!    success within a time limit, never a hang.
//!
//! Not covered (blocked by the VMHost lifecycle, S9-36, S9-29...31): any
//! combination with a running VM, restore and clone, two VMHost processes.

#![cfg(target_os = "macos")]

use od_host::storage_io::{HostFileStorage, ReadLease, WriterOpenError};
use od_host::storage_policy::{
    ParentLease, ParentLeaseError, PolicyError, export_with_lease, publish_no_replace,
    repair_with_writer,
};
use od_storage::cow_export::{
    ExportError, ExportOptions, ExportReport, ImageReader, StateSelection, export_raw,
};
use od_storage::cow_proto::SLOT_SIZE;
use od_storage::cow_snapshot::{
    NoParents, ParentRef, ParentResolver, ResolveError, SnapError, SnapFile,
};
use od_storage::io::{BackendError, CheckedStorage, StorageBackend, StorageCapabilities};
use std::any::Any;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

const ROLE_ENV: &str = "OD_S9_CONC_ROLE";
const DIR_ENV: &str = "OD_S9_CONC_DIR";
const FIRST_ENV: &str = "OD_S9_CONC_FIRST";
const SECOND_ENV: &str = "OD_S9_CONC_SECOND";
const ROUNDS_ENV: &str = "OD_S9_CONC_ROUNDS";

/// Upper bound for one step of a child. A hang is a failure, not a wait.
const STEP_TIMEOUT: Duration = Duration::from_secs(30);

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

// ------------------------------------------------------------------ files

static NEXT: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "od-s9-conc-{}-{}",
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

/// Parents by uid under a read lease (uid 1 = base, uid 2 = mid), as the
/// offline CLI resolves them.
struct Paths(BTreeMap<u64, PathBuf>);

impl ParentResolver for Paths {
    type Backend = od_host::storage_policy::ParentLeaseStorage;
    fn open_parent(
        &mut self,
        p: &ParentRef,
    ) -> Result<CheckedStorage<Self::Backend>, ResolveError> {
        let path = self.0.get(&p.file_uid).ok_or(ResolveError::Missing)?;
        let lease = ParentLease::open(path).map_err(|_| ResolveError::Busy)?;
        CheckedStorage::new(od_host::storage_policy::ParentLeaseStorage::new(lease))
            .map_err(ResolveError::Storage)
    }
}

fn paths(dir: &Path) -> Paths {
    Paths(BTreeMap::from([
        (1, dir.join("base.cow")),
        (2, dir.join("mid.cow")),
    ]))
}

fn created(path: &Path) -> CheckedStorage<HostFileStorage> {
    CheckedStorage::new(HostFileStorage::create_new_exclusive(path, 0).expect("create"))
        .expect("caps")
}

fn target_storage(path: &Path) -> std::io::Result<CheckedStorage<HostFileStorage>> {
    let s = HostFileStorage::create_new_exclusive(path, 0).map_err(|e| match e {
        WriterOpenError::Io(e) => e,
        other => std::io::Error::other(format!("{other:?}")),
    })?;
    CheckedStorage::new(s).map_err(|e| std::io::Error::other(format!("{e:?}")))
}

fn lease(path: &Path) -> CheckedStorage<ReadLease> {
    CheckedStorage::new(ReadLease::open(path).expect("lease")).expect("caps")
}

/// base (uid 1, snapshots 1 and 2; snapshot 2 is referenced by nobody) <-
/// mid (uid 2, child of base snapshot 1, own snapshot) <- top (uid 3).
/// Everything is written by this process and closed.
fn prepare(dir: &Path) {
    let (base, mid, top) = (
        dir.join("base.cow"),
        dir.join("mid.cow"),
        dir.join("top.cow"),
    );
    let (mut b, _) = SnapFile::create_base(created(&base), 1).expect("base");
    b.write_at(0, &[0xAA; 1024]).expect("base write");
    b.create_snapshot().expect("snapshot 1");
    b.write_at(512, &[0xBB; 512]).expect("base write 2");
    b.create_snapshot().expect("snapshot 2");
    let base_ref = b.snapshot_ref(1).expect("base ref");
    drop(b);
    let mut resolver = paths(dir);
    let (mut m, _) =
        SnapFile::create_child(created(&mid), &mut resolver, &base_ref, 2).expect("mid");
    m.write_at(100, &[1, 2, 3]).expect("mid write");
    let (id, _) = m.create_snapshot().expect("mid snapshot");
    let mid_ref = m.snapshot_ref(id).expect("mid ref");
    drop(m);
    let (t, _) = SnapFile::create_child(created(&top), &mut resolver, &mid_ref, 3).expect("top");
    drop(t);
}

/// Head of a standalone file, read through its writer lock.
fn image_of(path: &Path) -> Vec<u8> {
    let (f, _) = SnapFile::open(
        CheckedStorage::new(HostFileStorage::open_read_write_exclusive(path).expect("writer"))
            .expect("caps"),
        &mut NoParents,
    )
    .expect("open");
    let mut v = vec![0u8; f.disk_size() as usize];
    f.read_at(0, &mut v).expect("read");
    v
}

// ------------------------------------------------------------- operations

/// Outcome of a host-level operation on a file.
#[derive(Debug)]
enum Op {
    Done,
    /// The exclusive writer lock of the file itself is held by someone.
    Busy,
    /// The file is free but an ancestor is held by a writer.
    ParentBusy,
    Failed(String),
}

/// Fails the child on an outcome that is neither success nor Busy.
fn unexpected(what: &str, op: Op) -> ! {
    match op {
        Op::Failed(message) => panic!("{what}: {message}"),
        other => panic!("{what}: {other:?}"),
    }
}

impl Op {
    fn is_busy(&self) -> bool {
        matches!(self, Op::Busy | Op::ParentBusy)
    }
}

/// The exclusive writer lock is the precondition of every mutation of a
/// file's reachability (same shape as the S9-17 host test in
/// `storage_policy`): opening for writing takes it, a held lease or writer
/// makes this `Busy` before any byte is read for writing.
fn exclusive(path: &Path, r: &mut Paths) -> Result<SnapFile<HostFileStorage>, Op> {
    let storage = match HostFileStorage::open_read_write_exclusive(path) {
        Ok(s) => s,
        Err(WriterOpenError::WriterBusy) => return Err(Op::Busy),
        Err(e) => return Err(Op::Failed(format!("{e:?}"))),
    };
    SnapFile::open(CheckedStorage::new(storage).expect("caps"), r)
        .map(|(f, _)| f)
        .map_err(|e| match e {
            SnapError::ParentBusy => Op::ParentBusy,
            e => Op::Failed(format!("{e:?}")),
        })
}

fn delete(path: &Path, id: u64, r: &mut Paths) -> Op {
    match exclusive(path, r) {
        Ok(mut f) => match f.delete_snapshot(id) {
            Ok(_) => Op::Done,
            Err(e) => Op::Failed(format!("{e:?}")),
        },
        Err(op) => op,
    }
}

/// Copy-compaction of `path` into the new file `target`; the target is only created
/// once the source writer lock is held, so a busy source leaves no file.
fn compact(path: &Path, target: &Path, uid: u64, r: &mut Paths) -> Op {
    match exclusive(path, r) {
        Ok(f) => match f.compact_to(created(target), uid) {
            Ok(_) => Op::Done,
            Err(e) => Op::Failed(format!("{e:?}")),
        },
        Err(op) => op,
    }
}

/// Delete, compaction, a plain writer and a repair writer on every file: all
/// are `Busy` through the exclusive lock, nothing is created or written.
fn assert_mutations_busy(dir: &Path, files: &[&Path], tag: &str) {
    for (i, p) in files.iter().enumerate() {
        let before = std::fs::read(p).expect("bytes");
        let target = dir.join(format!("probe-{tag}-{i}.cow"));
        let op = delete(p, 2, &mut paths(dir));
        assert!(matches!(op, Op::Busy), "{tag}: delete {p:?}: {op:?}");
        let op = compact(p, &target, 90, &mut paths(dir));
        assert!(matches!(op, Op::Busy), "{tag}: compaction {p:?}: {op:?}");
        assert!(
            !target.exists(),
            "{tag}: a busy compaction created its target"
        );
        assert!(
            matches!(
                HostFileStorage::open_read_write_exclusive(p),
                Err(WriterOpenError::WriterBusy)
            ),
            "{tag}: writer {p:?}"
        );
        assert!(
            matches!(repair_with_writer(p, |_| ()), Err(PolicyError::RepairBusy)),
            "{tag}: repair {p:?}"
        );
        assert_eq!(
            std::fs::read(p).expect("bytes"),
            before,
            "{tag}: {p:?} written"
        );
    }
}

/// Export, a lease and a parent lease are all refused while a writer holds the
/// file.
fn assert_readers_busy(path: &Path, tag: &str) {
    assert!(
        matches!(
            export_with_lease(path, |_| ()),
            Err(PolicyError::ExportBusy)
        ),
        "{tag}: export"
    );
    assert!(
        matches!(
            ReadLease::open(path),
            Err(od_host::storage_io::LeaseOpenError::LeaseBusy)
        ),
        "{tag}: lease"
    );
    assert!(
        matches!(ParentLease::open(path), Err(ParentLeaseError::Busy)),
        "{tag}: parent lease"
    );
}

/// Backend that runs `hook` once, before its first mutating call.
struct Hook<B> {
    inner: B,
    hook: Option<Box<dyn FnOnce()>>,
}

impl<B> Hook<B> {
    fn new(inner: B, hook: impl FnOnce() + 'static) -> Self {
        Self {
            inner,
            hook: Some(Box::new(hook)),
        }
    }
    fn fire(&mut self) {
        if let Some(h) = self.hook.take() {
            h();
        }
    }
}

impl<B: StorageBackend> StorageBackend for Hook<B> {
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
        self.fire();
        self.inner.write_at(offset, buf)
    }
    fn set_len(&mut self, len: u64) -> Result<(), BackendError> {
        self.fire();
        self.inner.set_len(len)
    }
    fn flush_file_durable(&mut self) -> Result<(), BackendError> {
        self.fire();
        self.inner.flush_file_durable()
    }
    fn flush_namespace_durable(&mut self) -> Result<(), BackendError> {
        self.fire();
        self.inner.flush_namespace_durable()
    }
}

/// Compaction of `base` into `target`; `hook` runs in the middle of it, while the
/// source writer lock is held.
fn compact_with_hook(dir: &Path, target: &Path, hook: impl FnOnce() + 'static) {
    let source = exclusive(&dir.join("base.cow"), &mut paths(dir)).expect("source writer");
    let storage = CheckedStorage::new(Hook::new(
        HostFileStorage::create_new_exclusive(target, 0).expect("target"),
        hook,
    ))
    .expect("caps");
    source.compact_to(storage, 90).expect("compaction");
}

// ------------------------------------------------------------ child process

fn announce(tag: &str) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "S9CONC={tag}").expect("stdout");
    out.flush().expect("flush");
}

/// Next command line from the parent. The parent closing stdin ends the child.
fn wait_cmd() -> String {
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => std::process::exit(0),
        Ok(_) => line.trim().to_string(),
    }
}

/// A held lock of the `abba` role: `w:<file>` is the plain writer lock, `chain:<file>`
/// opens the file as a child, i.e. its writer lock and read leases on all its
/// ancestors.
fn acquire(spec: &str, dir: &Path) -> Result<Box<dyn Any>, Op> {
    let (kind, name) = spec.split_once(':').expect("spec");
    let path = dir.join(name);
    match kind {
        "w" => match HostFileStorage::open_read_write_exclusive(&path) {
            Ok(s) => Ok(Box::new(s)),
            Err(WriterOpenError::WriterBusy) => Err(Op::Busy),
            Err(e) => Err(Op::Failed(format!("{e:?}"))),
        },
        "chain" => exclusive(&path, &mut paths(dir)).map(|f| Box::new(f) as Box<dyn Any>),
        other => panic!("unknown spec kind {other}"),
    }
}

fn child_main(role: &str, dir: &Path) {
    let base = dir.join("base.cow");
    match role {
        // Export of base, paused at its first cancellation poll while it holds
        // the read lease. "go" completes it, "cancel" cancels it.
        "export_hold" => {
            let out = dir.join("export.raw");
            let first = Cell::new(true);
            let cancel = Cell::new(false);
            let result = export_raw(
                lease(&base),
                &mut NoParents,
                StateSelection::Head,
                || target_storage(&out),
                &ExportOptions::default(),
                &|| {
                    if first.replace(false) {
                        announce("READY");
                        cancel.set(wait_cmd() == "cancel");
                    }
                    cancel.get()
                },
            );
            match result {
                Ok(_) => announce("RETURNED"),
                Err(ExportError::Cancelled) => announce("CANCELLED"),
                Err(e) => panic!("export: {e:?}"),
            }
            wait_cmd();
        }
        // Compaction of base paused in the middle (first write to its target),
        // while it holds the exclusive writer lock.
        "compact_hold" => {
            compact_with_hook(dir, &dir.join("compact.cow"), || {
                announce("READY");
                wait_cmd();
            });
            announce("RETURNED");
            wait_cmd();
        }
        // A child of the chain: writer lock on top, read leases on mid and base.
        "child_open" => {
            let f = exclusive(&dir.join("top.cow"), &mut paths(dir)).expect("child open");
            announce("READY");
            wait_cmd();
            drop(f);
        }
        // Two locks in a fixed order; see the parent tests for the pairings.
        "abba" => {
            let first = std::env::var(FIRST_ENV).expect("first");
            let second = std::env::var(SECOND_ENV).expect("second");
            let rounds: u32 = std::env::var(ROUNDS_ENV)
                .ok()
                .map_or(0, |v| v.parse().expect("rounds"));
            if rounds == 0 {
                let held = acquire(&first, dir).expect("first lock");
                announce("READY");
                wait_cmd();
                match acquire(&second, dir) {
                    Ok(_) => announce("SECOND=ok"),
                    Err(op) if op.is_busy() => announce("SECOND=busy"),
                    Err(op) => unexpected("second lock", op),
                }
                wait_cmd();
                drop(held);
            } else {
                announce("READY");
                wait_cmd();
                let (mut ok, mut busy) = (0u32, 0u32);
                for _ in 0..rounds {
                    match acquire(&first, dir) {
                        Ok(held) => {
                            // Hold the first lock long enough to overlap with
                            // the other process; outcomes are accepted either way.
                            std::thread::sleep(Duration::from_millis(1));
                            match acquire(&second, dir) {
                                Ok(_) => ok += 1,
                                Err(op) if op.is_busy() => busy += 1,
                                Err(op) => unexpected("second lock", op),
                            }
                            drop(held);
                        }
                        Err(op) if op.is_busy() => busy += 1,
                        Err(op) => unexpected("first lock", op),
                    }
                }
                announce(&format!("DONE ok={ok} busy={busy}"));
                wait_cmd();
            }
        }
        other => panic!("unknown role {other}"),
    }
}

/// Child mode only (run by the parent with `--ignored`); not a test itself.
#[test]
#[ignore = "child process of the S9-37 storage concurrency tests"]
fn stage9_conc_child() {
    let (Ok(role), Ok(dir)) = (std::env::var(ROLE_ENV), std::env::var(DIR_ENV)) else {
        return;
    };
    child_main(&role, Path::new(&dir));
}

// ----------------------------------------------------------------- parent

struct Proc {
    child: Child,
    stdin: Option<ChildStdin>,
    events: Receiver<String>,
    role: String,
}

impl Proc {
    fn spawn(role: &str, dir: &Path, extra: &[(&str, &str)]) -> Self {
        let mut cmd = Command::new(std::env::current_exe().expect("exe"));
        cmd.args([
            "--exact",
            "stage9_conc_child",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ROLE_ENV, role)
        .env(DIR_ENV, dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
        for (k, v) in extra {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn child");
        let stdout = child.stdout.take().expect("stdout");
        let stdin = child.stdin.take();
        let (tx, events) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                // libtest may print "test stage9_conc_child ... " on the same line.
                if let Some(i) = line.find("S9CONC=")
                    && tx.send(line[i + "S9CONC=".len()..].to_string()).is_err()
                {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            events,
            role: role.to_string(),
        }
    }

    /// Next event of the child, within the step timeout.
    fn event(&self) -> String {
        match self.events.recv_timeout(STEP_TIMEOUT) {
            Ok(e) => e,
            Err(RecvTimeoutError::Timeout) => {
                panic!("{}: no event in {STEP_TIMEOUT:?}: hang", self.role)
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!("{}: child ended without an event", self.role)
            }
        }
    }

    fn expect(&self, tag: &str) {
        assert_eq!(self.event(), tag, "{}", self.role);
    }

    fn send(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{line}").expect("send");
        stdin.flush().expect("flush");
    }

    /// SIGKILL; the status must be signal 9 and the kernel has then released
    /// every lock of the process.
    fn kill(&mut self) {
        self.child.kill().expect("SIGKILL");
        let status = self.child.wait().expect("wait");
        assert_eq!(status.signal(), Some(9), "{}: {status:?}", self.role);
    }

    /// Closes stdin and waits (bounded) for the child to exit by itself.
    fn finish(&mut self) {
        self.stdin = None;
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                assert!(status.success(), "{}: {status:?}", self.role);
                return;
            }
            assert!(
                start.elapsed() < STEP_TIMEOUT,
                "{}: does not exit: hang",
                self.role
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn alias_pair(dir: &Path, file: &Path, tag: &str) -> (PathBuf, PathBuf) {
    let sym = dir.join(format!("{tag}.sym"));
    let hard = dir.join(format!("{tag}.hard"));
    std::os::unix::fs::symlink(file, &sym).expect("symlink");
    std::fs::hard_link(file, &hard).expect("hard link");
    (sym, hard)
}

// ------------------------------------------------ 1. export <-> delete/compaction

#[test]
fn an_export_lease_makes_delete_and_compaction_busy_in_one_process() {
    let _s = serial();
    let dir = TempDir::new();
    prepare(&dir.0);
    let base = dir.0.join("base.cow");
    let before = std::fs::read(&base).expect("base");
    let out = dir.0.join("export.raw");
    let polls = Cell::new(0u32);
    let report = export_raw(
        lease(&base),
        &mut NoParents,
        StateSelection::Head,
        || target_storage(&out),
        &ExportOptions::default(),
        &|| {
            polls.set(polls.get() + 1);
            if polls.get() == 1 {
                // In the middle of the export: the lease is held.
                assert_mutations_busy(&dir.0, &[&base], "export");
                // Shared leases coexist: two exports of one file are allowed.
                drop(ReadLease::open(&base).expect("a second lease coexists"));
                export_with_lease(&base, |_| ()).expect("a second export coexists");
            }
            false
        },
    )
    .expect("export");
    assert!(polls.get() >= 3, "the probe ran inside the export");
    assert_eq!(
        report.logical_size,
        std::fs::metadata(&out).expect("out").len()
    );
    assert_eq!(
        std::fs::read(&base).expect("base"),
        before,
        "export wrote the source"
    );
    assert_eq!(std::fs::read(&out).expect("out"), image_of(&base));
    // The lease is gone with the export: both operations now run.
    assert!(matches!(delete(&base, 2, &mut paths(&dir.0)), Op::Done));
    let compacted = dir.0.join("compacted.cow");
    assert!(matches!(
        compact(&base, &compacted, 90, &mut paths(&dir.0)),
        Op::Done
    ));
    assert!(compacted.exists());
}

#[test]
fn a_compaction_makes_export_and_delete_busy_in_one_process() {
    let _s = serial();
    let dir = TempDir::new();
    prepare(&dir.0);
    let base = dir.0.join("base.cow");
    let target = dir.0.join("compact.cow");
    let (d, b) = (dir.0.clone(), base.clone());
    let source_before = std::fs::read(&base).expect("base");
    let observed = std::rc::Rc::new(Cell::new(false));
    let seen = std::rc::Rc::clone(&observed);
    compact_with_hook(&dir.0, &target, move || {
        assert_readers_busy(&b, "compaction");
        assert_mutations_busy(&d, &[&b], "compaction");
        seen.set(true);
    });
    assert!(observed.get(), "the probe ran inside the compaction");
    assert_eq!(
        std::fs::read(&base).expect("base"),
        source_before,
        "compaction wrote its source"
    );
    // Once the compaction returned the source is free again.
    export_with_lease(&base, |_| ()).expect("export after compaction");
    assert_eq!(image_of(&target), image_of(&base));
}

#[test]
fn an_export_in_another_process_makes_delete_and_compaction_busy() {
    let _s = serial();
    let dir = TempDir::new();
    prepare(&dir.0);
    let base = dir.0.join("base.cow");
    let before = std::fs::read(&base).expect("base");
    let mut p = Proc::spawn("export_hold", &dir.0, &[]);
    p.expect("READY");
    assert_mutations_busy(&dir.0, &[&base], "export-other");
    drop(ReadLease::open(&base).expect("a second lease coexists"));
    p.send("go");
    p.expect("RETURNED");
    assert_eq!(
        std::fs::read(&base).expect("base"),
        before,
        "export wrote the source"
    );
    assert_eq!(
        std::fs::read(dir.0.join("export.raw")).expect("out"),
        image_of(&base)
    );
    assert!(matches!(delete(&base, 2, &mut paths(&dir.0)), Op::Done));
    p.finish();
}

#[test]
fn a_compaction_in_another_process_makes_export_delete_and_compaction_busy() {
    let _s = serial();
    let dir = TempDir::new();
    prepare(&dir.0);
    let base = dir.0.join("base.cow");
    let before = std::fs::read(&base).expect("base");
    let mut p = Proc::spawn("compact_hold", &dir.0, &[]);
    p.expect("READY");
    assert_readers_busy(&base, "compaction-other");
    assert_mutations_busy(&dir.0, &[&base], "compaction-other");
    p.send("go");
    p.expect("RETURNED");
    assert_eq!(
        std::fs::read(&base).expect("base"),
        before,
        "compaction wrote its source"
    );
    export_with_lease(&base, |_| ()).expect("export after compaction");
    assert_eq!(image_of(&dir.0.join("compact.cow")), image_of(&base));
    p.finish();
}

// ----------------------------------- 2. child in another process, then SIGKILL

#[test]
fn a_child_in_another_process_blocks_parent_delete_and_compaction_until_sigkill() {
    let _s = serial();
    let dir = TempDir::new();
    prepare(&dir.0);
    let (base, mid, top) = (
        dir.0.join("base.cow"),
        dir.0.join("mid.cow"),
        dir.0.join("top.cow"),
    );
    let before = [&base, &mid, &top].map(|p| std::fs::read(p).expect("bytes"));
    let mut p = Proc::spawn("child_open", &dir.0, &[]);
    p.expect("READY");
    // The child holds its own writer lock and read leases on mid and base.
    assert_mutations_busy(&dir.0, &[&base, &mid, &top], "child");
    // Leases are shared: another reader of the parent is not blocked.
    drop(ReadLease::open(&base).expect("shared read of a leased parent"));
    let during = [&base, &mid, &top].map(|p| std::fs::read(p).expect("bytes"));
    assert_eq!(during, before, "a parent changed while its child was open");

    p.kill();
    let after = [&base, &mid, &top].map(|p| std::fs::read(p).expect("bytes"));
    assert_eq!(after, before, "the kill changed a file");
    // The kernel released the locks: the operations now run.
    assert!(matches!(delete(&base, 2, &mut paths(&dir.0)), Op::Done));
    assert!(matches!(
        compact(&base, &dir.0.join("c-base.cow"), 90, &mut paths(&dir.0)),
        Op::Done
    ));
    assert!(matches!(
        compact(&mid, &dir.0.join("c-mid.cow"), 91, &mut paths(&dir.0)),
        Op::Done
    ));
    drop(exclusive(&top, &mut paths(&dir.0)).expect("the child's own file is free"));
}

// ----------------------------------------------------- 3. symlink / hard link

#[test]
fn symlink_and_hard_link_aliases_of_a_leased_parent_are_busy_in_one_process() {
    let _s = serial();
    let dir = TempDir::new();
    prepare(&dir.0);
    let (base, mid) = (dir.0.join("base.cow"), dir.0.join("mid.cow"));
    let (base_sym, base_hard) = alias_pair(&dir.0, &base, "base");
    let (mid_sym, mid_hard) = alias_pair(&dir.0, &mid, "mid");
    let held = (
        ParentLease::open(&base).expect("lease base"),
        ReadLease::open(&mid).expect("lease mid"),
    );
    // The same inode through every name: the lock conflicts there too.
    assert_mutations_busy(
        &dir.0,
        &[&base_sym, &base_hard, &mid_sym, &mid_hard],
        "alias",
    );
    drop(held);
    // Released: the alias is just another name, and works.
    assert!(matches!(delete(&base_sym, 2, &mut paths(&dir.0)), Op::Done));
    assert!(matches!(
        compact(&mid_hard, &dir.0.join("c-mid.cow"), 91, &mut paths(&dir.0)),
        Op::Done
    ));
}

#[test]
fn symlink_and_hard_link_aliases_of_parents_leased_by_another_process_are_busy() {
    let _s = serial();
    let dir = TempDir::new();
    prepare(&dir.0);
    let (base, mid) = (dir.0.join("base.cow"), dir.0.join("mid.cow"));
    let (base_sym, base_hard) = alias_pair(&dir.0, &base, "base");
    let (mid_sym, mid_hard) = alias_pair(&dir.0, &mid, "mid");
    let mut p = Proc::spawn("child_open", &dir.0, &[]);
    p.expect("READY");
    assert_mutations_busy(
        &dir.0,
        &[&base_sym, &base_hard, &mid_sym, &mid_hard],
        "alias-other",
    );
    p.kill();
    assert!(matches!(
        delete(&base_hard, 2, &mut paths(&dir.0)),
        Op::Done
    ));
    assert!(matches!(
        compact(&mid_sym, &dir.0.join("c-mid.cow"), 91, &mut paths(&dir.0)),
        Op::Done
    ));
}

// ------------------------------------------------------------ 4. cancellation

/// Export into `partial`, then publish it under `target` without replacing. The
/// publication step follows the CLI shape (S9-20) and is only reached after a
/// successful export; the CLI itself is not exercised here.
fn export_and_publish(
    base: &Path,
    partial: &Path,
    target: &Path,
    cancel: &dyn Fn() -> bool,
) -> Result<ExportReport, ExportError> {
    let report = export_raw(
        lease(base),
        &mut NoParents,
        StateSelection::Head,
        || target_storage(partial),
        &ExportOptions::default(),
        cancel,
    )?;
    let writer = HostFileStorage::open_read_write_exclusive(partial).expect("partial writer");
    publish_no_replace(partial, target, &writer).expect("publish");
    Ok(report)
}

#[test]
fn a_cancelled_export_releases_its_lease_at_once_and_publishes_nothing() {
    let _s = serial();
    let dir = TempDir::new();
    prepare(&dir.0);
    let base = dir.0.join("base.cow");
    let before = std::fs::read(&base).expect("base");
    let head = image_of(&base);

    // An uncancelled run counts the cancellation polls and publishes.
    let polls = Cell::new(0u32);
    let full = dir.0.join("full.raw");
    export_and_publish(&base, &dir.0.join("full.part"), &full, &|| {
        polls.set(polls.get() + 1);
        false
    })
    .expect("uncancelled export");
    assert_eq!(std::fs::read(&full).expect("full"), head);
    assert!(
        !dir.0.join("full.part").exists(),
        "the partial name was removed"
    );
    let total = polls.get();
    assert!(total >= 3, "{total}");

    for n in 1..=total {
        let (partial, target) = (
            dir.0.join(format!("c{n}.part")),
            dir.0.join(format!("c{n}.raw")),
        );
        let seen = Cell::new(0u32);
        let result = export_and_publish(&base, &partial, &target, &|| {
            seen.set(seen.get() + 1);
            seen.get() == n
        });
        assert_eq!(result, Err(ExportError::Cancelled), "poll {n}");
        // The callback answers true exactly once, at poll n; the export made
        // no further poll, i.e. it stopped at the first true.
        assert_eq!(
            seen.get(),
            n,
            "poll {n}: the export went on after Cancelled"
        );
        // The lease is released by the time the call returned: a writer opens
        // immediately, and writes nothing here.
        drop(HostFileStorage::open_read_write_exclusive(&base).expect("lease released at once"));
        assert_eq!(
            std::fs::read(&base).expect("base"),
            before,
            "poll {n}: source changed"
        );
        assert!(!target.exists(), "poll {n}: something was published");
        // The incomplete partial stays (its cleanup is the caller's); the
        // library refuses to reuse it.
        assert!(partial.exists(), "poll {n}");
        let reuse = export_raw(
            lease(&base),
            &mut NoParents,
            StateSelection::Head,
            || target_storage(&partial),
            &ExportOptions::default(),
            &|| false,
        );
        assert_eq!(
            reuse,
            Err(ExportError::TargetCreate(std::io::ErrorKind::AlreadyExists)),
            "poll {n}"
        );
        if n == 1 {
            // Cancelled before the first chunk: not the exported bytes.
            assert_ne!(std::fs::read(&partial).expect("partial"), head);
        }
    }
    // The next writer really commits (the lease is not left behind).
    let mut f = exclusive(&base, &mut paths(&dir.0)).expect("writer");
    f.write_at(0, &[1, 2, 3])
        .expect("commit after cancelled exports");
}

#[test]
fn an_export_cancelled_in_another_process_releases_its_lease_at_once() {
    let _s = serial();
    let dir = TempDir::new();
    prepare(&dir.0);
    let base = dir.0.join("base.cow");
    let before = std::fs::read(&base).expect("base");
    let mut p = Proc::spawn("export_hold", &dir.0, &[]);
    p.expect("READY");
    assert!(matches!(
        HostFileStorage::open_read_write_exclusive(&base),
        Err(WriterOpenError::WriterBusy)
    ));
    p.send("cancel");
    p.expect("CANCELLED");
    // The child process is still alive: the lease went with the export call,
    // not with the process.
    drop(HostFileStorage::open_read_write_exclusive(&base).expect("released at once"));
    assert_eq!(std::fs::read(&base).expect("base"), before);
    assert!(matches!(delete(&base, 2, &mut paths(&dir.0)), Op::Done));
    p.finish();
}

// -------------------------------------------------------- 5. lease and generation

#[test]
fn a_lease_pins_the_file_so_generation_and_root_slots_stay_until_it_is_released() {
    let _s = serial();
    let dir = TempDir::new();
    prepare(&dir.0);
    let base = dir.0.join("base.cow");
    let file_before = std::fs::read(&base).expect("base");
    let slots_before = file_before[..2 * SLOT_SIZE].to_vec();

    let lease_held = ReadLease::open(&base).expect("lease");
    // A reader that fixed the current root. It reads through its own unlocked
    // descriptor: the protection comes from the lease above, not from it.
    let reader = ImageReader::open(
        CheckedStorage::new(HostFileStorage::open_read_only(&base).expect("reader")).expect("caps"),
        &mut NoParents,
    )
    .expect("reader opens");
    let generation = reader.generation();
    let mut old = vec![0u8; 1024];
    reader
        .read(StateSelection::Head, 0, &mut old)
        .expect("read");
    assert!(reader.root_unchanged().expect("check"));

    // While the lease is held no writer starts, whichever way it asks.
    for _ in 0..3 {
        assert!(matches!(
            HostFileStorage::open_read_write_exclusive(&base),
            Err(WriterOpenError::WriterBusy)
        ));
        assert!(matches!(
            exclusive(&base, &mut paths(&dir.0)),
            Err(Op::Busy)
        ));
    }
    let now = std::fs::read(&base).expect("base");
    assert_eq!(now, file_before, "the leased file changed");
    assert_eq!(
        now[..2 * SLOT_SIZE],
        slots_before[..],
        "a root slot changed"
    );
    assert_eq!(reader.generation(), generation);
    assert!(
        reader.root_unchanged().expect("check"),
        "root changed under the lease"
    );

    // Release; the writer commits and the reader's fixed root is stale.
    drop(lease_held);
    let mut f = exclusive(&base, &mut paths(&dir.0)).expect("writer after release");
    f.write_at(0, &[7u8; 16]).expect("commit");
    drop(f);
    assert!(
        !reader.root_unchanged().expect("check"),
        "the commit went unnoticed"
    );
    assert_eq!(
        reader.generation(),
        generation,
        "the reader's own generation is fixed"
    );
    let after = std::fs::read(&base).expect("base");
    assert_ne!(
        after[..2 * SLOT_SIZE],
        slots_before[..],
        "no root slot was written"
    );
    let newer = ImageReader::open(
        CheckedStorage::new(HostFileStorage::open_read_only(&base).expect("reader")).expect("caps"),
        &mut NoParents,
    )
    .expect("newer reader");
    assert!(newer.generation() > generation);
    // Whether the old reader can still read its fixed root after the commit
    // is not a v1 contract (its descriptor is not a lease), so it is neither
    // read nor asserted here.
}

// --------------------------------------------------------- 6. opposite order

/// Starts two processes that hold their first lock, releases both to take the
/// second one at the same moment and returns what each got. Each holds what
/// the other wants, so both must be refused: not a race.
///
/// The pairs are built so that each refusal comes from the other process and
/// never from a lock of the same process (a chain open takes read leases on
/// its ancestors, which conflict with a writer lock on the same file).
fn crossed_pair(dir: &Path, a: (&str, &str), b: (&str, &str)) -> (String, String) {
    let mut pa = Proc::spawn("abba", dir, &[(FIRST_ENV, a.0), (SECOND_ENV, a.1)]);
    let mut pb = Proc::spawn("abba", dir, &[(FIRST_ENV, b.0), (SECOND_ENV, b.1)]);
    pa.expect("READY");
    pb.expect("READY");
    pa.send("go");
    pb.send("go");
    let (ea, eb) = (pa.event(), pb.event());
    pa.send("end");
    pb.send("end");
    pa.finish();
    pb.finish();
    (ea, eb)
}

#[test]
fn two_processes_taking_two_files_in_opposite_order_are_both_refused_without_a_hang() {
    let _s = serial();
    let dir = TempDir::new();
    prepare(&dir.0);
    let before: Vec<Vec<u8>> = ["base.cow", "mid.cow", "top.cow"]
        .iter()
        .map(|n| std::fs::read(dir.0.join(n)).expect("bytes"))
        .collect();
    // Plain writer locks: A takes base then mid, B takes mid then base.
    let (a, b) = crossed_pair(
        &dir.0,
        ("w:base.cow", "w:mid.cow"),
        ("w:mid.cow", "w:base.cow"),
    );
    assert_eq!((a.as_str(), b.as_str()), ("SECOND=busy", "SECOND=busy"));
    // A chain: A opens the chain of top (writer on top, read leases on mid and
    // base) and wants x; B holds the separate file x and wants the writer lock
    // of base. A's wish is refused by B's lock on x, B's by A's lease on base:
    // each refusal is from the other process.
    let x = dir.0.join("x.cow");
    drop(SnapFile::create_base(created(&x), 4).expect("x"));
    let x_before = std::fs::read(&x).expect("x bytes");
    let (a, b) = crossed_pair(
        &dir.0,
        ("chain:top.cow", "w:x.cow"),
        ("w:x.cow", "w:base.cow"),
    );
    assert_eq!((a.as_str(), b.as_str()), ("SECOND=busy", "SECOND=busy"));
    assert_eq!(std::fs::read(&x).expect("x bytes"), x_before, "x written");
    drop(HostFileStorage::open_read_write_exclusive(&x).expect("x free"));
    // Nothing was left locked or written by either pair.
    for (i, n) in ["base.cow", "mid.cow", "top.cow"].iter().enumerate() {
        let path = dir.0.join(n);
        drop(HostFileStorage::open_read_write_exclusive(&path).expect("free"));
        assert_eq!(
            std::fs::read(&path).expect("bytes"),
            before[i],
            "{n} written"
        );
    }
}

#[test]
fn two_processes_racing_in_opposite_order_get_busy_or_success_within_the_time_limit() {
    let _s = serial();
    let dir = TempDir::new();
    prepare(&dir.0);
    const ROUNDS: u32 = 60;
    let rounds = ROUNDS.to_string();
    let x = dir.0.join("x.cow");
    drop(SnapFile::create_base(created(&x), 4).expect("x"));
    let mut totals = Vec::new();
    for (a, b) in [
        (("w:base.cow", "w:mid.cow"), ("w:mid.cow", "w:base.cow")),
        (("chain:top.cow", "w:x.cow"), ("w:x.cow", "w:base.cow")),
    ] {
        let mut pa = Proc::spawn(
            "abba",
            &dir.0,
            &[(FIRST_ENV, a.0), (SECOND_ENV, a.1), (ROUNDS_ENV, &rounds)],
        );
        let mut pb = Proc::spawn(
            "abba",
            &dir.0,
            &[(FIRST_ENV, b.0), (SECOND_ENV, b.1), (ROUNDS_ENV, &rounds)],
        );
        pa.expect("READY");
        pb.expect("READY");
        pa.send("go");
        pb.send("go");
        // `event` is bounded by the step timeout: a hang fails here.
        for p in [&pa, &pb] {
            let done = p.event();
            let counts: Vec<u32> = done
                .strip_prefix("DONE ")
                .expect("done line")
                .split(' ')
                .map(|kv| kv.split_once('=').expect("kv").1.parse().expect("count"))
                .collect();
            // What is checked: the child did not panic (any outcome other than
            // success or Busy panics there) and reported DONE within the step
            // timeout, i.e. no hang. The sum is only a consistency check of
            // the child's counters; it says nothing about whether the two
            // processes overlapped in time, and the split between success and
            // Busy is not asserted (the crossed test above proves that the
            // pairs conflict across processes).
            assert_eq!(counts.iter().sum::<u32>(), ROUNDS, "{done}");
            totals.push(done);
        }
        pa.send("end");
        pb.send("end");
        pa.finish();
        pb.finish();
    }
    for n in ["base.cow", "mid.cow", "top.cow", "x.cow"] {
        drop(HostFileStorage::open_read_write_exclusive(&dir.0.join(n)).expect("free"));
    }
    eprintln!("S9-37 opposite-order rounds: {}", totals.join(" | "));
}
