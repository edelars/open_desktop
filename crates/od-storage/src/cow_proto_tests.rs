//! Fault-model tests for the CoW commit prototype in [`crate::cow_proto`].
//!
//! The [`Oracle`] below is a plain model of "which block snapshots may be on
//! media": the last acknowledged snapshot, plus the target snapshot of a
//! transaction that started but was not acknowledged. It sees only whole
//! snapshots (`Vec<Vec<u8>>`) and never the file format, the roots, the map
//! or `cow_proto` internals; the harness adapts `open` results into
//! [`Observed`].
//!
//! Contract checked after every crash and reopen:
//!
//! * a transaction that returned a `DurableAck` is present exactly;
//! * a transaction that returned an error is either entirely present or
//!   entirely absent (never a mix of old and new blocks);
//! * before the first acknowledged `create` the file may not exist (open
//!   refuses) or be empty;
//! * a refusal to open is a violation whenever a file was acknowledged.
//!
//! A power cut between two operations is modelled as a failing operation at
//! the next ordinal: the process is gone, so no later operation runs and the
//! pending set is identical.

use crate::cow_proto::{
    BLOCK_SIZE, CommitOrder, CowError, CowFile, CowView, HEADER_LEN, MAX_BLOCKS, OpenReport,
    SLOT_SIZE, SlotState, crc32, forge_root_sector, slot_offset,
};
use crate::io::CheckedStorage;
use crate::testing::fault::{
    CrashPolicy, Fault, FaultBackend, FaultHandle, FlushFault, WriteFault,
};
use crate::testing::rng::SplitMix64;
use std::collections::BTreeMap;
use std::io::ErrorKind;

pub(crate) type Block = Vec<u8>;
pub(crate) type Snap = Vec<Block>;

// ----------------------------------------------------------------- oracle

#[derive(Debug)]
enum Observed {
    NoFile,
    State(Snap),
    Refused(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Outcome {
    /// The state before the in-flight transaction (or no transaction).
    Before,
    /// The in-flight transaction's target state.
    After,
}

pub(crate) struct Oracle {
    /// Snapshots a crash may legally leave behind. `None` is "no file".
    allowed: Vec<Option<Snap>>,
    target: Option<Snap>,
}

impl Oracle {
    fn new() -> Self {
        Self {
            allowed: vec![None],
            target: None,
        }
    }

    pub(crate) fn base(&self) -> Snap {
        assert_eq!(self.allowed.len(), 1, "base() with a transaction in flight");
        self.allowed[0].clone().unwrap_or_default()
    }

    pub(crate) fn target_after(&self, writes: &[(u64, Block)]) -> Snap {
        let mut snap = self.base();
        for (index, data) in writes {
            let i = *index as usize;
            if i < snap.len() {
                snap[i] = data.clone();
            } else {
                assert_eq!(i, snap.len(), "test bug: hole in oracle writes");
                snap.push(data.clone());
            }
        }
        snap
    }

    /// A transaction was submitted; its target may now appear after a crash.
    fn begin(&mut self, target: Snap) {
        self.allowed.push(Some(target.clone()));
        self.target = Some(target);
    }

    /// The transaction returned a durable ack.
    fn ack(&mut self) {
        self.allowed = vec![self.target.take()];
    }

    fn check(&self, observed: &Observed) -> Result<Outcome, String> {
        let state = match observed {
            Observed::Refused(why) => {
                return Err(format!(
                    "open refused although a file was acknowledged: {why}"
                ));
            }
            Observed::NoFile => None,
            Observed::State(snap) => Some(snap.clone()),
        };
        if !self.allowed.contains(&state) {
            let desc = |s: &Option<Snap>| match s {
                None => "no file".to_string(),
                Some(s) => format!("{} blocks, digest {:#010x}", s.len(), digest(s)),
            };
            return Err(format!(
                "observed state ({}) is not in the allowed set [{}]",
                desc(&state),
                self.allowed.iter().map(desc).collect::<Vec<_>>().join("; ")
            ));
        }
        let is_after = self.allowed.len() > 1 && self.allowed.last() == Some(&state);
        Ok(if is_after {
            Outcome::After
        } else {
            Outcome::Before
        })
    }

    /// Validates the post-crash state and adopts it as the new truth.
    fn observe(&mut self, observed: &Observed) -> Result<Outcome, String> {
        let outcome = self.check(observed)?;
        let state = match observed {
            Observed::State(s) => Some(s.clone()),
            _ => None,
        };
        self.allowed = vec![state];
        self.target = None;
        Ok(outcome)
    }
}

/// Test-only fingerprint for messages (not the code under test's CRC use).
fn digest(snap: &Snap) -> u32 {
    snap.iter().flatten().fold(0x811c_9dc5u32, |h, &b| {
        (h ^ u32::from(b)).wrapping_mul(16_777_619)
    })
}

// ---------------------------------------------------------------- harness

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OpKind {
    SetLen,
    Write,
    Flush,
}

pub(crate) fn block(rng: &mut SplitMix64) -> Block {
    rng.bytes(BLOCK_SIZE)
}

pub(crate) struct Sim {
    pub(crate) handle: FaultHandle,
    pub(crate) file: Option<CowFile<FaultBackend>>,
    pub(crate) oracle: Oracle,
    order: CommitOrder,
    /// Report of the most recent successful `crash_reopen`.
    last_report: Option<OpenReport>,
}

impl Sim {
    pub(crate) fn new(seed: u64, order: CommitOrder) -> Self {
        Self {
            handle: FaultHandle::new(&[], seed),
            file: None,
            oracle: Oracle::new(),
            order,
            last_report: None,
        }
    }

    pub(crate) fn storage(&self) -> CheckedStorage<FaultBackend> {
        CheckedStorage::new(self.handle.open()).expect("capabilities are consistent")
    }

    fn fail(&self, msg: &str) -> ! {
        panic!("{}", self.handle.report(msg))
    }

    fn script(&self, fault: Option<(u64, Fault)>) {
        if let Some((offset, f)) = fault {
            self.handle.script(self.handle.next_ordinal() + offset, f);
        }
    }

    /// Returns whether the create was acknowledged.
    pub(crate) fn create(&mut self, fault: Option<(u64, Fault)>) -> bool {
        self.script(fault);
        self.oracle.begin(Vec::new());
        self.handle.log("harness: create");
        match CowFile::create(self.storage()) {
            Ok((file, _ack)) => {
                self.file = Some(file);
                self.oracle.ack();
                true
            }
            Err(e) => {
                self.handle.log(format!("harness: create failed {e:?}"));
                false
            }
        }
    }

    /// Returns whether the commit was acknowledged.
    pub(crate) fn commit(&mut self, writes: &[(u64, Block)], fault: Option<(u64, Fault)>) -> bool {
        let target = self.oracle.target_after(writes);
        self.script(fault);
        self.oracle.begin(target);
        self.handle.log(format!(
            "harness: commit {:?} order={:?} fault={fault:?}",
            writes.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            self.order
        ));
        let refs: Vec<(u64, &[u8])> = writes.iter().map(|(i, d)| (*i, d.as_slice())).collect();
        let order = self.order;
        let Some(file) = self.file.as_mut() else {
            self.fail("test bug: commit without an open file");
        };
        match file.commit_with_order(&refs, order) {
            Ok(_ack) => {
                self.oracle.ack();
                self.handle.log("harness: ack");
                true
            }
            Err(e) => {
                self.handle.log(format!("harness: commit failed {e:?}"));
                false
            }
        }
    }

    /// Live (no crash) check that the writer view equals the oracle.
    fn verify_live(&self) {
        let Some(file) = self.file.as_ref() else {
            self.fail("test bug: verify_live without a file");
        };
        match file.snapshot() {
            Ok(snap) if snap == self.oracle.base() => {}
            other => self.fail(&format!(
                "live snapshot differs from oracle: {:?}",
                other.map(|s| s.len())
            )),
        }
    }

    /// Crash, reopen (writable), check against the oracle.
    fn crash_reopen(&mut self, policy: CrashPolicy) -> Result<Outcome, String> {
        self.file = None;
        self.handle.crash(policy);
        let (obs, file, report) = observe_open(&self.handle);
        self.handle.log(format!(
            "harness: reopened -> {} report={report:?}",
            match &obs {
                Observed::NoFile => "no file".to_string(),
                Observed::State(s) => format!("{} blocks", s.len()),
                Observed::Refused(w) => format!("refused {w}"),
            }
        ));
        let outcome = self.oracle.observe(&obs)?;
        self.file = file;
        self.last_report = report;
        Ok(outcome)
    }

    fn must_crash_reopen(&mut self, policy: CrashPolicy) -> Outcome {
        match self.crash_reopen(policy) {
            Ok(o) => o,
            Err(m) => self.fail(&format!("oracle violation after {policy:?}: {m}")),
        }
    }
}

type Opened = (Observed, Option<CowFile<FaultBackend>>, Option<OpenReport>);

/// Opens the handle's current media (writable) and adapts the result for the
/// oracle.
fn observe_open(handle: &FaultHandle) -> Opened {
    let storage = CheckedStorage::new(handle.open()).expect("capabilities are consistent");
    match CowFile::open(storage) {
        Ok((file, report)) => match file.snapshot() {
            Ok(snap) => (Observed::State(snap), Some(file), Some(report)),
            Err(e) => (Observed::Refused(format!("{e:?}")), None, None),
        },
        Err(CowError::NoValidRoot { .. } | CowError::IncompleteCreate { .. }) => {
            (Observed::NoFile, None, None)
        }
        Err(e) => (Observed::Refused(format!("{e:?}")), None, None),
    }
}

pub(crate) fn parse_kind(line: &str) -> Option<(u64, OpKind)> {
    let rest = line.strip_prefix('#')?;
    let (ordinal, op) = rest.split_once(' ')?;
    let kind = if op.starts_with("write") {
        OpKind::Write
    } else if op.starts_with("set_len") {
        OpKind::SetLen
    } else if op.starts_with("flush") {
        OpKind::Flush
    } else {
        return None;
    };
    Some((ordinal.parse().ok()?, kind))
}

/// Kinds of the mutating backend calls one clean transaction of `n` appended
/// blocks makes, read from the fault backend's own trace.
pub(crate) fn tx_ops(order: CommitOrder, n: u64) -> Vec<OpKind> {
    let mut sim = Sim::new(0x0907, order);
    assert!(sim.create(None));
    let mut rng = SplitMix64(1);
    let start = sim.handle.next_ordinal();
    let writes: Vec<(u64, Block)> = (0..n).map(|i| (i, block(&mut rng))).collect();
    assert!(sim.commit(&writes, None));
    let end = sim.handle.next_ordinal();
    let ops: Vec<OpKind> = sim
        .handle
        .trace()
        .iter()
        .filter_map(|l| parse_kind(l))
        .filter(|(o, _)| *o >= start && *o < end)
        .map(|(_, k)| k)
        .collect();
    assert_eq!(ops.len() as u64, end - start);
    ops
}

pub(crate) fn faults_for(kind: OpKind) -> Vec<Fault> {
    match kind {
        OpKind::SetLen => vec![
            Fault::Resize(ErrorKind::Other),
            Fault::Resize(ErrorKind::StorageFull),
        ],
        OpKind::Write => vec![
            Fault::Write(WriteFault::Fail(ErrorKind::Other)),
            Fault::Write(WriteFault::Fail(ErrorKind::StorageFull)),
            Fault::Write(WriteFault::Short(0)),
            Fault::Write(WriteFault::Short(1)),
            Fault::Write(WriteFault::Short(100)),
            Fault::Write(WriteFault::TornIo { written: 0 }),
            Fault::Write(WriteFault::TornIo { written: 1 }),
            Fault::Write(WriteFault::TornIo { written: 100 }),
            Fault::Write(WriteFault::TornIo { written: 508 }),
        ],
        OpKind::Flush => vec![
            Fault::Flush(FlushFault::IoNotDurable),
            Fault::Flush(FlushFault::IoAfterDurable),
        ],
    }
}

/// A fault that makes the operation (and so the commit) fail. `Short(n>0)` is
/// a normal short transfer that `CheckedStorage` completes.
pub(crate) fn is_failing(f: Fault) -> bool {
    !matches!(f, Fault::Write(WriteFault::Short(n)) if n > 0)
}

pub(crate) fn policies() -> Vec<CrashPolicy> {
    let mut v = vec![CrashPolicy::DropPending, CrashPolicy::KeepPending];
    v.extend((0..8).map(|s| CrashPolicy::Seeded(0xC0DE + s)));
    v
}

pub(crate) fn random_policy(rng: &mut SplitMix64) -> CrashPolicy {
    match rng.below(4) {
        0 => CrashPolicy::DropPending,
        1 => CrashPolicy::KeepPending,
        _ => CrashPolicy::Seeded(rng.next_u64()),
    }
}

// ------------------------------------------------------------------ tests

#[test]
fn crc32_matches_ieee_check_value() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(b""), 0);
}

/// (a) ack -> crash -> open gives exactly the acknowledged snapshot.
#[test]
fn ack_then_crash_open_returns_acked_snapshot() {
    for policy in policies() {
        let mut sim = Sim::new(0xA, CommitOrder::Correct);
        let mut rng = SplitMix64(0xA);
        assert!(sim.create(None));
        let w1: Vec<_> = (0..3).map(|i| (i, block(&mut rng))).collect();
        assert!(sim.commit(&w1, None));
        let w2 = vec![(1, block(&mut rng)), (3, block(&mut rng))];
        assert!(sim.commit(&w2, None));
        sim.verify_live();
        let expected = sim.oracle.base();
        assert_eq!(expected.len(), 4);
        assert_eq!(sim.handle.pending_len(), 0, "ack means nothing is pending");
        // Drop = process death.
        let outcome = sim.must_crash_reopen(policy);
        assert_eq!(outcome, Outcome::Before);
        sim.verify_live();
        assert_eq!(sim.oracle.base(), expected);

        // A clean file opens writable and reports neither damage nor repair.
        let storage = sim.storage();
        sim.file = None;
        let (file, report) = CowFile::open(storage).expect("writable open");
        assert_eq!(file.generation(), 4); // create = 2, two commits
        assert!(
            !report.possible_lost_commit && !report.rolled_forward,
            "{report:?}"
        );
    }
}

#[derive(Clone)]
struct Shape {
    prior: u64,
    indexes: Vec<u64>,
}

/// (b) A fault at every mutating ordinal of a commit x every fault kind x many
/// crash outcomes.
#[test]
fn every_cut_point_leaves_old_or_new_never_mixed() {
    let shapes = [
        Shape {
            prior: 0,
            indexes: vec![0, 1],
        },
        Shape {
            prior: 3,
            indexes: vec![1, 3],
        },
        Shape {
            prior: 2,
            indexes: vec![0],
        },
    ];
    let mut summary: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    let mut cases = 0u32;
    for shape in &shapes {
        let ops = tx_ops(CommitOrder::Correct, shape.indexes.len() as u64);
        let flushes: Vec<usize> = (0..ops.len())
            .filter(|&i| ops[i] == OpKind::Flush)
            .collect();
        assert_eq!(flushes.len(), 2, "protocol has exactly two barriers");
        let (b1, b2) = (flushes[0], flushes[1]);
        let root = b1 + 1;
        assert_eq!(b2, root + 1, "root write sits between the barriers");
        assert_eq!(ops[root], OpKind::Write);

        for (k, kind) in ops.iter().enumerate() {
            for fault in faults_for(*kind) {
                for policy in policies() {
                    cases += 1;
                    let seed = 0xB000 + cases as u64;
                    let mut sim = Sim::new(seed, CommitOrder::Correct);
                    let mut rng = SplitMix64(seed);
                    assert!(sim.create(None));
                    if shape.prior > 0 {
                        let prior: Vec<_> =
                            (0..shape.prior).map(|i| (i, block(&mut rng))).collect();
                        assert!(sim.commit(&prior, None));
                    }
                    let writes: Vec<_> = shape
                        .indexes
                        .iter()
                        .map(|&i| (i, block(&mut rng)))
                        .collect();
                    let acked = sim.commit(&writes, Some((k as u64, fault)));

                    // No ack unless the commit really went through.
                    if is_failing(fault) {
                        if acked {
                            sim.fail(&format!("ack at cut {k} with failing fault {fault:?}"));
                        }
                    } else if !acked {
                        sim.fail(&format!(
                            "non-failing fault {fault:?} at {k} broke the commit"
                        ));
                    }

                    let outcome = sim.must_crash_reopen(policy);
                    let label = format!("{:?} #{k}/{:?}", kind, fault);
                    let expected: &[Outcome] = if !is_failing(fault) {
                        // Acked: the oracle has no transaction in flight, and
                        // it already required the exact acked snapshot.
                        &[Outcome::Before]
                    } else if k <= b1 {
                        // Root not written yet: the transaction cannot be visible.
                        &[Outcome::Before]
                    } else if k == b2 && fault == Fault::Flush(FlushFault::IoAfterDurable) {
                        &[Outcome::After]
                    } else {
                        &[Outcome::Before, Outcome::After]
                    };
                    if !expected.contains(&outcome) {
                        sim.fail(&format!(
                            "cut {label} policy {policy:?}: outcome {outcome:?} not in {expected:?}"
                        ));
                    }
                    let report = sim.last_report.expect("open succeeded");
                    // Writable open never carries unexplained damage.
                    if report.possible_lost_commit {
                        sim.fail(&format!("cut {label}: possible_lost_commit"));
                    }
                    if is_failing(fault) && k <= b1 && report.rolled_forward {
                        sim.fail(&format!(
                            "cut {label}: crash before barrier 1 rolled forward"
                        ));
                    }
                    let torn_root =
                        k == root && matches!(fault, Fault::Write(WriteFault::TornIo { .. }));
                    let torn_deep = matches!(
                        fault,
                        Fault::Write(WriteFault::TornIo { written }) if written >= 100
                    );
                    if torn_root {
                        // The root copy was durable before the torn root write:
                        // a kept torn root is repaired by roll-forward, a lost
                        // one leaves the old valid slot. Never anything else.
                        let rolled = outcome == Outcome::After;
                        if rolled != report.rolled_forward {
                            sim.fail(&format!("cut {label}: outcome {outcome:?} vs {report:?}"));
                        }
                        if torn_deep && policy == CrashPolicy::KeepPending && !rolled {
                            sim.fail(&format!("cut {label}: kept torn root not rolled forward"));
                        }
                    } else if report.rolled_forward {
                        sim.fail(&format!("cut {label}: unexpected roll-forward"));
                    }
                    let entry = summary.entry(label).or_default();
                    match outcome {
                        Outcome::Before => entry.0 += 1,
                        Outcome::After => entry.1 += 1,
                    }

                    // The recovered file is writable again and stays correct.
                    let more = vec![(0, block(&mut rng))];
                    if !sim.commit(&more, None) {
                        sim.fail("commit after recovery failed");
                    }
                    sim.verify_live();
                    sim.must_crash_reopen(CrashPolicy::DropPending);
                }
            }
        }
    }
    // Both outcomes really occur at the points where the model allows both.
    let seen = |needle: &str, before: bool| {
        summary
            .iter()
            .any(|(k, (b, a))| k.contains(needle) && if before { *b > 0 } else { *a > 0 })
    };
    assert!(seen("IoNotDurable", true) && seen("IoNotDurable", false));
    assert!(seen("TornIo", true) && seen("TornIo", false));
    assert!(seen("IoAfterDurable", false));
    eprintln!("cut-point cases: {cases}");
    for (label, (before, after)) in &summary {
        eprintln!("  {label}: before={before} after={after}");
    }
}

/// Faults while creating: either no file or an empty file, never garbage.
#[test]
fn create_cut_points_give_no_file_or_empty_file() {
    let ops = {
        let sim = Sim::new(1, CommitOrder::Correct);
        let mut sim = sim;
        assert!(sim.create(None));
        sim.handle
            .trace()
            .iter()
            .filter_map(|l| parse_kind(l))
            .map(|(_, k)| k)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ops,
        [OpKind::SetLen, OpKind::Write, OpKind::Write, OpKind::Flush]
    );
    for (k, kind) in ops.iter().enumerate() {
        for fault in faults_for(*kind) {
            for policy in policies() {
                let mut sim = Sim::new(0xC1, CommitOrder::Correct);
                let acked = sim.create(Some((k as u64, fault)));
                assert_eq!(acked, !is_failing(fault));
                sim.must_crash_reopen(policy);
            }
        }
    }
}

pub(crate) fn history(seed: u64, txs: u64) -> (Sim, Vec<Snap>) {
    let mut sim = Sim::new(seed, CommitOrder::Correct);
    let mut rng = SplitMix64(seed);
    assert!(sim.create(None));
    let mut snaps = vec![Vec::new()];
    for t in 0..txs {
        let writes: Vec<(u64, Block)> = if t == 0 {
            (0..3).map(|i| (i, block(&mut rng))).collect()
        } else {
            vec![(t % 3, block(&mut rng)), (2 + t, block(&mut rng))]
        };
        assert!(sim.commit(&writes, None));
        snaps.push(sim.oracle.base());
    }
    sim.handle.crash(CrashPolicy::DropPending);
    (sim, snaps)
}

pub(crate) fn with_bytes(sim: &Sim, mutate: impl FnOnce(&mut Vec<u8>)) -> FaultHandle {
    let mut bytes = sim.handle.durable_bytes();
    mutate(&mut bytes);
    FaultHandle::new(&bytes, 0xDA7A)
}

pub(crate) fn writable(
    handle: &FaultHandle,
) -> Result<(CowFile<FaultBackend>, OpenReport), CowError> {
    CowFile::open(CheckedStorage::new(handle.open()).expect("caps"))
}

pub(crate) fn salvage(
    handle: &FaultHandle,
) -> Result<(CowView<FaultBackend>, OpenReport), CowError> {
    CowView::salvage(CheckedStorage::new(handle.open()).expect("caps"))
}

/// (c) Corrupting the active root is caught by the CRC. With the root copy
/// intact the writable open rolls forward and repairs the slot; without it the
/// writable open refuses and only a read-only salvage shows the older state,
/// flagged and without writing a byte.
#[test]
fn corrupted_active_root_rolls_forward_or_refuses_never_silent() {
    let (sim, snaps) = history(0xC0, 2); // generations 1..=4, active = slot 0
    let last = snaps.last().cloned().expect("history");
    let previous = snaps[snaps.len() - 2].clone();
    let active = slot_offset(4) as usize;
    let original = sim.handle.durable_bytes();
    let copy_at = original.len() - SLOT_SIZE;

    for byte in 0..SLOT_SIZE {
        // (1) Root copy intact: roll-forward to the acknowledged state.
        let broken = with_bytes(&sim, |b| b[active + byte] ^= 0xFF);
        let (file, report) = writable(&broken)
            .unwrap_or_else(|e| panic!("byte {byte}: writable open failed: {e:?}"));
        let snap = file.snapshot().expect("verified");
        assert_eq!(snap, last, "byte {byte}");
        assert!(
            report.rolled_forward && !report.possible_lost_commit,
            "{report:?}"
        );
        assert_eq!(report.generation, 4);
        drop(file);
        // The oracle (last ack = `last`) accepts it.
        let mut oracle = Oracle::new();
        oracle.allowed = vec![Some(last.clone())];
        assert!(oracle.check(&Observed::State(snap)).is_ok());
        // The damaged slot was repaired on media (barrier included).
        broken.crash(CrashPolicy::DropPending);
        assert_eq!(
            broken.durable_bytes(),
            original,
            "byte {byte}: not repaired"
        );

        // (2) Root copy destroyed as well: unexplained damage.
        let broken = with_bytes(&sim, |b| {
            b[active + byte] ^= 0xFF;
            b[copy_at + 20] ^= 0x01;
        });
        match writable(&broken) {
            Err(CowError::DamagedOtherSlot {
                chosen_generation: 3,
                other: SlotState::Damaged,
            }) => {}
            other => panic!(
                "byte {byte}: writable must refuse, got {:?}",
                other.map(|(_, r)| r)
            ),
        }
        let (bytes_before, ordinal) = (broken.volatile_bytes(), broken.next_ordinal());
        let (view, report) = salvage(&broken).expect("salvage");
        let snap = view.snapshot().expect("older generation is intact");
        assert_eq!(snap, previous, "byte {byte}");
        assert!(
            report.possible_lost_commit && !report.rolled_forward,
            "{report:?}"
        );
        assert_eq!(report.generation, 3);
        drop(view);
        // Read-only: no mutating backend call, no byte changed, nothing pending.
        assert_eq!(broken.next_ordinal(), ordinal);
        assert_eq!(broken.volatile_bytes(), bytes_before);
        assert_eq!(broken.durable_bytes(), bytes_before);
        assert_eq!(broken.pending_len(), 0);
        // The oracle calls that state a loss; the flag is what makes it visible.
        let mut oracle = Oracle::new();
        oracle.allowed = vec![Some(last.clone())];
        assert!(oracle.check(&Observed::State(snap)).is_err());
    }

    // Zeroing the whole active root: roll-forward; plus the copy: refusal.
    let wiped = with_bytes(&sim, |b| b[active..active + SLOT_SIZE].fill(0));
    let (file, report) = writable(&wiped).expect("roll-forward");
    assert_eq!(file.snapshot().expect("snapshot"), last);
    assert!(report.rolled_forward);
    let wiped = with_bytes(&sim, |b| {
        b[active..active + SLOT_SIZE].fill(0);
        b[copy_at..].fill(0);
    });
    assert!(matches!(
        writable(&wiped),
        Err(CowError::DamagedOtherSlot {
            other: SlotState::Blank,
            ..
        })
    ));

    // Damaging the *older* slot: the last-sector copy is of generation 4 (not
    // 5), so nothing explains it. Writable refuses; salvage shows the newest
    // state, flagged.
    let older = slot_offset(3) as usize;
    let broken = with_bytes(&sim, |b| b[older + 20] ^= 0x01);
    assert!(matches!(
        writable(&broken),
        Err(CowError::DamagedOtherSlot { .. })
    ));
    let (view, report) = salvage(&broken).expect("salvage");
    assert_eq!(view.snapshot().expect("snapshot"), last);
    assert!(report.possible_lost_commit);
}

/// (d) Both slots damaged: refusal, never an empty file. Same for salvage.
#[test]
fn both_slots_damaged_refuses_to_open() {
    let (sim, _) = history(0xD0, 2);
    type Mutation = Box<dyn Fn(&mut Vec<u8>)>;
    let cases: Vec<Mutation> = vec![
        Box::new(|b| {
            b[10] ^= 0xFF;
            b[SLOT_SIZE + 10] ^= 0xFF;
        }),
        Box::new(|b| b[..2 * SLOT_SIZE].fill(0)),
        Box::new(|b| {
            let junk = SplitMix64(7).bytes(2 * SLOT_SIZE);
            b[..2 * SLOT_SIZE].copy_from_slice(&junk);
        }),
        Box::new(|b| b.truncate(HEADER_LEN as usize - 1)),
        Box::new(|b| b.clear()),
    ];
    for (i, mutate) in cases.iter().enumerate() {
        let broken = with_bytes(&sim, |b| mutate(b));
        assert!(
            matches!(writable(&broken), Err(CowError::NoValidRoot { .. })),
            "case {i}: writable"
        );
        assert!(
            matches!(salvage(&broken), Err(CowError::NoValidRoot { .. })),
            "case {i}: salvage"
        );
    }
}

/// A valid newest root whose data is damaged refuses; it does not fall back to
/// the older valid slot. Orphaned (unreferenced) bytes are irrelevant.
#[test]
fn damaged_data_under_valid_root_refuses_without_fallback() {
    let (sim, _) = history(0xE0, 3);
    let (file, _) = writable(&with_bytes(&sim, |_| {})).expect("clean open");
    let referenced = file.block_offset(0).expect("block 0") as usize;
    let map_at = file.map_offset() as usize;
    let count = file.block_count();
    drop(file);

    let refused = |handle: &FaultHandle, pred: fn(&CowError) -> bool| {
        let w = writable(handle).err().expect("writable must refuse");
        let v = salvage(handle).err().expect("salvage must refuse");
        assert!(pred(&w) && pred(&v), "{w:?} / {v:?}");
    };
    refused(&with_bytes(&sim, |b| b[referenced + 17] ^= 0x40), |e| {
        matches!(e, CowError::BlockCorrupt { .. })
    });
    refused(&with_bytes(&sim, |b| b[map_at + 3] ^= 0x01), |e| {
        matches!(e, CowError::MapCorrupt)
    });
    // File cut short after an ack is outside the model: refused, not zero-filled.
    refused(
        &with_bytes(&sim, |b| b.truncate(b.len() - BLOCK_SIZE)),
        |e| matches!(e, CowError::RootInconsistent(_)),
    );
    // Garbage past the published end that no root references does not matter.
    let grown = with_bytes(&sim, |b| {
        b.extend(std::iter::repeat_n(0xEE, 3 * BLOCK_SIZE))
    });
    let (file, report) = writable(&grown).expect("orphan tail");
    assert_eq!(file.block_count(), count);
    assert!(!report.rolled_forward && !report.possible_lost_commit);
}

// ------------------------------------------------- (b) parser, valid CRC

/// A file of `len` zero bytes with `sector` installed in the slot of its
/// generation. The sector's CRC is valid; only its fields are hostile.
pub(crate) fn forged(len: usize, sectors: &[[u8; SLOT_SIZE]]) -> Vec<u8> {
    let mut bytes = vec![0u8; len];
    for sector in sectors {
        let generation = u64::from_le_bytes(sector[16..24].try_into().expect("8 bytes"));
        let at = (generation & 1) as usize * SLOT_SIZE;
        bytes[at..at + SLOT_SIZE].copy_from_slice(sector);
    }
    bytes
}

fn entry_bytes(phys: u64) -> Vec<u8> {
    let mut e = phys.to_le_bytes().to_vec();
    e.extend_from_slice(&0u32.to_le_bytes());
    e
}

fn expect_refused(bytes: &[u8], pred: impl Fn(&CowError) -> bool, label: &str) {
    let handle = FaultHandle::new(bytes, 0xF0F0);
    let w = writable(&handle)
        .err()
        .unwrap_or_else(|| panic!("{label}: writable opened"));
    let v = salvage(&handle)
        .err()
        .unwrap_or_else(|| panic!("{label}: salvage opened"));
    assert!(pred(&w) && pred(&v), "{label}: {w:?} / {v:?}");
    assert_eq!(
        handle.next_ordinal(),
        0,
        "{label}: refused open must not write"
    );
}

#[test]
fn crc_valid_but_hostile_roots_are_refused() {
    let end = HEADER_LEN + SLOT_SIZE as u64; // 1536: header + one body sector
    let len = end as usize;
    let inconsistent = |e: &CowError| matches!(e, CowError::RootInconsistent(_));

    // block_count above the limit.
    let root = forge_root_sector(1, MAX_BLOCKS + 1, HEADER_LEN, 0, end);
    expect_refused(&forged(len, &[root]), inconsistent, "block_count > MAX");
    let root = forge_root_sector(1, u64::MAX, HEADER_LEN, 0, end);
    expect_refused(
        &forged(len, &[root]),
        inconsistent,
        "block_count = u64::MAX",
    );

    // map_offset + map_len overflows u64.
    let root = forge_root_sector(1, 1, u64::MAX - 5, 0, end);
    expect_refused(&forged(len, &[root]), inconsistent, "map_offset overflow");
    // map before the header / past the published end.
    let root = forge_root_sector(1, 1, 0, 0, end);
    expect_refused(&forged(len, &[root]), inconsistent, "map inside header");
    let root = forge_root_sector(1, 1, end, 0, end);
    expect_refused(&forged(len, &[root]), inconsistent, "map past end");

    // end_offset beyond the file, unaligned, or inside the header.
    for (bad_end, label) in [
        (end + 512, "end beyond EOF"),
        (end - 1, "end unaligned"),
        (512, "end in header"),
    ] {
        let root = forge_root_sector(1, 0, HEADER_LEN, crate::cow_proto::crc32(&[]), bad_end);
        expect_refused(&forged(len, &[root]), inconsistent, label);
    }

    // Map entries with a valid map CRC but hostile physical offsets. The
    // published area is [header][map sector][root copy sector]: the strict
    // parser (S9-09) reserves the last sector for the root copy, so blocks and
    // the map must end before it.
    let end = HEADER_LEN + 2 * SLOT_SIZE as u64;
    let len = end as usize;
    for (phys, label) in [
        (HEADER_LEN + 1, "phys unaligned"),
        (1u64 << 40, "phys far beyond EOF"),
        (end, "phys block starts at end"),
        (end - 256, "phys block straddles end"),
        (SLOT_SIZE as u64, "phys inside header"),
        (u64::MAX - 100, "phys + block overflows"),
    ] {
        let map = entry_bytes(phys);
        let root = forge_root_sector(1, 1, HEADER_LEN, crate::cow_proto::crc32(&map), end);
        let mut bytes = forged(len, &[root]);
        bytes[HEADER_LEN as usize..HEADER_LEN as usize + map.len()].copy_from_slice(&map);
        expect_refused(
            &bytes,
            |e| matches!(e, CowError::MapEntryOutOfRange { index: 0 }),
            label,
        );
    }

    // generation = u64::MAX (odd, so it sits in slot 1 and the CRC is valid).
    let root = forge_root_sector(
        u64::MAX,
        0,
        HEADER_LEN,
        crate::cow_proto::crc32(&[]),
        HEADER_LEN,
    );
    expect_refused(
        &forged(HEADER_LEN as usize, &[root]),
        inconsistent,
        "generation = MAX",
    );

    // Valid slots that are not consecutive generations (misdirected write).
    let crc0 = crate::cow_proto::crc32(&[]);
    let a = forge_root_sector(5, 0, HEADER_LEN, crc0, HEADER_LEN);
    let b = forge_root_sector(2, 0, HEADER_LEN, crc0, HEADER_LEN);
    expect_refused(
        &forged(HEADER_LEN as usize, &[a, b]),
        inconsistent,
        "generations 5 and 2",
    );
}

#[test]
fn generation_exhaustion_is_rejected_before_any_io() {
    let crc0 = crate::cow_proto::crc32(&[]);
    let top = forge_root_sector(u64::MAX - 1, 0, HEADER_LEN, crc0, HEADER_LEN);
    let prev = forge_root_sector(u64::MAX - 2, 0, HEADER_LEN, crc0, HEADER_LEN);
    let handle = FaultHandle::new(&forged(HEADER_LEN as usize, &[top, prev]), 1);
    let (mut file, report) = writable(&handle).expect("open at MAX - 1");
    assert_eq!(report.generation, u64::MAX - 1);
    let data = vec![7u8; BLOCK_SIZE];
    let ordinal = handle.next_ordinal();
    assert!(matches!(
        file.commit(&[(0, data.as_slice())]),
        Err(CowError::RootInconsistent("generation exhausted"))
    ));
    assert_eq!(
        handle.next_ordinal(),
        ordinal,
        "no I/O before the rejection"
    );
    // Rejected before I/O: the session is not poisoned.
    assert!(!matches!(
        file.commit(&[(0, data.as_slice())]),
        Err(CowError::Poisoned)
    ));
}

#[test]
fn rejected_input_does_not_poison_and_failure_does() {
    let mut sim = Sim::new(0xF0, CommitOrder::Correct);
    let mut rng = SplitMix64(5);
    assert!(sim.create(None));
    let good = block(&mut rng);
    let file = sim.file.as_mut().expect("file");
    assert!(matches!(
        file.commit(&[(0, &good[..10])]),
        Err(CowError::BadWrite(_))
    ));
    assert!(matches!(
        file.commit(&[(1, good.as_slice())]),
        Err(CowError::BadWrite(_)) // hole: block 0 does not exist
    ));
    assert!(matches!(
        file.commit(&[(0, good.as_slice()), (0, good.as_slice())]),
        Err(CowError::BadWrite(_))
    ));
    file.commit(&[(0, good.as_slice())]).expect("still usable");

    sim.handle.script_next(Fault::Resize(ErrorKind::Other));
    let file = sim.file.as_mut().expect("file");
    assert!(file.commit(&[(0, good.as_slice())]).is_err());
    assert!(matches!(
        file.commit(&[(0, good.as_slice())]),
        Err(CowError::Poisoned)
    ));
}

/// (e) 100 seeded sequences: random transactions, random faults, random crash
/// outcomes, every step checked against the oracle.
#[test]
fn seeded_sequences_match_the_oracle() {
    let mut ops_cache: BTreeMap<u64, Vec<OpKind>> = BTreeMap::new();
    let (mut faulted, mut acked_total, mut crashes) = (0u32, 0u32, 0u32);
    for seed in 0..100u64 {
        let mut rng = SplitMix64(0x5EED_0000 + seed);
        let mut sim = Sim::new(0x5EED_0000 + seed, CommitOrder::Correct);
        assert!(sim.create(None));
        let steps = 3 + rng.below(8);
        for step in 0..steps {
            let count = sim.oracle.base().len() as u64;
            let n = 1 + rng.below(3);
            let mut writes: Vec<(u64, Block)> = Vec::new();
            let mut next_new = count;
            let mut used = Vec::new();
            for _ in 0..n {
                if count > 0 && rng.below(2) == 0 {
                    let idx = rng.below(count);
                    if used.contains(&idx) {
                        continue;
                    }
                    used.push(idx);
                    writes.push((idx, block(&mut rng)));
                } else {
                    writes.push((next_new, block(&mut rng)));
                    next_new += 1;
                }
            }
            let n = writes.len() as u64;
            let ops = ops_cache
                .entry(n)
                .or_insert_with(|| tx_ops(CommitOrder::Correct, n))
                .clone();
            let fault = if rng.below(2) == 0 {
                let k = rng.below(ops.len() as u64);
                let choices = faults_for(ops[k as usize]);
                let f = choices[rng.below(choices.len() as u64) as usize];
                Some((k, f))
            } else {
                None
            };
            faulted += u32::from(fault.is_some());
            let acked = sim.commit(&writes, fault);
            sim.handle
                .log(format!("harness: seed={seed} step={step} acked={acked}"));
            if fault.is_some_and(|(_, f)| is_failing(f)) && acked {
                sim.fail("ack for a failing fault");
            }
            if acked {
                acked_total += 1;
                sim.verify_live();
            }
            if !acked || rng.below(3) == 0 {
                crashes += 1;
                let policy = random_policy(&mut rng);
                let outcome = sim.must_crash_reopen(policy);
                if acked && outcome != Outcome::Before {
                    sim.fail("acked state changed after crash");
                }
                sim.verify_live();
            }
        }
        sim.must_crash_reopen(CrashPolicy::DropPending);
    }
    eprintln!("seeded: faulted={faulted} acked={acked_total} crashes={crashes}");
    assert!(faulted > 100 && acked_total > 100 && crashes > 100);
}

// --------------------------------------------- (f) negative: broken orders

/// Runs one transaction with a broken order and a crash; returns the oracle's
/// verdict.
fn broken_run(
    order: CommitOrder,
    fault: Option<FlushFault>,
    fault_at_flush: usize,
    policy: CrashPolicy,
    seed: u64,
) -> Result<Outcome, String> {
    let mut sim = Sim::new(seed, order);
    let mut rng = SplitMix64(seed);
    assert!(sim.create(None));
    let base: Vec<_> = (0..2).map(|i| (i, block(&mut rng))).collect();
    assert!(sim.commit(&base, None));
    let writes = vec![(0, block(&mut rng)), (2, block(&mut rng))];
    let ops = tx_ops(order, 2);
    let flush_ordinals: Vec<u64> = (0..ops.len())
        .filter(|&i| ops[i] == OpKind::Flush)
        .map(|i| i as u64)
        .collect();
    let fault = fault.map(|f| (flush_ordinals[fault_at_flush], Fault::Flush(f)));
    sim.commit(&writes, fault);
    sim.crash_reopen(policy)
}

#[test]
fn oracle_detects_ack_before_barrier_2() {
    // Control: the same scenario with the correct order passes.
    assert_eq!(
        broken_run(CommitOrder::Correct, None, 0, CrashPolicy::DropPending, 1),
        Ok(Outcome::Before)
    );
    // Broken: the ack was given while the root is still volatile; losing the
    // pending root loses an acknowledged commit.
    let err = broken_run(
        CommitOrder::AckBeforeBarrier2,
        None,
        0,
        CrashPolicy::DropPending,
        1,
    )
    .expect_err("ack before barrier 2 must be detected");
    assert!(err.contains("not in the allowed set"), "{err}");
    // It only survives crashes that happen to keep the root.
    assert!(
        broken_run(
            CommitOrder::AckBeforeBarrier2,
            None,
            0,
            CrashPolicy::KeepPending,
            1
        )
        .is_ok()
    );
    let detected = (0..64)
        .filter(|s| {
            broken_run(
                CommitOrder::AckBeforeBarrier2,
                None,
                0,
                CrashPolicy::Seeded(*s),
                2,
            )
            .is_err()
        })
        .count();
    assert!(detected > 0, "no seeded crash exposed the early ack");
    eprintln!("ack-before-barrier-2 detected in {detected}/64 seeded crashes (+ DropPending)");
}

#[test]
fn oracle_detects_root_without_data_barrier() {
    // The single remaining barrier fails before it is durable; the crash then
    // decides data, map and root independently.
    let mut detected = Vec::new();
    for s in 0..256u64 {
        let broken = broken_run(
            CommitOrder::NoDataBarrier,
            Some(FlushFault::IoNotDurable),
            0,
            CrashPolicy::Seeded(s),
            3,
        );
        if let Err(msg) = broken {
            detected.push(msg);
        }
        // Control: the correct protocol under the same cut never violates.
        let control = broken_run(
            CommitOrder::Correct,
            Some(FlushFault::IoNotDurable),
            1,
            CrashPolicy::Seeded(s),
            3,
        );
        assert!(control.is_ok(), "control seed {s}: {control:?}");
    }
    assert!(
        !detected.is_empty(),
        "root-before-data-barrier went undetected"
    );
    assert!(
        detected.iter().all(|m| m.contains("refused")),
        "unexpected detection: {:?}",
        detected.first()
    );
    eprintln!(
        "root-without-data-barrier detected in {}/256 seeded crashes, e.g. {}",
        detected.len(),
        detected[0]
    );
}

#[test]
fn oracle_detects_a_backend_that_acks_without_durability() {
    // Outside the failure model: barrier 2 returns success but flushes nothing.
    let mut sim = Sim::new(0xBAD, CommitOrder::Correct);
    let mut rng = SplitMix64(9);
    assert!(sim.create(None));
    let ops = tx_ops(CommitOrder::Correct, 1);
    let b2 = (ops.len() - 1) as u64;
    let writes = vec![(0, block(&mut rng))];
    assert!(sim.commit(
        &writes,
        Some((b2, Fault::Flush(FlushFault::OkWithoutDurability)))
    ));
    let err = sim
        .crash_reopen(CrashPolicy::DropPending)
        .expect_err("lost acked commit");
    assert!(err.contains("not in the allowed set"), "{err}");
}
