//! Bounded repeated-recovery soak (S9-41) over the prototype CoW stack.
//!
//! One seeded run drives a main image through a fixed number of cycles. Every
//! cycle is one randomly chosen operation (write, snapshot, delete, branch,
//! compaction, RAW export, salvage, RAW migration round trip, clean restart,
//! child maintenance, chain growth) and some of them run with an injected
//! fault (failed, short, torn or `ENOSPC` write, failed resize, failed or
//! lost-acknowledgement flush) or a crash at an exact backend call. After
//! every failure the writer is dropped, the media is crashed and the file is
//! opened again, sometimes with a fault inside that recovery as well.
//!
//! # Oracle
//!
//! The oracle is a plain `Vec<u8>` for the head and for every snapshot plus
//! the next snapshot id. It never reads a header, bitmap or block map. The
//! checks compare the oracle's next id with `next_snapshot_id` of the
//! published header, read through the read-only inspector after every
//! recovery and every cycle. An
//! acknowledged operation moves the oracle to the new state and that state
//! must survive every later crash. An operation that failed leaves two
//! legal outcomes, the previous acknowledged state or the whole new one;
//! the next open must show exactly one of them, never a mix (the same rule
//! as the sweeps in `cow_proto_tests`). An operation rejected before any
//! backend call must change nothing.
//!
//! # Measured scope
//!
//! The scope is fixed before the run: [`SOAK_CYCLES`] cycles per seed and
//! [`SOAK_SEEDS`]. The run is deterministic for a seed; a failure panics with
//! the seed, the cycle, the operation, the harness log and the tail of the
//! backend trace. `OD_SOAK_CYCLES` and `OD_SOAK_SEED` override the scope for a
//! longer manual run; `OD_SOAK_REPORT=1` prints the summary. A bounded run
//! that finds nothing is evidence for this model and these parameters only.
//! It is not a proof, and the fault backend is a model of the storage, not a
//! disk, an operating system or a power loss.
//!
//! What is measured per cycle (all against numbers the oracle derives, not
//! from the implementation's own bookkeeping):
//!
//! * file size against an upper bound from the commits that were acknowledged,
//!   plus the capacity quota that the layer must enforce as a hard limit;
//! * number of snapshots against `MAX_SNAPSHOTS`, snapshot ids strictly
//!   increasing and never reused, across crashes and compactions;
//! * generation: exactly one step per acknowledged commit, never below the
//!   last acknowledged value, far from `u64::MAX`;
//! * sessions: every `FaultBackend` the harness opens is counted and must be
//!   dropped by the end of the cycle;
//! * a writer that was open before a crash must not write to the media that
//!   the recovered writer owns.

use crate::cow_export::{ExportOptions, StateSelection, export_raw, import_raw};
use crate::cow_inspect::{InspectMode, inspect_storage};
use crate::cow_proto::{BLOCK_SIZE, CREATE_GENERATION};
use crate::cow_proto_tests::{OpKind, is_failing, random_policy};
use crate::cow_salvage::{CopyStatus, Recoverability, SalvageOptions, salvage_to_copy};
use crate::cow_snapshot::{
    CapacityPolicy, DATA_BASE, MAX_CHAIN_DEPTH, MAX_SNAPSHOTS, NoParents, ParentRef,
    ParentResolver, ResolveError, SnapError, SnapFile,
};
use crate::io::{BackendError, CheckedStorage, StorageBackend, StorageCapabilities};
use crate::testing::fault::{
    CrashPolicy, Fault, FaultBackend, FaultHandle, FlushFault, WriteFault,
};
use crate::testing::rng::SplitMix64;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::ErrorKind;
use std::rc::Rc;

const BS: usize = BLOCK_SIZE;

/// Cycles per seed. Fixed before the run; `OD_SOAK_CYCLES` overrides it.
pub(crate) const SOAK_CYCLES: usize = 500;
/// Seeds of the default run. `OD_SOAK_SEED` replaces them with one seed.
pub(crate) const SOAK_SEEDS: [u64; 3] = [0x5041_C0DE_0001, 0x5041_C0DE_0002, 0x5041_C0DE_0003];

/// Disk size limit of the soak, in blocks.
const DISK_BLOCKS_MAX: usize = 16;
/// Most closed child files kept at once (they form the chains).
const MAX_CHILDREN: usize = 12;
/// Hard file limit and metadata reserve of the main image. Reaching it forces
/// a compaction, so the soak also measures reclamation.
const QUOTA: CapacityPolicy = CapacityPolicy {
    max_file_bytes: 160 * 1024,
    reserve_bytes: 16 * 1024,
};
/// A generation closer than this to `u64::MAX` is a failure of the soak.
const GENERATION_MARGIN: u64 = 1 << 32;
/// Percentage of operations that run with an injected fault or crash.
const FAULT_PERCENT: u64 = 40;
/// Percentage of recovery attempts that run with an injected fault or crash.
const RECOVERY_FAULT_PERCENT: u64 = 25;
/// The oracle's own upper bound for the size of one map entry.
const ENTRY_BOUND: u64 = 16;
/// Lines of the harness log and of the backend trace in a failure report.
const REPORT_LINES: usize = 80;

// ------------------------------------------------------------ fault source

#[derive(Clone, Copy, Debug)]
enum Class {
    Io,
    NoSpace,
    Short,
    Torn,
    FlushLost,
    FlushAfter,
}

#[derive(Clone, Copy, Debug)]
enum Plan {
    Fault(Class, u64),
    Crash(CrashPolicy),
    /// A torn write of the first write after the first barrier of the window:
    /// in a commit that is the root slot, so the next open must roll the root
    /// copy forward. The harness keeps the torn bytes (`KeepPending`).
    SlotTear(u64),
}

fn map_fault(op: OpKind, class: Class, param: u64) -> Fault {
    let pick = |n: u64| (param % n) as usize;
    match op {
        OpKind::Write => Fault::Write(match class {
            Class::NoSpace => WriteFault::Fail(ErrorKind::StorageFull),
            Class::Short => WriteFault::Short([0, 1, 100][pick(3)]),
            Class::Torn => WriteFault::TornIo {
                written: [0, 1, 100, 508][pick(4)],
            },
            _ => WriteFault::Fail(ErrorKind::Other),
        }),
        OpKind::SetLen => Fault::Resize(match class {
            Class::NoSpace => ErrorKind::StorageFull,
            _ => ErrorKind::Other,
        }),
        OpKind::Flush => Fault::Flush(match class {
            Class::Torn | Class::FlushAfter => FlushFault::IoAfterDurable,
            _ => FlushFault::IoNotDurable,
        }),
    }
}

/// What happened in the window between `arm` and `disarm`.
#[derive(Clone, Copy, Debug, Default)]
struct Window {
    /// The armed plan fired.
    fired: bool,
    /// A fault that must make the operation fail was scripted.
    failing: bool,
    /// A crash of a handle was triggered inside the window.
    crashed: bool,
    /// The fault is a torn root slot write.
    tear: bool,
    /// Durable flushes seen in the window.
    flushes: u32,
}

/// Counts the mutating backend calls of the window and fires one plan at the
/// chosen call. Reads are never faulted.
#[derive(Default)]
struct Injector {
    plan: Option<(u64, Plan)>,
    calls: u64,
    window: Window,
}

/// [`FaultBackend`] that asks the [`Injector`] before every mutating call, so a
/// fault is scripted for the call's own kind. It also counts live sessions.
struct SoakBackend {
    inner: FaultBackend,
    handle: FaultHandle,
    inj: Rc<RefCell<Injector>>,
    live: Rc<Cell<usize>>,
}

impl SoakBackend {
    fn new(handle: &FaultHandle, inj: &Rc<RefCell<Injector>>, live: &Rc<Cell<usize>>) -> Self {
        live.set(live.get() + 1);
        Self {
            inner: handle.open(),
            handle: handle.clone(),
            inj: Rc::clone(inj),
            live: Rc::clone(live),
        }
    }

    fn before(&self, op: OpKind) {
        let fire = {
            let mut inj = self.inj.borrow_mut();
            let after_barrier = inj.window.flushes > 0;
            if inj.plan.is_some() && op == OpKind::Flush {
                inj.window.flushes += 1;
            }
            match inj.plan {
                Some((_, plan @ Plan::SlotTear(_))) => {
                    if op == OpKind::Write && after_barrier {
                        inj.plan = None;
                        inj.window.fired = true;
                        Some(plan)
                    } else {
                        None
                    }
                }
                Some((at, plan)) => {
                    let index = inj.calls;
                    inj.calls += 1;
                    if index == at {
                        inj.plan = None;
                        inj.window.fired = true;
                        Some(plan)
                    } else {
                        None
                    }
                }
                None => None,
            }
        };
        match fire {
            Some(Plan::Crash(policy)) => {
                self.inj.borrow_mut().window.crashed = true;
                self.handle.crash(policy);
            }
            Some(Plan::Fault(class, param)) => {
                let fault = map_fault(op, class, param);
                if is_failing(fault) {
                    self.inj.borrow_mut().window.failing = true;
                }
                self.handle.script_next(fault);
            }
            Some(Plan::SlotTear(param)) => {
                {
                    let mut inj = self.inj.borrow_mut();
                    inj.window.failing = true;
                    inj.window.tear = true;
                }
                let written = [1, 100, 508][(param % 3) as usize];
                self.handle
                    .script_next(Fault::Write(WriteFault::TornIo { written }));
            }
            None => {}
        }
    }
}

impl Drop for SoakBackend {
    fn drop(&mut self) {
        self.live.set(self.live.get().saturating_sub(1));
    }
}

impl StorageBackend for SoakBackend {
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
        self.before(OpKind::Write);
        self.inner.write_at(offset, buf)
    }

    fn set_len(&mut self, len: u64) -> Result<(), BackendError> {
        self.before(OpKind::SetLen);
        self.inner.set_len(len)
    }

    fn flush_file_durable(&mut self) -> Result<(), BackendError> {
        self.before(OpKind::Flush);
        self.inner.flush_file_durable()
    }
}

type File = SnapFile<SoakBackend>;

// ------------------------------------------------------------------ oracle

/// Plain model of one file: the head, every snapshot, the next id.
#[derive(Clone, Default, Eq, PartialEq)]
struct State {
    head: Vec<u8>,
    snaps: BTreeMap<u64, Vec<u8>>,
    next_id: u64,
}

impl State {
    fn new() -> Self {
        Self {
            head: Vec::new(),
            snaps: BTreeMap::new(),
            next_id: 1,
        }
    }

    fn write(&mut self, off: usize, data: &[u8]) {
        let end = off + data.len();
        if self.head.len() < end {
            self.head.resize(end.next_multiple_of(BS), 0);
        }
        self.head[off..end].copy_from_slice(data);
    }

    fn disk_blocks(&self) -> u64 {
        (self.head.len() / BS) as u64
    }

    /// Same logical content (ids and bytes). `next_id` is not observable
    /// through the file API; [`Soak::resolve`] and `check_cycle` check it against the header
    /// separately.
    fn same(&self, other: &State) -> bool {
        self.head == other.head && self.snaps == other.snaps
    }

    fn summary(&self) -> String {
        format!(
            "head={}B snaps={:?} next_id={}",
            self.head.len(),
            self.snaps.keys().collect::<Vec<_>>(),
            self.next_id
        )
    }
}

/// Reads the logical content of an open file through its public API.
fn observe<B: StorageBackend>(f: &SnapFile<B>) -> Result<State, SnapError> {
    let mut head = vec![0u8; usize::try_from(f.disk_size()).map_err(|_| SnapError::OutOfRange)?];
    f.read_at(0, &mut head)?;
    let mut snaps = BTreeMap::new();
    for id in f.snapshot_ids() {
        let blocks = f
            .snapshot_disk_blocks(id)
            .ok_or(SnapError::SnapshotMissing { id })?;
        let mut bytes = vec![0u8; usize::try_from(blocks).map_err(|_| SnapError::OutOfRange)? * BS];
        f.read_snapshot_at(id, 0, &mut bytes)?;
        snaps.insert(id, bytes);
    }
    Ok(State {
        head,
        snaps,
        next_id: 0,
    })
}

/// `next_snapshot_id` of the published header, read by the inspector over a
/// separate read-only view of the media (reads do not count as backend calls
/// of the fault model). `None` unless the inspector read the same root as the
/// open writer: generation `gen`, file `uid`, a usable (not building) head.
fn media_next_id(h: &FaultHandle, generation: u64, uid: u64) -> Option<u64> {
    let st = CheckedStorage::new(h.open()).ok()?;
    let rep = inspect_storage(st, &mut NoParents, InspectMode::Info);
    if rep.root_generation != Some(generation) {
        return None;
    }
    rep.layer
        .filter(|l| l.file_uid == uid && !l.building)
        .map(|l| l.next_snapshot_id)
}

/// Upper bound of what one acknowledged write commit appends: data blocks,
/// zero placeholders of a gap, bitmap, header, the block map and two sectors.
fn write_cost(off: usize, len: usize, disk_after: u64) -> u64 {
    let first = (off / BS) as u64;
    let last = ((off + len - 1) / BS) as u64;
    let blocks = (last - first + 1) + first + 3;
    blocks * BS as u64 + (disk_after + DATA_BASE) * ENTRY_BOUND + 1024
}

/// Upper bound of a header-only commit (snapshot, delete).
fn meta_cost(disk_after: u64) -> u64 {
    BS as u64 + (disk_after + DATA_BASE) * ENTRY_BOUND + 1024
}

/// Upper bound of a freshly compacted file that holds `state`: one level per
/// snapshot plus the head, each as up to two commits.
fn live_bound(state: &State) -> u64 {
    let level = |blocks: u64| {
        (blocks + 4) * BS as u64 + 2 * (blocks + DATA_BASE) * ENTRY_BOUND + 2048 + BS as u64
    };
    let snaps: u64 = state
        .snaps
        .values()
        .map(|s| level((s.len() / BS) as u64))
        .sum();
    4096 + snaps + level(state.disk_blocks())
}

// ------------------------------------------------------------------- world

/// Media of every file by uid; the parent resolver's catalogue.
type Fs = BTreeMap<u64, FaultHandle>;

struct FsResolver<'a> {
    fs: &'a Fs,
    inj: Rc<RefCell<Injector>>,
    live: Rc<Cell<usize>>,
}

impl ParentResolver for FsResolver<'_> {
    type Backend = SoakBackend;

    fn open_parent(&mut self, p: &ParentRef) -> Result<CheckedStorage<SoakBackend>, ResolveError> {
        let h = self.fs.get(&p.file_uid).ok_or(ResolveError::Missing)?;
        CheckedStorage::new(SoakBackend::new(h, &self.inj, &self.live))
            .map_err(ResolveError::Storage)
    }
}

/// The main image: always an open writer between cycles.
struct Img {
    uid: u64,
    handle: FaultHandle,
    acked: State,
    /// Last known generation of the open file.
    gen_now: u64,
    /// Oracle bound for the file size (see `write_cost`, `live_bound`).
    bound: u64,
    /// Largest single attempt so far (an unpublished tail may exist).
    slack: u64,
}

/// A closed child file (a branch) and the oracle of its content.
struct Child {
    uid: u64,
    handle: FaultHandle,
    parent_uid: u64,
    parent_snap: u64,
    depth: u32,
    model: State,
}

#[derive(Clone, Copy, Debug, Default)]
struct Stats {
    cycles: u64,
    ops: [u64; 12],
    acked_commits: u64,
    failed_ops: u64,
    rejected_ops: u64,
    quota_rejects: u64,
    snapshot_limit_hits: u64,
    faults_fired: u64,
    failing_faults: u64,
    crashes: u64,
    slot_tears: u64,
    opens: u64,
    failed_opens: u64,
    rolled_forward: u64,
    stale_probes: u64,
    branches_ok: u64,
    branches_failed: u64,
    chain_depth_max: u64,
    chain_too_deep: u64,
    children_retired: u64,
    child_opens: u64,
    compactions_ok: u64,
    compactions_failed: u64,
    reclaimed_bytes: u64,
    exports_ok: u64,
    exports_failed: u64,
    salvages_ok: u64,
    salvages_failed: u64,
    salvages_damaged: u64,
    migrations_ok: u64,
    migrations_failed: u64,
    max_file_len: u64,
    max_generation: u64,
    max_snapshots: u64,
    max_sessions: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Op {
    Write,
    Snapshot,
    Delete,
    Branch,
    Compact,
    Export,
    Salvage,
    Migrate,
    Restart,
    Children,
    Chain,
}

const OP_NAMES: [&str; 12] = [
    "write", "snapshot", "delete", "branch", "compact", "export", "salvage", "migrate", "restart",
    "child", "chain", "-",
];

enum Act {
    Write { off: usize, data: Vec<u8> },
    Snap,
    Del(u64),
}

struct Soak {
    seed: u64,
    cycle: usize,
    op: &'static str,
    rng: SplitMix64,
    inj: Rc<RefCell<Injector>>,
    live: Rc<Cell<usize>>,
    fs: Fs,
    main: Img,
    file: Option<File>,
    children: Vec<Child>,
    next_uid: u64,
    /// Every snapshot id of the main lineage ever seen.
    ever_ids: BTreeSet<u64>,
    need_compact: bool,
    log: VecDeque<String>,
    stats: Stats,
}

impl Soak {
    fn new(seed: u64) -> Self {
        let inj = Rc::new(RefCell::new(Injector::default()));
        let live = Rc::new(Cell::new(0usize));
        let handle = FaultHandle::new(&[], seed);
        let storage = CheckedStorage::new(SoakBackend::new(&handle, &inj, &live))
            .expect("capabilities are consistent");
        let (mut file, _ack) = SnapFile::create_base(storage, 1).expect("create base");
        file.set_capacity_policy(QUOTA).expect("policy");
        let mut fs = Fs::new();
        fs.insert(1, handle.clone());
        let main = Img {
            uid: 1,
            handle,
            acked: State::new(),
            gen_now: file.generation(),
            bound: 4096,
            slack: 0,
        };
        Self {
            seed,
            cycle: 0,
            op: "-",
            rng: SplitMix64(seed ^ 0x9E37_79B9_7F4A_7C15),
            inj,
            live,
            fs,
            main,
            file: Some(file),
            children: Vec::new(),
            next_uid: 2,
            ever_ids: BTreeSet::new(),
            need_compact: false,
            log: VecDeque::new(),
            stats: Stats::default(),
        }
    }

    // ------------------------------------------------------------ plumbing

    fn note(&mut self, line: impl Into<String>) {
        if self.log.len() >= 400 {
            self.log.pop_front();
        }
        let line = format!("c{} {}: {}", self.cycle, self.op, line.into());
        self.log.push_back(line);
    }

    fn fail(&self, msg: &str) -> ! {
        let mut out = format!(
            "S9-41 soak failure: {msg}\nseed={:#x} cycle={} op={}\nstats: {:?}\nharness log (last {REPORT_LINES}):",
            self.seed, self.cycle, self.op, self.stats
        );
        let skip = self.log.len().saturating_sub(REPORT_LINES);
        for l in self.log.iter().skip(skip) {
            out.push_str("\n  ");
            out.push_str(l);
        }
        let trace = self.main.handle.trace();
        out.push_str(&format!(
            "\nmain handle (uid {}) trace, last {REPORT_LINES} of {}:",
            self.main.uid,
            trace.len()
        ));
        for l in trace.iter().skip(trace.len().saturating_sub(REPORT_LINES)) {
            out.push_str("\n  ");
            out.push_str(l);
        }
        panic!("{out}");
    }

    fn pick(&mut self, n: usize) -> usize {
        self.rng.below(n as u64) as usize
    }

    fn fresh_uid(&mut self) -> u64 {
        let uid = self.next_uid;
        self.next_uid += 1;
        uid
    }

    fn fresh_handle(&mut self) -> FaultHandle {
        FaultHandle::new(&[], self.rng.next_u64())
    }

    fn st(&self, h: &FaultHandle) -> CheckedStorage<SoakBackend> {
        CheckedStorage::new(SoakBackend::new(h, &self.inj, &self.live))
            .expect("capabilities are consistent")
    }

    fn resolver(&self) -> FsResolver<'_> {
        FsResolver {
            fs: &self.fs,
            inj: Rc::clone(&self.inj),
            live: Rc::clone(&self.live),
        }
    }

    /// Starts a window. With probability `percent` one fault or crash is armed
    /// at a random mutating call among the first `reach` of the window.
    fn arm(&mut self, percent: u64, reach: u64) {
        if percent == 0 || self.rng.below(100) >= percent {
            return self.reset_window(None);
        }
        let at = self.rng.below(reach);
        let plan = if self.rng.below(100) < 30 {
            Plan::Crash(random_policy(&mut self.rng))
        } else if self.rng.below(100) < 20 {
            Plan::SlotTear(self.rng.next_u64())
        } else {
            let class = match self.rng.below(6) {
                0 => Class::Io,
                1 => Class::NoSpace,
                2 => Class::Short,
                3 => Class::Torn,
                4 => Class::FlushLost,
                _ => Class::FlushAfter,
            };
            Plan::Fault(class, self.rng.next_u64())
        };
        self.reset_window(Some((at, plan)));
    }

    fn reset_window(&mut self, plan: Option<(u64, Plan)>) {
        let mut inj = self.inj.borrow_mut();
        inj.plan = plan;
        inj.calls = 0;
        inj.window = Window::default();
    }

    /// Ends the window and returns what happened in it.
    fn disarm(&mut self) -> Window {
        let mut inj = self.inj.borrow_mut();
        inj.plan = None;
        let w = inj.window;
        drop(inj);
        if w.fired {
            if w.crashed {
                self.stats.crashes += 1;
            } else {
                self.stats.faults_fired += 1;
                self.stats.failing_faults += u64::from(w.failing);
            }
        }
        w
    }

    fn crash_random(&mut self, h: &FaultHandle) {
        let policy = random_policy(&mut self.rng);
        h.crash(policy);
    }

    /// Crashes `h` after a failed window unless the window already did. A torn
    /// root slot keeps its bytes so that the next open has to roll forward.
    fn crash_after(&mut self, h: &FaultHandle, w: Window) {
        if w.crashed {
            return;
        }
        self.stats.crashes += 1;
        if w.tear {
            self.stats.slot_tears += 1;
            h.crash(CrashPolicy::KeepPending);
        } else {
            self.crash_random(h);
        }
    }

    // ------------------------------------------------------------- main file

    /// Reopens the main image after a crash, possibly with faults inside the
    /// recovery itself, and resolves the oracle against what the media holds.
    fn reopen_main(&mut self, cand: Option<(State, u64)>, stale: Option<File>) {
        let mut attempt = 0;
        let (mut file, report) = loop {
            attempt += 1;
            if attempt > 8 {
                self.fail("recovery does not converge");
            }
            let percent = if attempt <= 3 {
                RECOVERY_FAULT_PERCENT
            } else {
                0
            };
            self.arm(percent, 3);
            self.stats.opens += 1;
            let storage = self.st(&self.main.handle.clone());
            let result = SnapFile::open(storage, &mut NoParents);
            let w = self.disarm();
            match result {
                Ok(v) => {
                    if w.failing {
                        self.fail("open succeeded although a failing fault fired");
                    }
                    break v;
                }
                Err(e) => {
                    if !w.fired {
                        self.fail(&format!("open refused without an injected fault: {e:?}"));
                    }
                    self.stats.failed_opens += 1;
                    self.note(format!("open attempt {attempt} failed: {e:?}"));
                    let h = self.main.handle.clone();
                    self.crash_after(&h, w);
                }
            }
        };
        if report.rolled_forward {
            self.stats.rolled_forward += 1;
        }
        file.set_capacity_policy(QUOTA).expect("policy");
        let landed = self.resolve(&file, cand.as_ref().map(|(s, _)| s));
        if let (true, Some((state, cost))) = (landed, cand) {
            self.main.acked = state;
            self.main.bound += cost;
        }
        self.note(format!(
            "reopened gen={} landed={landed} rolled_forward={} {}",
            file.generation(),
            report.rolled_forward,
            self.main.acked.summary()
        ));
        self.record_ids();
        self.probe_stale(stale);
        self.file = Some(file);
    }

    /// Compares what the media shows with the acknowledged state and with the
    /// candidate of a failed operation. Returns whether the candidate landed.
    fn resolve(&mut self, f: &File, cand: Option<&State>) -> bool {
        let obs = match observe(f) {
            Ok(o) => o,
            Err(e) => self.fail(&format!("recovered file does not read back: {e:?}")),
        };
        let is_acked = obs.same(&self.main.acked);
        let is_cand = cand.is_some_and(|c| obs.same(c));
        let (g, before) = (f.generation(), self.main.gen_now);
        let landed = match (is_acked, is_cand) {
            (false, false) => self.fail(&format!(
                "unexplained state after recovery: observed {}; acked {}; candidate {}",
                obs.summary(),
                self.main.acked.summary(),
                cand.map_or("none".to_string(), State::summary)
            )),
            (true, false) => false,
            (false, true) => true,
            (true, true) => g == before + 1,
        };
        let expected = before + u64::from(landed);
        if g != expected {
            self.fail(&format!(
                "generation {g} after recovery, expected {expected} (before {before}, landed {landed})"
            ));
        }
        self.main.gen_now = g;
        // `next_id` is not visible through the file API; the published header
        // carries it. It must be the one of the state that landed, never a
        // value from the other outcome (a reused or skipped id).
        let want = match (landed, cand) {
            (true, Some(c)) => c.next_id,
            _ => self.main.acked.next_id,
        };
        let h = self.main.handle.clone();
        match media_next_id(&h, g, f.file_uid()) {
            Some(n) if n == want => {}
            got => self.fail(&format!(
                "next snapshot id {got:?} after recovery, expected {want} (landed {landed})"
            )),
        }
        landed
    }

    /// Snapshot ids: bounded, below `next_id`, never reused.
    fn record_ids(&mut self) {
        let acked = &self.main.acked;
        if acked.snaps.len() > MAX_SNAPSHOTS {
            self.fail("more snapshots than MAX_SNAPSHOTS");
        }
        for id in acked.snaps.keys() {
            if *id >= acked.next_id {
                self.fail(&format!(
                    "snapshot id {id} not below next id {}",
                    acked.next_id
                ));
            }
        }
        let ids: Vec<u64> = acked.snaps.keys().copied().collect();
        self.ever_ids.extend(ids);
    }

    /// A writer from before the crash must not write to the recovered media.
    fn probe_stale(&mut self, stale: Option<File>) {
        let Some(old) = stale else {
            return;
        };
        let h = self.main.handle.clone();
        let snap = self.main.acked.snaps.keys().next().copied();
        self.probe_writer(old, &h, snap);
    }

    /// Every commit through `old` (opened before the crash of `h`) must fail
    /// and leave the media of `h` byte-identical, volatile and durable.
    fn probe_writer(&mut self, mut old: File, h: &FaultHandle, snap: Option<u64>) {
        self.reset_window(None);
        let (ordinal, volatile, durable) =
            (h.next_ordinal(), h.volatile_bytes(), h.durable_bytes());
        let mut results = vec![old.write_at(0, &[0xEE; 700]).map(|_| ())];
        results.push(old.create_snapshot().map(|_| ()));
        if let Some(id) = snap {
            results.push(old.delete_snapshot(id).map(|_| ()));
        }
        if results.iter().any(Result::is_ok) {
            self.fail("a writer from before the crash still acknowledged a commit");
        }
        if h.next_ordinal() != ordinal
            || h.volatile_bytes() != volatile
            || h.durable_bytes() != durable
        {
            self.fail("a writer from before the crash changed the recovered media");
        }
        self.stats.stale_probes += 1;
    }

    fn mutate(&mut self, act: Act) {
        let mut cand = self.main.acked.clone();
        let full = cand.snaps.len() >= MAX_SNAPSHOTS;
        let mut expect_id = 0;
        let cost = match &act {
            Act::Write { off, data } => {
                cand.write(*off, data);
                write_cost(*off, data.len(), cand.disk_blocks())
            }
            Act::Snap => {
                if !full {
                    expect_id = cand.next_id;
                    let head = cand.head.clone();
                    cand.snaps.insert(expect_id, head);
                    cand.next_id += 1;
                }
                meta_cost(cand.disk_blocks())
            }
            Act::Del(id) => {
                cand.snaps.remove(id);
                meta_cost(cand.disk_blocks())
            }
        };
        let Some(mut file) = self.file.take() else {
            self.fail("test bug: no open main file");
        };
        let handle = self.main.handle.clone();
        let ordinal = handle.next_ordinal();
        let gen_before = file.generation();
        self.arm(FAULT_PERCENT, 14);
        let result = match &act {
            Act::Write { off, data } => file.write_at(*off as u64, data).map(|_| 0),
            Act::Snap => file.create_snapshot().map(|(id, _)| id),
            Act::Del(id) => file.delete_snapshot(*id).map(|_| 0),
        };
        let w = self.disarm();
        self.main.slack = self.main.slack.max(cost);
        match result {
            Ok(id) => {
                if w.failing || w.crashed {
                    self.fail("commit acknowledged although a failing fault or crash fired");
                }
                if matches!(act, Act::Snap) {
                    if id != expect_id {
                        self.fail(&format!("snapshot id {id}, oracle expects {expect_id}"));
                    }
                    if !self.ever_ids.insert(id) {
                        self.fail(&format!("snapshot id {id} reused"));
                    }
                }
                if file.generation() != gen_before + 1 {
                    self.fail(&format!(
                        "generation {} after one commit from {gen_before}",
                        file.generation()
                    ));
                }
                self.main.acked = cand;
                self.main.gen_now = gen_before + 1;
                self.main.bound += cost;
                self.stats.acked_commits += 1;
                self.record_ids();
                self.file = Some(file);
            }
            Err(e) => {
                let untouched = !w.fired && handle.next_ordinal() == ordinal;
                if untouched {
                    let expected = match (&act, &e) {
                        (Act::Snap, SnapError::SnapshotLimit) if full => {
                            self.stats.snapshot_limit_hits += 1;
                            true
                        }
                        (_, SnapError::QuotaExceeded { .. }) => {
                            self.stats.quota_rejects += 1;
                            self.need_compact = true;
                            true
                        }
                        _ => false,
                    };
                    if !expected || file.is_poisoned() {
                        self.fail(&format!("unexpected rejection {e:?}"));
                    }
                    self.stats.rejected_ops += 1;
                    self.file = Some(file);
                } else {
                    if !w.fired {
                        self.fail(&format!("error without an injected fault: {e:?}"));
                    }
                    self.stats.failed_ops += 1;
                    self.note(format!("op failed: {e:?}"));
                    self.crash_after(&handle, w);
                    self.reopen_main(Some((cand, cost)), Some(file));
                }
            }
        }
        self.retire_orphans();
    }

    fn do_restart(&mut self) {
        let stale = self.file.take();
        let h = self.main.handle.clone();
        self.crash_random(&h);
        self.reopen_main(None, stale);
    }

    // -------------------------------------------------------------- children

    fn open_child(&mut self, i: usize) -> Result<File, SnapError> {
        let h = self.children[i].handle.clone();
        self.crash_random(&h);
        self.stats.child_opens += 1;
        let storage = self.st(&h);
        let mut res = self.resolver();
        SnapFile::open(storage, &mut res).map(|(f, _)| f)
    }

    fn child_index(&self, uid: u64) -> Option<usize> {
        self.children.iter().position(|c| c.uid == uid)
    }

    /// Children whose chain no longer reaches a live parent snapshot.
    fn broken_children(&self) -> Vec<usize> {
        let direct = |c: &Child| {
            if c.parent_uid == self.main.uid {
                !self.main.acked.snaps.contains_key(&c.parent_snap)
            } else {
                self.child_index(c.parent_uid).is_none()
            }
        };
        let mut broken: BTreeSet<usize> = (0..self.children.len())
            .filter(|i| direct(&self.children[*i]))
            .collect();
        loop {
            let before = broken.len();
            for (i, c) in self.children.iter().enumerate() {
                let parent_broken = self
                    .child_index(c.parent_uid)
                    .is_some_and(|p| broken.contains(&p));
                if parent_broken {
                    broken.insert(i);
                }
            }
            if broken.len() == before {
                break;
            }
        }
        broken.into_iter().collect()
    }

    /// A deleted parent snapshot must make its children (and theirs) refuse
    /// to open; they are then dropped.
    fn retire_orphans(&mut self) {
        let broken = self.broken_children();
        for i in &broken {
            if let Ok(f) = self.open_child(*i) {
                drop(f);
                self.fail(&format!(
                    "child {} opened although its parent snapshot is gone",
                    self.children[*i].uid
                ));
            }
        }
        self.remove_children(&broken);
    }

    fn remove_children(&mut self, indexes: &[usize]) {
        let mut sorted = indexes.to_vec();
        sorted.sort_unstable();
        for i in sorted.into_iter().rev() {
            let c = self.children.remove(i);
            self.fs.remove(&c.uid);
            self.stats.children_retired += 1;
        }
    }

    /// Drops leaf children, oldest first, until there is room for one more.
    fn make_room(&mut self, keep: u64) {
        while self.children.len() >= MAX_CHILDREN {
            let leaf = (0..self.children.len()).find(|i| {
                let uid = self.children[*i].uid;
                uid != keep && !self.children.iter().any(|c| c.parent_uid == uid)
            });
            match leaf {
                Some(i) => self.remove_children(&[i]),
                None => self.fail("test bug: children without a leaf"),
            }
        }
    }

    /// Branches from snapshot `snap` of the main image (`from == None`) or of
    /// child `from`. Returns the index of the new child.
    fn branch(&mut self, from: Option<usize>, snap: u64, fault_percent: u64) -> Option<usize> {
        let (src_uid, src_depth, expected) = match from {
            None => (
                self.main.uid,
                0u32,
                self.main.acked.snaps.get(&snap).cloned(),
            ),
            Some(i) => {
                let c = &self.children[i];
                (c.uid, c.depth, c.model.snaps.get(&snap).cloned())
            }
        };
        let Some(expected) = expected else {
            self.fail("test bug: branch from an unknown snapshot");
        };
        let src_child = match from {
            None => None,
            Some(i) => match self.open_child(i) {
                Ok(f) => Some(f),
                Err(e) => self.fail(&format!("branch source child refused to open: {e:?}")),
            },
        };
        self.make_room(src_uid);
        let uid = self.fresh_uid();
        let handle = self.fresh_handle();
        let storage = self.st(&handle);
        self.arm(fault_percent, 10);
        let result = {
            let source: &File = match (&src_child, &self.file) {
                (Some(c), _) => c,
                (None, Some(m)) => m,
                (None, None) => self.fail("test bug: no open main file"),
            };
            let mut res = self.resolver();
            SnapFile::branch_from(storage, &mut res, source, snap, uid)
        };
        let w = self.disarm();
        drop(src_child);
        match result {
            Ok((cf, _ack)) => {
                if w.failing || w.crashed {
                    self.fail("branch created although a failing fault or crash fired");
                }
                let obs = match observe(&cf) {
                    Ok(o) => o,
                    Err(e) => self.fail(&format!("new branch does not read back: {e:?}")),
                };
                if obs.head != expected || !obs.snaps.is_empty() {
                    self.fail("new branch does not show exactly the parent snapshot");
                }
                if cf.depth() != src_depth + 1 {
                    self.fail("branch depth differs from the oracle");
                }
                drop(cf);
                self.fs.insert(uid, handle.clone());
                let model = State {
                    head: expected,
                    snaps: BTreeMap::new(),
                    next_id: 1,
                };
                self.children.push(Child {
                    uid,
                    handle,
                    parent_uid: src_uid,
                    parent_snap: snap,
                    depth: src_depth + 1,
                    model,
                });
                self.stats.branches_ok += 1;
                self.stats.chain_depth_max =
                    self.stats.chain_depth_max.max(u64::from(src_depth + 1));
                Some(self.children.len() - 1)
            }
            Err(e) => {
                if matches!(e, SnapError::ChainTooDeep { .. }) && src_depth + 1 > MAX_CHAIN_DEPTH {
                    self.stats.chain_too_deep += 1;
                    if handle.next_ordinal() != 0 {
                        self.fail("a too deep branch wrote to its target");
                    }
                    return None;
                }
                if !w.fired {
                    self.fail(&format!("branch failed without an injected fault: {e:?}"));
                }
                self.stats.branches_failed += 1;
                self.crash_after(&handle, w);
                // An unpublished branch either does not open or shows exactly
                // the parent snapshot.
                let storage = self.st(&handle);
                let opened = {
                    let mut res = self.resolver();
                    SnapFile::open(storage, &mut res)
                };
                if let Ok((f, _)) = opened {
                    match observe(&f) {
                        Ok(o) if o.head == expected && o.snaps.is_empty() => {}
                        _ => self.fail("failed branch opens with other content"),
                    }
                }
                None
            }
        }
    }

    fn child_op(&mut self) {
        if self.children.is_empty() {
            return;
        }
        let i = self.pick(self.children.len());
        let mut f = match self.open_child(i) {
            Ok(f) => f,
            Err(e) => self.fail(&format!(
                "child {} refused to open: {e:?}",
                self.children[i].uid
            )),
        };
        match observe(&f) {
            Ok(o) if o.same(&self.children[i].model) => {}
            Ok(_) => self.fail(&format!(
                "child {} content differs from its oracle",
                self.children[i].uid
            )),
            Err(e) => self.fail(&format!("child read failed: {e:?}")),
        }
        match self.pick(3) {
            0 if self.children[i].model.snaps.len() < MAX_SNAPSHOTS - 1 => {
                let (id, _ack) = match f.create_snapshot() {
                    Ok(v) => v,
                    Err(e) => self.fail(&format!("child snapshot failed: {e:?}")),
                };
                let c = &mut self.children[i];
                if id != c.model.next_id {
                    self.fail("child snapshot id differs from the oracle");
                }
                let head = c.model.head.clone();
                c.model.snaps.insert(id, head);
                c.model.next_id += 1;
            }
            _ => self.child_write(i, f),
        }
    }

    fn child_write(&mut self, i: usize, mut f: File) {
        let len = 1 + self.pick(1500);
        let off = self.pick(DISK_BLOCKS_MAX * BS - len + 1);
        let data = self.rng.bytes(len);
        let mut cand = self.children[i].model.clone();
        cand.write(off, &data);
        self.arm(FAULT_PERCENT, 10);
        let result = f.write_at(off as u64, &data);
        let w = self.disarm();
        match result {
            Ok(_) => {
                if w.failing || w.crashed {
                    self.fail("child write acknowledged despite a failing fault or crash");
                }
                self.children[i].model = cand;
            }
            Err(e) => {
                if !w.fired {
                    self.fail(&format!("child write failed without a fault: {e:?}"));
                }
                let h = self.children[i].handle.clone();
                self.crash_after(&h, w);
                let again = match self.open_child_no_crash(i) {
                    Ok(f) => f,
                    Err(e) => self.fail(&format!("child did not recover: {e:?}")),
                };
                match observe(&again) {
                    Ok(o) if o.same(&self.children[i].model) => {}
                    Ok(o) if o.same(&cand) => self.children[i].model = cand,
                    _ => self.fail("child state after a failed write is neither old nor new"),
                }
                drop(again);
                // The writer from before the crash is stale now.
                self.probe_writer(f, &h, None);
            }
        }
    }

    fn open_child_no_crash(&mut self, i: usize) -> Result<File, SnapError> {
        let h = self.children[i].handle.clone();
        self.stats.child_opens += 1;
        let storage = self.st(&h);
        let mut res = self.resolver();
        SnapFile::open(storage, &mut res).map(|(f, _)| f)
    }

    /// Builds a chain of branches to the depth limit and one past it, with
    /// distinct data at every level, and reads the deepest file back.
    fn chain(&mut self) {
        let Some(&first) = self.main.acked.snaps.keys().next() else {
            return;
        };
        // Start from a clean set of children so the chain is the only one.
        let all: Vec<usize> = (0..self.children.len()).collect();
        for i in &all {
            if self.open_child(*i).is_err() {
                self.fail("child lost before a chain run");
            }
        }
        self.remove_children(&all);
        let mut at = self.branch(None, first, 0);
        let mut depth = 1u32;
        while let Some(i) = at {
            // Distinct data at every level, then a snapshot to branch from.
            let mut f = match self.open_child(i) {
                Ok(f) => f,
                Err(e) => self.fail(&format!("chain child refused to open: {e:?}")),
            };
            let at_block = (depth as usize) % DISK_BLOCKS_MAX;
            let data = self.rng.bytes(BS);
            if f.write_at((at_block * BS) as u64, &data).is_err() {
                self.fail("chain child write failed without a fault");
            }
            self.children[i].model.write(at_block * BS, &data);
            let (id, _ack) = match f.create_snapshot() {
                Ok(v) => v,
                Err(e) => self.fail(&format!("chain snapshot failed: {e:?}")),
            };
            let head = self.children[i].model.head.clone();
            self.children[i].model.snaps.insert(id, head);
            self.children[i].model.next_id += 1;
            drop(f);
            if depth > MAX_CHAIN_DEPTH + 1 {
                self.fail("chain grew past the depth limit");
            }
            let next = self.branch(Some(i), id, 0);
            // `branch` may have dropped children; find the source again by uid.
            match next {
                Some(n) => {
                    at = Some(n);
                    depth += 1;
                }
                None => {
                    // Either a faulted branch or the depth limit; the deepest
                    // child must still read back.
                    let deepest = self.children.len().checked_sub(1);
                    if let Some(d) = deepest {
                        let f = match self.open_child(d) {
                            Ok(f) => f,
                            Err(e) => self.fail(&format!("deepest chain child: {e:?}")),
                        };
                        match observe(&f) {
                            Ok(o) if o.same(&self.children[d].model) => {}
                            _ => self.fail("deepest chain child differs from the oracle"),
                        }
                    }
                    at = None;
                }
            }
        }
    }

    // ------------------------------------------------------------ compaction

    fn compact(&mut self) {
        let new_uid = self.fresh_uid();
        let target = self.fresh_handle();
        let storage = self.st(&target);
        let before = target_len(&self.main.handle);
        self.arm(FAULT_PERCENT, 40);
        let result = match &self.file {
            Some(f) => f.compact_to(storage, new_uid),
            None => self.fail("test bug: no open main file"),
        };
        let w = self.disarm();
        match result {
            Ok((built, _ack)) => {
                if w.failing || w.crashed {
                    self.fail("compaction published although a failing fault or crash fired");
                }
                drop(built);
                // Acknowledged: durable. Reopen from the media and compare.
                target.crash(CrashPolicy::DropPending);
                self.stats.opens += 1;
                let storage = self.st(&target);
                let mut next = match SnapFile::open(storage, &mut NoParents) {
                    Ok((f, _)) => f,
                    Err(e) => self.fail(&format!("acknowledged compaction does not open: {e:?}")),
                };
                match observe(&next) {
                    Ok(o) if o.same(&self.main.acked) && next.file_uid() == new_uid => {}
                    _ => self.fail("compacted file differs from the oracle"),
                }
                // The source was only read: children still resolve against it.
                for i in 0..self.children.len() {
                    if let Err(e) = self.open_child(i) {
                        self.fail(&format!(
                            "compaction disturbed a child of the source: {e:?}"
                        ));
                    }
                }
                let all: Vec<usize> = (0..self.children.len()).collect();
                self.remove_children(&all);
                next.set_capacity_policy(QUOTA).expect("policy");
                let after = target_len(&target);
                if after > live_bound(&self.main.acked) {
                    self.fail(&format!(
                        "compacted file {after} B above the bound {} B",
                        live_bound(&self.main.acked)
                    ));
                }
                self.stats.reclaimed_bytes += before.saturating_sub(after);
                self.stats.compactions_ok += 1;
                self.fs.remove(&self.main.uid);
                self.fs.insert(new_uid, target.clone());
                self.main = Img {
                    uid: new_uid,
                    handle: target,
                    acked: self.main.acked.clone(),
                    gen_now: next.generation(),
                    bound: live_bound(&self.main.acked),
                    slack: self.main.slack,
                };
                self.file = Some(next);
                self.need_compact = false;
                self.note(format!("compacted {before}B -> {after}B"));
            }
            Err(e) => {
                if !w.fired {
                    self.fail(&format!(
                        "compaction failed without an injected fault: {e:?}"
                    ));
                }
                self.stats.compactions_failed += 1;
                self.crash_after(&target, w);
                // The unpublished target does not open, or is complete.
                let storage = self.st(&target);
                self.stats.opens += 1;
                if let Ok((f, _)) = SnapFile::open(storage, &mut NoParents) {
                    match observe(&f) {
                        Ok(o) if o.same(&self.main.acked) && f.file_uid() == new_uid => {}
                        _ => self.fail("failed compaction target opens with other content"),
                    }
                }
                self.note(format!("compaction failed: {e:?}"));
            }
        }
    }

    // ---------------------------------------------------------- export, etc.

    fn selection(&mut self) -> (StateSelection, Vec<u8>) {
        let ids: Vec<u64> = self.main.acked.snaps.keys().copied().collect();
        if ids.is_empty() || self.rng.below(100) < 55 {
            return (StateSelection::Head, self.main.acked.head.clone());
        }
        let id = ids[self.pick(ids.len())];
        (
            StateSelection::Snapshot(id),
            self.main.acked.snaps[&id].clone(),
        )
    }

    fn export(&mut self) {
        let (sel, expected) = self.selection();
        let target = self.fresh_handle();
        let source = self.st(&self.main.handle.clone());
        let tstorage = self.st(&target);
        self.arm(FAULT_PERCENT, 12);
        let result = export_raw(
            source,
            &mut NoParents,
            sel,
            || Ok(tstorage),
            &ExportOptions::default(),
            &|| false,
        );
        let w = self.disarm();
        match result {
            Ok(report) => {
                if w.failing || w.crashed {
                    self.fail("export succeeded although a failing fault or crash fired");
                }
                if target.durable_bytes() != expected
                    || report.logical_size != expected.len() as u64
                    || report.source_uid != self.main.uid
                {
                    self.fail("exported RAW differs from the oracle");
                }
                self.stats.exports_ok += 1;
            }
            Err(e) => {
                if !w.fired {
                    self.fail(&format!("export failed without an injected fault: {e:?}"));
                }
                self.stats.exports_failed += 1;
            }
        }
    }

    fn salvage(&mut self) {
        let (sel, expected) = self.selection();
        let mut bytes = self.main.handle.durable_bytes();
        let damaged = !bytes.is_empty() && self.rng.below(100) < 25;
        if damaged {
            let at = self.pick(bytes.len());
            bytes[at] ^= 1 << self.pick(8);
            self.stats.salvages_damaged += 1;
        }
        let source_handle = FaultHandle::new(&bytes, self.rng.next_u64());
        let target = self.fresh_handle();
        let uid = self.fresh_uid();
        let source = self.st(&source_handle);
        let tstorage = self.st(&target);
        self.arm(if damaged { 0 } else { FAULT_PERCENT }, 12);
        let options = SalvageOptions {
            selection: sel,
            publish_partial: false,
        };
        let result = salvage_to_copy(source, &mut NoParents, &options, uid, || Ok(tstorage));
        let w = self.disarm();
        let outcome = match result {
            Ok(o) => o,
            Err(e) => self.fail(&format!("salvage analysis failed on readable media: {e:?}")),
        };
        if source_handle.volatile_bytes() != bytes || source_handle.next_ordinal() != 0 {
            self.fail("salvage changed its source");
        }
        let report = &outcome.report;
        let complete =
            report.recoverability == Recoverability::Complete && !report.possible_lost_commit;
        if !damaged && !complete {
            self.fail(&format!(
                "clean copy is not a complete recovery: {}",
                report.reason
            ));
        }
        match (report.copy, &outcome.copy) {
            (CopyStatus::Published, Some(copy)) => {
                if w.failing || w.crashed {
                    self.fail("salvage published although a failing fault or crash fired");
                }
                let mut got = vec![0u8; copy.disk_size() as usize];
                if copy.read_at(0, &mut got).is_err() {
                    self.fail("salvage copy does not read back");
                }
                // Damage may hit unreferenced bytes only when the recovery is
                // complete; a complete recovery must equal the oracle.
                if complete && got != expected {
                    self.fail("complete salvage differs from the oracle");
                }
                self.stats.salvages_ok += 1;
            }
            (CopyStatus::Failed(why), None) => {
                if !w.fired && !damaged {
                    self.fail(&format!("salvage copy failed without a fault: {why}"));
                }
                self.stats.salvages_failed += 1;
                self.crash_after(&target, w);
                let storage = self.st(&target);
                if let Ok((f, _)) = SnapFile::open(storage, &mut NoParents) {
                    let mut got = vec![0u8; f.disk_size() as usize];
                    if f.read_at(0, &mut got).is_err() || (complete && got != expected) {
                        self.fail("failed salvage target opens with other content");
                    }
                }
            }
            (CopyStatus::NotCreated(_), None) => {
                if !damaged {
                    self.fail("clean copy was not salvaged");
                }
                self.stats.salvages_failed += 1;
            }
            _ => self.fail("salvage report and copy disagree"),
        }
    }

    /// RAW -> CoW -> RAW -> CoW ... : repeated migration of the head.
    fn migrate(&mut self) {
        if self.main.acked.head.is_empty() {
            return;
        }
        let expected = self.main.acked.head.clone();
        let mut source_handle = self.main.handle.clone();
        for round in 0..3 {
            let raw = self.fresh_handle();
            let rstorage = self.st(&raw);
            let source = self.st(&source_handle);
            self.reset_window(None);
            let exported = export_raw(
                source,
                &mut NoParents,
                StateSelection::Head,
                || Ok(rstorage),
                &ExportOptions::default(),
                &|| false,
            );
            if exported.is_err() || raw.durable_bytes() != expected {
                self.fail(&format!(
                    "migration round {round}: export differs or failed"
                ));
            }
            let image = self.fresh_handle();
            let uid = self.fresh_uid();
            let raw_storage = self.st(&raw);
            let istorage = self.st(&image);
            self.arm(if round == 0 { FAULT_PERCENT } else { 0 }, 30);
            let imported = import_raw(&raw_storage, || Ok(istorage), uid, &|| false);
            let w = self.disarm();
            match imported {
                Ok((built, _)) => {
                    if w.failing || w.crashed {
                        self.fail("import succeeded although a failing fault or crash fired");
                    }
                    drop(built);
                    image.crash(CrashPolicy::DropPending);
                    let storage = self.st(&image);
                    self.stats.opens += 1;
                    let reopened = match SnapFile::open(storage, &mut NoParents) {
                        Ok((f, _)) => f,
                        Err(e) => self.fail(&format!("imported image does not open: {e:?}")),
                    };
                    match observe(&reopened) {
                        Ok(o) if o.head == expected && o.snaps.is_empty() => {}
                        _ => self.fail("imported image differs from the oracle"),
                    }
                    drop(reopened);
                    source_handle = image;
                }
                Err(e) => {
                    if !w.fired {
                        self.fail(&format!("import failed without an injected fault: {e:?}"));
                    }
                    self.stats.migrations_failed += 1;
                    self.crash_after(&image, w);
                    let storage = self.st(&image);
                    if let Ok((f, _)) = SnapFile::open(storage, &mut NoParents) {
                        match observe(&f) {
                            Ok(o) if o.head == expected => {}
                            _ => self.fail("failed import target opens with other content"),
                        }
                    }
                    return;
                }
            }
        }
        self.stats.migrations_ok += 1;
    }

    // -------------------------------------------------------------- the loop

    fn choose(&mut self) -> Op {
        if self.need_compact {
            return Op::Compact;
        }
        let has_snaps = !self.main.acked.snaps.is_empty();
        let op = match self.rng.below(100) {
            0..=36 => Op::Write,
            37..=46 => Op::Snapshot,
            47..=53 => Op::Delete,
            54..=60 => Op::Branch,
            61..=63 => Op::Compact,
            64..=72 => Op::Export,
            73..=78 => Op::Salvage,
            79..=81 => Op::Migrate,
            82..=86 => Op::Restart,
            87..=94 => Op::Children,
            _ => Op::Chain,
        };
        match op {
            Op::Delete | Op::Branch | Op::Chain if !has_snaps => Op::Snapshot,
            Op::Children if self.children.is_empty() => Op::Branch,
            other => other,
        }
    }

    fn step(&mut self) {
        let op = self.choose();
        self.op = OP_NAMES[op as usize];
        self.stats.ops[op as usize] += 1;
        self.note("start");
        match op {
            Op::Write => {
                let len = 1 + self.pick(2000);
                let off = self.pick(DISK_BLOCKS_MAX * BS - len + 1);
                let data = self.rng.bytes(len);
                self.mutate(Act::Write { off, data });
            }
            Op::Snapshot => self.mutate(Act::Snap),
            Op::Delete => {
                let ids: Vec<u64> = self.main.acked.snaps.keys().copied().collect();
                let id = ids[self.pick(ids.len())];
                self.mutate(Act::Del(id));
            }
            Op::Branch => {
                let ids: Vec<u64> = self.main.acked.snaps.keys().copied().collect();
                if ids.is_empty() {
                    self.mutate(Act::Snap);
                } else {
                    let id = ids[self.pick(ids.len())];
                    let _ = self.branch(None, id, FAULT_PERCENT);
                }
            }
            Op::Compact => self.compact(),
            Op::Export => self.export(),
            Op::Salvage => self.salvage(),
            Op::Migrate => self.migrate(),
            Op::Restart => self.do_restart(),
            Op::Children => self.child_op(),
            Op::Chain => self.chain(),
        }
        self.check_cycle();
    }

    /// Invariants that must hold between any two cycles.
    fn check_cycle(&mut self) {
        self.reset_window(None);
        let Some(file) = self.file.take() else {
            self.fail("no open main file at the end of a cycle");
        };
        if file.is_poisoned() {
            self.fail("main file is poisoned between cycles");
        }
        // Live content equals the oracle (head and every snapshot).
        match observe(&file) {
            Ok(o) if o.same(&self.main.acked) => {}
            Ok(o) => self.fail(&format!(
                "live state differs from the oracle: observed {}; oracle {}",
                o.summary(),
                self.main.acked.summary()
            )),
            Err(e) => self.fail(&format!("live read failed: {e:?}")),
        }
        let ids: Vec<u64> = file.snapshot_ids();
        let want: Vec<u64> = self.main.acked.snaps.keys().copied().collect();
        if ids != want || ids.len() > MAX_SNAPSHOTS {
            self.fail(&format!("snapshot ids {ids:?}, oracle {want:?}"));
        }
        // Generation: monotonic, in step with the oracle, far from overflow.
        let g = file.generation();
        if g != self.main.gen_now
            || !(CREATE_GENERATION..=u64::MAX - GENERATION_MARGIN).contains(&g)
        {
            self.fail(&format!("generation {g}, tracked {}", self.main.gen_now));
        }
        // Next snapshot id of the published header equals the oracle's after
        // every cycle, acknowledged or not (localises a drift to its cycle).
        let h = self.main.handle.clone();
        let want = self.main.acked.next_id;
        match media_next_id(&h, g, file.file_uid()) {
            Some(n) if n == want => {}
            got => self.fail(&format!("next snapshot id {got:?}, oracle {want}")),
        }
        // Space: the block count is bounded by the disk, the file by the
        // acknowledged commits and by the hard quota.
        let blocks = file.file_block_count_for_tests();
        if blocks > DATA_BASE + DISK_BLOCKS_MAX as u64 {
            self.fail(&format!("block count {blocks} above the disk size"));
        }
        let len = target_len(&self.main.handle);
        if len > QUOTA.max_file_bytes {
            self.fail(&format!("file {len} B above the quota"));
        }
        if len > self.main.bound + self.main.slack {
            self.fail(&format!(
                "file {len} B above the bound {} B + {} B",
                self.main.bound, self.main.slack
            ));
        }
        self.stats.max_file_len = self.stats.max_file_len.max(len);
        self.stats.max_generation = self.stats.max_generation.max(g);
        self.stats.max_snapshots = self.stats.max_snapshots.max(ids.len() as u64);
        // Handles: only the main writer is open; the catalogue is bounded.
        let sessions = self.live.get();
        self.stats.max_sessions = self.stats.max_sessions.max(sessions as u64);
        if sessions != 1 {
            self.fail(&format!(
                "{sessions} sessions open at the end of a cycle, expected 1"
            ));
        }
        if self.fs.len() != self.children.len() + 1 || self.children.len() > MAX_CHILDREN {
            self.fail("catalogue grew beyond the live files");
        }
        self.file = Some(file);
        self.stats.cycles += 1;
    }

    fn finish(&mut self) {
        self.op = "final";
        self.note("final crash, reopen and verification");
        self.do_restart();
        for i in 0..self.children.len() {
            match self.open_child(i) {
                Ok(f) => match observe(&f) {
                    Ok(o) if o.same(&self.children[i].model) => {}
                    _ => self.fail("child differs from its oracle at the end"),
                },
                Err(e) => self.fail(&format!("child refused to open at the end: {e:?}")),
            }
        }
        self.check_cycle();
        self.file = None;
        if self.live.get() != 0 {
            self.fail("sessions left open after the run");
        }
        // Ids of the main lineage are strictly below the next id.
        let next_id = self.main.acked.next_id;
        if self
            .ever_ids
            .iter()
            .next_back()
            .is_some_and(|max| *max >= next_id)
        {
            self.fail("a published snapshot id is not below the next id");
        }
    }
}

/// Current size of a file in bytes (volatile view).
fn target_len(h: &FaultHandle) -> u64 {
    h.volatile_bytes().len() as u64
}

fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name).ok()?.trim().parse().ok()
}

fn env_seed() -> Option<u64> {
    let raw = std::env::var("OD_SOAK_SEED").ok()?;
    let raw = raw.trim();
    match raw.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => raw.parse().ok(),
    }
}

/// Runs the soak for `cycles` cycles of one seed.
fn run(seed: u64, cycles: usize) -> Stats {
    let mut soak = Soak::new(seed);
    for cycle in 0..cycles {
        soak.cycle = cycle;
        soak.step();
    }
    soak.cycle = cycles;
    soak.finish();
    soak.stats
}

fn print_report(seed: u64, cycles: usize, stats: &Stats, millis: u128) {
    eprintln!(
        "soak seed={seed:#x} cycles={cycles} ({millis} ms): result=ok\n  ops: {}\n  {stats:#?}",
        OP_NAMES
            .iter()
            .zip(stats.ops.iter())
            .filter(|(_, n)| **n > 0)
            .map(|(name, n)| format!("{name}={n}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
}

/// Sums of the numbers that decide whether the run reached what it claims.
#[derive(Default)]
struct Totals {
    crashes: u64,
    faults: u64,
    slot_tears: u64,
    opens: u64,
    failed_opens: u64,
    rolled_forward: u64,
    stale_probes: u64,
    compactions: u64,
    reclaimed_bytes: u64,
    quota_rejects: u64,
    snapshot_limit_hits: u64,
    exports: u64,
    salvages: u64,
    migrations: u64,
    branches: u64,
    chain_too_deep: u64,
    children_retired: u64,
}

impl Totals {
    fn add(&mut self, s: &Stats) {
        self.crashes += s.crashes;
        self.faults += s.faults_fired;
        self.slot_tears += s.slot_tears;
        self.opens += s.opens;
        self.failed_opens += s.failed_opens;
        self.rolled_forward += s.rolled_forward;
        self.stale_probes += s.stale_probes;
        self.compactions += s.compactions_ok;
        self.reclaimed_bytes += s.reclaimed_bytes;
        self.quota_rejects += s.quota_rejects;
        self.snapshot_limit_hits += s.snapshot_limit_hits;
        self.exports += s.exports_ok;
        self.salvages += s.salvages_ok;
        self.migrations += s.migrations_ok;
        self.branches += s.branches_ok;
        self.chain_too_deep += s.chain_too_deep;
        self.children_retired += s.children_retired;
    }
}

/// The default scope: [`SOAK_CYCLES`] cycles for each of [`SOAK_SEEDS`].
#[test]
fn bounded_soak_with_repeated_recovery_has_no_unexplained_loss() {
    let cycles = env_usize("OD_SOAK_CYCLES").unwrap_or(SOAK_CYCLES);
    let seeds: Vec<u64> = match env_seed() {
        Some(s) => vec![s],
        None => SOAK_SEEDS.to_vec(),
    };
    let report = std::env::var("OD_SOAK_REPORT").is_ok_and(|v| v == "1");
    let mut total = Totals::default();
    for seed in &seeds {
        let started = std::time::Instant::now();
        let stats = run(*seed, cycles);
        if report {
            print_report(*seed, cycles, &stats, started.elapsed().as_millis());
        }
        total.add(&stats);
    }
    if report {
        eprintln!(
            "soak summary: seeds={} cycles/seed={cycles} crashes={} faults={} slot_tears={} opens={} \
             failed_opens={} rolled_forward={} stale_probes={} compactions={} reclaimed_bytes={} \
             quota_rejects={} snapshot_limit_hits={} exports={} salvages={} migrations={} \
             branches={} chain_too_deep={} children_retired={} result=ok",
            seeds.len(),
            total.crashes,
            total.faults,
            total.slot_tears,
            total.opens,
            total.failed_opens,
            total.rolled_forward,
            total.stale_probes,
            total.compactions,
            total.reclaimed_bytes,
            total.quota_rejects,
            total.snapshot_limit_hits,
            total.exports,
            total.salvages,
            total.migrations,
            total.branches,
            total.chain_too_deep,
            total.children_retired,
        );
    }
    // The default scope must actually reach the paths it claims to measure.
    if cycles >= SOAK_CYCLES && seeds.len() == SOAK_SEEDS.len() {
        assert!(total.crashes >= 100, "crashes {}", total.crashes);
        assert!(total.faults >= 100, "faults {}", total.faults);
        assert!(total.opens >= 200, "opens {}", total.opens);
        assert!(total.slot_tears >= 5, "slot tears {}", total.slot_tears);
        assert!(
            total.rolled_forward >= 3,
            "roll-forward {}",
            total.rolled_forward
        );
        assert!(total.compactions >= 5, "compactions {}", total.compactions);
        assert!(
            total.stale_probes >= 50,
            "stale probes {}",
            total.stale_probes
        );
        assert!(total.exports >= 10, "exports {}", total.exports);
        assert!(total.salvages >= 5, "salvages {}", total.salvages);
        assert!(total.migrations >= 3, "migrations {}", total.migrations);
        assert!(total.branches >= 20, "branches {}", total.branches);
        assert!(
            total.children_retired >= 1,
            "retired {}",
            total.children_retired
        );
        assert!(total.snapshot_limit_hits >= 1, "snapshot limit never hit");
        assert!(total.quota_rejects >= 1, "quota never hit");
        assert!(total.chain_too_deep >= 1, "chain depth limit never hit");
    }
}

// ------------------------------------------------- generation exhaustion

fn fst(h: &FaultHandle) -> CheckedStorage<FaultBackend> {
    CheckedStorage::new(h.open()).expect("capabilities are consistent")
}

/// Policy at the top of the generation range: a commit that would reach
/// `u64::MAX` is refused before any backend call and poisons nothing; every
/// acknowledged state stays readable across crashes; the way out is a
/// compaction into a new file, which restarts the generation and keeps the
/// snapshot ids. The file is forged by moving the roots of a real image to
/// `u64::MAX - 3` (the existing proto test does the same for a bare CoW file).
#[test]
fn generation_exhaustion_is_refused_before_io_and_compaction_continues() {
    use crate::cow_proto::{CowView, forge_root_sector, slot_offset};
    let mut rng = SplitMix64(0x6E4);
    let h0 = FaultHandle::new(&[], 0x6E4);
    let (mut f, _) = SnapFile::create_base(fst(&h0), 7).expect("create");
    let mut model = State::new();
    for round in 0..3 {
        let data = rng.bytes(BS + 100);
        f.write_at((round * BS) as u64, &data).expect("write");
        model.write(round * BS, &data);
        let (id, _) = f.create_snapshot().expect("snapshot");
        model.snaps.insert(id, model.head.clone());
        model.next_id = id + 1;
    }
    drop(f);
    h0.crash(CrashPolicy::DropPending);
    let bytes = h0.durable_bytes();
    let (view, _) = CowView::salvage(fst(&h0)).expect("view");
    let top = u64::MAX - 3;
    let root = |g: u64| {
        forge_root_sector(
            g,
            view.block_count(),
            view.map_offset(),
            view.map_crc(),
            view.end_offset(),
        )
    };
    let mut forged = bytes.clone();
    for g in [top - 1, top] {
        let at = slot_offset(g) as usize;
        forged[at..at + 512].copy_from_slice(&root(g));
    }
    drop(view);

    let h = FaultHandle::new(&forged, 0x6E5);
    let open = |h: &FaultHandle| SnapFile::open(fst(h), &mut NoParents).map(|(f, _)| f);
    let mut f = open(&h).expect("open at the top of the range");
    assert_eq!(f.generation(), top);
    assert!(
        observe(&f).expect("read").same(&model),
        "forged file differs"
    );
    // Two commits are still possible (top + 1, top + 2), then the range ends.
    for _ in 0..2 {
        let data = rng.bytes(300);
        f.write_at(10, &data).expect("write below the limit");
        model.write(10, &data);
        // A crash and a reopen at every step of the approach.
        drop(f);
        h.crash(random_policy(&mut rng));
        f = open(&h).expect("reopen");
        assert!(observe(&f).expect("read").same(&model));
    }
    assert_eq!(f.generation(), u64::MAX - 1);
    let ordinal = h.next_ordinal();
    for attempt in 0..3 {
        let refused = [
            f.write_at(0, &[1; 10]).map(|_| ()),
            f.create_snapshot().map(|_| ()),
            f.delete_snapshot(1).map(|_| ()),
        ];
        for r in &refused {
            assert!(
                matches!(r, Err(SnapError::Cow(crate::cow_proto::CowError::RootInconsistent(m))) if *m == "generation exhausted"),
                "attempt {attempt}: {r:?}"
            );
        }
        assert_eq!(h.next_ordinal(), ordinal, "refused before any backend call");
        assert!(!f.is_poisoned());
        assert!(observe(&f).expect("read").same(&model), "state unchanged");
        drop(f);
        h.crash(random_policy(&mut rng));
        f = open(&h).expect("reopen at the last generation");
        assert_eq!(f.generation(), u64::MAX - 1);
        assert!(observe(&f).expect("read").same(&model));
    }
    // The way out: compaction restarts the generation and keeps snapshot ids.
    let th = FaultHandle::new(&[], 0x6E6);
    let (mut next, _) = f
        .compact_to(fst(&th), 8)
        .expect("compact an exhausted file");
    assert!(
        next.generation() < 1 << 16,
        "generation {}",
        next.generation()
    );
    assert!(observe(&next).expect("read").same(&model));
    drop(next);
    th.crash(CrashPolicy::DropPending);
    next = open(&th).expect("open the compacted file");
    let (id, _) = next
        .create_snapshot()
        .expect("the new file accepts commits");
    assert_eq!(id, model.next_id, "ids continue across the compaction");
    next.write_at(0, &[9; 20]).expect("write");
}
