//! Tests for the CoW write transaction (S9-11): [`crate::cow_tx`] over the
//! commit protocol of [`crate::cow_proto`].
//!
//! The oracle here is a plain byte vector: the logical disk as an independent
//! `Vec<u8>` (padded to whole blocks) that every test updates with its own
//! `apply` and compares byte by byte. [`ByteOracle`] adds the allowed-outcome
//! list for crashes (last acknowledged image, plus the target of a commit
//! whose result was uncertain). Neither sees the format, roots or maps.

use crate::cow_proto::{
    BLOCK_SIZE, CommitOrder, CommitStep, CowError, CowFile, CowView, HEADER_LEN, MAX_TX_BLOCKS,
    slot_offset,
};
use crate::cow_proto_tests::{OpKind, faults_for, is_failing, parse_kind, policies};
use crate::cow_tx::{CowTx, TxError, TxState};
use crate::io::{BackendError, CheckedStorage, StorageBackend, StorageCapabilities};
use crate::testing::fault::{CrashPolicy, Fault, FaultBackend, FaultHandle, FlushFault};
use crate::testing::rng::SplitMix64;
use std::cell::Cell;
use std::rc::Rc;

// ------------------------------------------------------- independent model

/// Writes `data` at `off` into the byte model: grows with zeros, then pads to
/// a whole number of blocks (a disk is a whole number of blocks).
pub(crate) fn apply(model: &mut Vec<u8>, off: usize, data: &[u8]) {
    let end = off + data.len();
    if model.len() < end {
        model.resize(end, 0);
    }
    model[off..end].copy_from_slice(data);
    let padded = model.len().div_ceil(BLOCK_SIZE) * BLOCK_SIZE;
    model.resize(padded, 0);
}

pub(crate) fn flat(snap: &[Vec<u8>]) -> Vec<u8> {
    snap.iter().flatten().copied().collect()
}

/// Compares the file with the model byte by byte through `read_block`.
pub(crate) fn assert_bytes<B: StorageBackend>(view: &CowFile<B>, model: &[u8], label: &str) {
    assert_eq!(
        view.block_count() as usize * BLOCK_SIZE,
        model.len(),
        "{label}: size"
    );
    for b in 0..view.block_count() {
        let blk = view.read_block(b).expect("verified block");
        for (j, got) in blk.iter().enumerate() {
            let at = b as usize * BLOCK_SIZE + j;
            assert_eq!(*got, model[at], "{label}: byte {at} (block {b}, +{j})");
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Verdict {
    /// Last acknowledged image.
    Before,
    /// Target of a commit whose result was uncertain.
    After,
}

/// Allowed post-crash images as flat bytes.
pub(crate) struct ByteOracle {
    acked: Vec<u8>,
    target: Option<Vec<u8>>,
}

impl ByteOracle {
    pub(crate) fn new() -> Self {
        Self {
            acked: Vec::new(),
            target: None,
        }
    }

    pub(crate) fn base(&self) -> Vec<u8> {
        assert!(self.target.is_none(), "base() with an uncertain commit");
        self.acked.clone()
    }

    pub(crate) fn ack(&mut self, image: Vec<u8>) {
        self.acked = image;
        self.target = None;
    }

    /// A commit ended uncertain: its image may appear after a crash.
    pub(crate) fn begin(&mut self, image: Vec<u8>) {
        self.target = Some(image);
    }

    pub(crate) fn check(&self, observed: &[u8]) -> Result<Verdict, String> {
        if observed == self.acked.as_slice() {
            return Ok(Verdict::Before);
        }
        if self.target.as_deref() == Some(observed) {
            return Ok(Verdict::After);
        }
        Err(format!(
            "observed image ({} bytes) is neither the acknowledged one ({} bytes){}",
            observed.len(),
            self.acked.len(),
            match &self.target {
                Some(t) => format!(" nor the uncertain target ({} bytes)", t.len()),
                None => " (no uncertain commit)".to_string(),
            }
        ))
    }

    /// Validates `observed` and adopts it as the new acknowledged truth.
    pub(crate) fn observe(&mut self, observed: &[u8]) -> Result<Verdict, String> {
        let v = self.check(observed)?;
        self.acked = observed.to_vec();
        self.target = None;
        Ok(v)
    }
}

// ------------------------------------------------------------------ harness

/// A backend that counts every call except `capabilities` (and delegates to a
/// [`FaultBackend`]), so a test can prove that nothing touched the backend.
pub(crate) struct CountingBackend {
    inner: FaultBackend,
    calls: Rc<Cell<u64>>,
}

impl CountingBackend {
    pub(crate) fn new(inner: FaultBackend, calls: Rc<Cell<u64>>) -> Self {
        Self { inner, calls }
    }

    fn bump(&self) {
        self.calls.set(self.calls.get() + 1);
    }
}

impl StorageBackend for CountingBackend {
    fn capabilities(&self) -> StorageCapabilities {
        self.inner.capabilities()
    }
    fn size(&self) -> Result<u64, BackendError> {
        self.bump();
        self.inner.size()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, BackendError> {
        self.bump();
        self.inner.read_at(offset, buf)
    }
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<usize, BackendError> {
        self.bump();
        self.inner.write_at(offset, buf)
    }
    fn set_len(&mut self, len: u64) -> Result<(), BackendError> {
        self.bump();
        self.inner.set_len(len)
    }
    fn flush_file_durable(&mut self) -> Result<(), BackendError> {
        self.bump();
        self.inner.flush_file_durable()
    }
    fn flush_namespace_durable(&mut self) -> Result<(), BackendError> {
        self.bump();
        self.inner.flush_namespace_durable()
    }
}

fn write_all(file: &mut CowFile<impl StorageBackend>, writes: &[(u64, Vec<u8>)]) {
    let mut tx = CowTx::begin(file).expect("begin");
    for (off, data) in writes {
        tx.write_at(*off, data).expect("write_at");
    }
    tx.commit().expect("commit");
}

/// A created file over a [`CountingBackend`] holding `prior` random blocks
/// (one acknowledged commit when `prior > 0`), with its byte model.
fn counting_file(
    seed: u64,
    prior: usize,
) -> (
    FaultHandle,
    Rc<Cell<u64>>,
    CowFile<CountingBackend>,
    Vec<u8>,
) {
    let handle = FaultHandle::new(&[], seed);
    let calls = Rc::new(Cell::new(0));
    let storage =
        CheckedStorage::new(CountingBackend::new(handle.open(), Rc::clone(&calls))).expect("caps");
    let (mut file, _ack) = CowFile::create(storage).expect("create");
    let mut model = Vec::new();
    if prior > 0 {
        let data = SplitMix64(seed).bytes(prior * BLOCK_SIZE);
        apply(&mut model, 0, &data);
        write_all(&mut file, &[(0, data)]);
    }
    (handle, calls, file, model)
}

/// File plus oracle on a plain [`FaultBackend`]; every transaction goes
/// through [`CowTx`].
pub(crate) struct TxSim {
    pub(crate) handle: FaultHandle,
    pub(crate) file: Option<CowFile<FaultBackend>>,
    pub(crate) oracle: ByteOracle,
    pub(crate) last_class: Option<crate::cow_proto::OpenClass>,
}

impl TxSim {
    /// Creates the file (acknowledged, empty).
    pub(crate) fn new(seed: u64) -> Self {
        let handle = FaultHandle::new(&[], seed);
        let storage = CheckedStorage::new(handle.open()).expect("caps");
        let (file, _ack) = CowFile::create(storage).expect("create");
        Self {
            handle,
            file: Some(file),
            oracle: ByteOracle::new(),
            last_class: None,
        }
    }

    pub(crate) fn fail(&self, msg: &str) -> ! {
        panic!("{}", self.handle.report(msg))
    }

    pub(crate) fn file(&mut self) -> &mut CowFile<FaultBackend> {
        match self.file.as_mut() {
            Some(f) => f,
            None => panic!("{}", self.handle.report("test bug: no open file")),
        }
    }

    /// One transaction of byte writes, with an optional fault at the
    /// `offset`-th mutating backend call from now. Keeps the oracle in step:
    /// `Ok` acknowledges, `Uncertain` makes the target a possible outcome,
    /// anything else changes nothing.
    pub(crate) fn tx(
        &mut self,
        writes: &[(u64, Vec<u8>)],
        fault: Option<(u64, Fault)>,
    ) -> Result<(), TxError> {
        let mut target = self.oracle.base();
        for (off, data) in writes {
            apply(&mut target, *off as usize, data);
        }
        if let Some((k, f)) = fault {
            self.handle.script(self.handle.next_ordinal() + k, f);
        }
        self.handle.log(format!(
            "harness: tx {:?} fault={fault:?}",
            writes
                .iter()
                .map(|(o, d)| (*o, d.len()))
                .collect::<Vec<_>>()
        ));
        let handle = self.handle.clone();
        let result = (|| {
            let mut tx = CowTx::begin(self.file())?;
            for (off, data) in writes {
                tx.write_at(*off, data)?;
            }
            tx.commit().map(|_ack| ())
        })();
        match &result {
            Ok(()) => {
                handle.log("harness: ack");
                self.oracle.ack(target);
            }
            Err(TxError::Uncertain(e)) => {
                handle.log(format!("harness: uncertain {e:?}"));
                self.oracle.begin(target);
            }
            Err(e) => handle.log(format!("harness: rejected {e:?}")),
        }
        result
    }

    /// Crash, writable reopen, oracle check. `Err` is an oracle violation or a
    /// refusal.
    pub(crate) fn crash_reopen(&mut self, policy: CrashPolicy) -> Result<Verdict, String> {
        self.file = None;
        self.handle.crash(policy);
        let storage = CheckedStorage::new(self.handle.open()).expect("caps");
        match CowFile::open(storage) {
            Ok((file, report)) => {
                let image = flat(&file.snapshot().map_err(|e| format!("{e:?}"))?);
                self.handle.log(format!(
                    "harness: reopened {} bytes {report:?}",
                    image.len()
                ));
                let v = self.oracle.observe(&image)?;
                self.last_class = Some(report.class());
                self.file = Some(file);
                Ok(v)
            }
            Err(e) => Err(format!(
                "open refused although a file was acknowledged: {e:?}"
            )),
        }
    }

    pub(crate) fn must_crash_reopen(&mut self, policy: CrashPolicy) -> Verdict {
        match self.crash_reopen(policy) {
            Ok(v) => v,
            Err(m) => self.fail(&format!("oracle violation after {policy:?}: {m}")),
        }
    }
}

/// Reader of the media as it is right now (including unflushed bytes): a
/// second, read-only view of the same backing object.
pub(crate) fn peek(handle: &FaultHandle) -> Result<Vec<u8>, CowError> {
    let (view, _) = CowView::salvage(CheckedStorage::new(handle.open()).expect("caps"))?;
    Ok(flat(&view.snapshot()?))
}

pub(crate) fn flush_count(handle: &FaultHandle) -> usize {
    handle
        .trace()
        .iter()
        .filter(|l| parse_kind(l).is_some_and(|(_, k)| k == OpKind::Flush))
        .count()
}

/// Mutating backend calls of one clean transaction with these writes on a file
/// that already holds `prior` blocks.
pub(crate) fn measure_ops(prior: usize, writes: &[(u64, Vec<u8>)]) -> Vec<OpKind> {
    let mut sim = TxSim::new(0x0A5);
    if prior > 0 {
        let data = SplitMix64(1).bytes(prior * BLOCK_SIZE);
        sim.tx(&[(0, data)], None).expect("prior");
    }
    let start = sim.handle.next_ordinal();
    sim.tx(writes, None).expect("measured tx");
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

fn rnd(rng: &mut SplitMix64, n: usize) -> Vec<u8> {
    rng.bytes(n)
}

// ------------------------------------------------------ S9-11 (a) partial

/// Read-modify-write: partial and boundary-straddling ranges, an exact block,
/// an extension with a zero gap and overlapping writes in one transaction, all
/// against an independent byte vector; the replaced physical blocks keep their
/// bytes (copy-on-write).
#[test]
fn partial_block_overwrite_is_copy_on_write_and_matches_the_byte_model() {
    let mut rng = SplitMix64(0xA11);
    let mut sim = TxSim::new(0xA11);
    let mut model = Vec::new();
    let base = rnd(&mut rng, 3 * BLOCK_SIZE);
    apply(&mut model, 0, &base);
    sim.tx(&[(0, base)], None).expect("base");
    assert_bytes(sim.file(), &model, "base");

    let cases: Vec<(&str, Vec<(u64, usize)>)> = vec![
        ("one byte inside a block", vec![(700, 1)]),
        ("straddles blocks 0 and 1", vec![(510, 100)]),
        ("exactly one block", vec![(1024, BLOCK_SIZE)]),
        ("first and last byte of the disk", vec![(0, 1), (1535, 1)]),
        ("extends the last block", vec![(1500, 100)]),
        ("gap of zero blocks", vec![(4000, 10)]),
        (
            "overlapping writes, later wins",
            vec![(100, 300), (250, 300), (0, 5)],
        ),
        ("whole disk", vec![(0, 3000)]),
    ];
    for (label, ranges) in cases {
        let writes: Vec<(u64, Vec<u8>)> = ranges
            .iter()
            .map(|(o, n)| (*o, rnd(&mut rng, *n)))
            .collect();
        // Physical blocks of the published mapping before the commit.
        let before = sim.handle.volatile_bytes();
        let count = sim.file().block_count();
        let was: Vec<u64> = (0..count)
            .map(|b| sim.file().block_offset(b).expect("mapped"))
            .collect();
        let old: Vec<Vec<u8>> = was
            .iter()
            .map(|at| before[*at as usize..*at as usize + BLOCK_SIZE].to_vec())
            .collect();
        for (o, d) in &writes {
            apply(&mut model, *o as usize, d);
        }
        sim.tx(&writes, None).expect(label);
        assert_bytes(sim.file(), &model, label);

        let after = sim.handle.volatile_bytes();
        for (b, at) in was.iter().enumerate() {
            let at = *at as usize;
            assert_eq!(
                &after[at..at + BLOCK_SIZE],
                old[b].as_slice(),
                "{label}: published physical block {b} was modified"
            );
            // Only blocks a range touches move; the others keep their place.
            let (lo, hi) = (b as u64 * 512, (b as u64 + 1) * 512);
            let touched = writes
                .iter()
                .any(|(o, d)| *o < hi && *o + d.len() as u64 > lo);
            let now = sim.file().block_offset(b as u64);
            assert_eq!(now != Some(at as u64), touched, "{label}: block {b}");
        }
        sim.must_crash_reopen(CrashPolicy::DropPending);
        assert_bytes(sim.file(), &model, &format!("{label} after crash"));
    }
}

/// 60 random transactions (1-3 ranges each, partial, straddling, extending)
/// against the byte model, with a crash every few commits.
#[test]
fn random_ranges_match_the_byte_model() {
    let mut rng = SplitMix64(0xA_5EED);
    let mut sim = TxSim::new(0xA_5EED);
    let mut model: Vec<u8> = Vec::new();
    for step in 0..60 {
        let n = 1 + rng.below(3);
        let mut writes = Vec::new();
        for _ in 0..n {
            let len = 1 + rng.below(1200) as usize;
            // Mostly inside the disk, sometimes extending (with a gap).
            let reach = model.len() as u64 + if rng.below(4) == 0 { 700 } else { 0 };
            let off = rng.below(reach.max(1));
            writes.push((off, rnd(&mut rng, len)));
        }
        for (o, d) in &writes {
            apply(&mut model, *o as usize, d);
        }
        sim.tx(&writes, None)
            .unwrap_or_else(|e| sim.fail(&format!("step {step}: {e:?}")));
        assert_bytes(sim.file(), &model, &format!("step {step}"));
        if step % 7 == 6 {
            let v = sim.must_crash_reopen(CrashPolicy::Seeded(step));
            assert_eq!(v, Verdict::Before);
            assert_bytes(sim.file(), &model, &format!("step {step} after crash"));
        }
    }
    assert!(model.len() > 5 * BLOCK_SIZE);
}

// ------------------------------------------------- S9-11 (b) several extents

/// Several non-adjacent ranges in one commit: before the commit every reader
/// (the transaction's view and an independent read-only view of the media)
/// sees the old mapping, after it the whole new one; no block of the old
/// mapping is overwritten.
#[test]
fn several_extents_in_one_commit_appear_together() {
    let mut rng = SplitMix64(0xB22);
    let mut sim = TxSim::new(0xB22);
    let mut model = Vec::new();
    let base = rnd(&mut rng, 8 * BLOCK_SIZE);
    apply(&mut model, 0, &base);
    sim.tx(&[(0, base)], None).expect("base");
    let old = model.clone();
    let old_gen = sim.file().generation();

    let writes: Vec<(u64, Vec<u8>)> = vec![
        (5, rnd(&mut rng, 20)),                             // partial block 0
        (3 * BLOCK_SIZE as u64, rnd(&mut rng, BLOCK_SIZE)), // whole block 3
        (7 * BLOCK_SIZE as u64 + 500, rnd(&mut rng, 12)),   // tail of block 7
        (9 * BLOCK_SIZE as u64 + 40, rnd(&mut rng, 30)),    // block 9, gap block 8
    ];
    let mut target = model.clone();
    for (o, d) in &writes {
        apply(&mut target, *o as usize, d);
    }
    assert_ne!(target, old);

    let handle = sim.handle.clone();
    let file = sim.file();
    let mut tx = CowTx::begin(file).expect("begin");
    for (o, d) in &writes {
        tx.write_at(*o, d).expect("write_at");
    }
    assert_eq!(tx.dirty_blocks(), 5, "blocks 0, 3, 7, 8 (gap), 9");
    // Dirty state is memory only: every reader still sees the old mapping.
    assert_eq!(flat(&tx.view().snapshot().expect("view")), old);
    assert_eq!(peek(&handle).expect("peek"), old);
    tx.commit().expect("commit");
    sim.oracle.ack(target.clone());

    assert_bytes(sim.file(), &target, "after commit");
    assert_eq!(sim.file().generation(), old_gen + 1, "one generation");
    assert_eq!(peek(&handle).expect("peek"), target, "independent reader");
    sim.must_crash_reopen(CrashPolicy::DropPending);
    assert_bytes(sim.file(), &target, "after crash");
}

// ----------------------------------------------- S9-11 flush boundary sweep

struct Shape {
    name: &'static str,
    prior: usize,
    writes: Vec<(u64, usize)>,
}

/// Every mutating ordinal of a transaction x every fault kind x many crash
/// outcomes, through the transaction layer and the byte oracle. Covers the
/// flush boundaries (a failed barrier 1, a failed or lost-ack barrier 2), and
/// the reader view of the media at the failure point.
#[test]
fn every_cut_of_a_transaction_gives_whole_old_or_whole_new() {
    let shapes = [
        Shape {
            name: "first commit, two blocks",
            prior: 0,
            writes: vec![(0, 2 * BLOCK_SIZE)],
        },
        Shape {
            name: "partial ranges over four blocks",
            prior: 4,
            writes: vec![(100, 50), (1030, 700), (2047, 2)],
        },
        Shape {
            name: "single partial block",
            prior: 2,
            writes: vec![(600, 40)],
        },
    ];
    let mut cases = 0u32;
    let mut after_seen = 0u32;
    for shape in &shapes {
        let mk = |rng: &mut SplitMix64| -> Vec<(u64, Vec<u8>)> {
            shape
                .writes
                .iter()
                .map(|(o, n)| (*o, rng.bytes(*n)))
                .collect()
        };
        let ops = measure_ops(shape.prior, &mk(&mut SplitMix64(7)));
        let flushes: Vec<usize> = (0..ops.len())
            .filter(|&i| ops[i] == OpKind::Flush)
            .collect();
        assert_eq!(flushes.len(), 2, "{}: exactly two barriers", shape.name);
        let (b1, b2) = (flushes[0], flushes[1]);
        assert_eq!(
            b2,
            b1 + 2,
            "{}: root write between the barriers",
            shape.name
        );
        let first_commit = shape.prior == 0;

        for (k, kind) in ops.iter().enumerate() {
            for fault in faults_for(*kind) {
                for policy in policies() {
                    cases += 1;
                    let seed = 0xC000 + cases as u64;
                    let mut rng = SplitMix64(seed);
                    let mut sim = TxSim::new(seed);
                    if shape.prior > 0 {
                        let data = rnd(&mut rng, shape.prior * BLOCK_SIZE);
                        sim.tx(&[(0, data)], None).expect("prior");
                    }
                    let old = sim.oracle.base();
                    let writes = mk(&mut rng);
                    let mut new = old.clone();
                    for (o, d) in &writes {
                        apply(&mut new, *o as usize, d);
                    }
                    let result = sim.tx(&writes, Some((k as u64, fault)));
                    let label = format!("{} {:?} #{k}/{fault:?} {policy:?}", shape.name, kind);
                    if is_failing(fault) {
                        if !matches!(result, Err(TxError::Uncertain(_))) {
                            sim.fail(&format!("{label}: failing fault gave {result:?}"));
                        }
                        // Reader view of the media at the failure point.
                        let seen = peek(&sim.handle)
                            .unwrap_or_else(|e| sim.fail(&format!("{label}: reader {e:?}")));
                        let early = k <= b1 && !first_commit;
                        if seen != old && (early || seen != new) {
                            sim.fail(&format!("{label}: reader saw a mixed or early image"));
                        }
                    } else if result.is_err() {
                        sim.fail(&format!(
                            "{label}: non-failing fault broke the commit: {result:?}"
                        ));
                    }

                    let verdict = sim.must_crash_reopen(policy);
                    let allowed: &[Verdict] = if !is_failing(fault) || (k <= b1 && !first_commit) {
                        &[Verdict::Before]
                    } else if k <= b1 {
                        &[Verdict::Before, Verdict::After]
                    } else if k == b2 && fault == Fault::Flush(FlushFault::IoAfterDurable) {
                        &[Verdict::After]
                    } else {
                        &[Verdict::Before, Verdict::After]
                    };
                    if !allowed.contains(&verdict) && !(old == new) {
                        sim.fail(&format!("{label}: {verdict:?} not in {allowed:?}"));
                    }
                    if verdict == Verdict::After {
                        after_seen += 1;
                    }
                    // The recovered file keeps working.
                    let more = vec![(3u64, rnd(&mut rng, 9))];
                    let mut expect = sim.oracle.base();
                    apply(&mut expect, 3, &more[0].1);
                    sim.tx(&more, None)
                        .unwrap_or_else(|e| sim.fail(&format!("{label}: after recovery {e:?}")));
                    assert_bytes(sim.file(), &expect, &label);
                }
            }
        }
    }
    eprintln!("tx sweep: {cases} cases, new image seen after {after_seen}");
    assert!(cases > 1000 && after_seen > 0);
}

// ------------------------------------------------------- S9-11 (c) size cap

#[test]
fn transaction_size_limit_is_checked_before_any_backend_call() {
    const { assert!(MAX_TX_BLOCKS >= 2) };
    let (handle, calls, mut file, model) = counting_file(0xC1, 0);
    assert!(model.is_empty());
    let blk = vec![0x5Au8; BLOCK_SIZE];

    // CowFile::commit: one block over the limit.
    let big: Vec<(u64, &[u8])> = (0..=MAX_TX_BLOCKS as u64)
        .map(|i| (i, blk.as_slice()))
        .collect();
    let (c0, o0) = (calls.get(), handle.next_ordinal());
    assert_eq!(
        file.commit(&big).err(),
        Some(CowError::TxTooLarge {
            blocks: MAX_TX_BLOCKS + 1,
            limit: MAX_TX_BLOCKS
        })
    );
    assert_eq!(
        (calls.get(), handle.next_ordinal()),
        (c0, o0),
        "no backend call"
    );
    assert!(!file.is_poisoned(), "rejected input is not a failure");

    // CowTx: a single range one block over, no read, no write.
    let mut tx = CowTx::begin(&mut file).expect("begin");
    let too_long = vec![1u8; (MAX_TX_BLOCKS + 1) * BLOCK_SIZE];
    assert_eq!(
        tx.write_at(0, &too_long),
        Err(TxError::TooLarge {
            limit: MAX_TX_BLOCKS,
            needed: MAX_TX_BLOCKS + 1
        })
    );
    // A gap that would have to be zero-filled counts too.
    assert!(matches!(
        tx.write_at(((MAX_TX_BLOCKS as u64) + 5) * BLOCK_SIZE as u64, &[1]),
        Err(TxError::TooLarge { .. })
    ));
    assert_eq!(tx.dirty_blocks(), 0, "a rejected write changes nothing");
    assert_eq!(calls.get(), c0, "still no backend call");

    // Fill exactly to the limit over several writes, then one more block.
    for i in 0..MAX_TX_BLOCKS as u64 {
        tx.write_at(i * BLOCK_SIZE as u64, &blk)
            .expect("within limit");
    }
    assert_eq!(tx.dirty_blocks(), MAX_TX_BLOCKS);
    // Re-writing a dirty block needs no new block.
    tx.write_at(10, &[9]).expect("dirty block reused");
    let before = calls.get();
    assert_eq!(
        tx.write_at(MAX_TX_BLOCKS as u64 * BLOCK_SIZE as u64, &[1]),
        Err(TxError::TooLarge {
            limit: MAX_TX_BLOCKS,
            needed: MAX_TX_BLOCKS + 1
        })
    );
    assert_eq!(calls.get(), before, "limit hit: no backend call");
    assert_eq!(tx.dirty_blocks(), MAX_TX_BLOCKS);
    assert_eq!(tx.state(), TxState::Open, "the transaction is still usable");
    // The transaction at exactly the limit commits.
    tx.commit().expect("MAX_TX_BLOCKS blocks commit");
    assert_eq!(file.block_count(), MAX_TX_BLOCKS as u64);
    assert_eq!(file.read_block(0).expect("block")[10], 9);

    // Arithmetic overflow and the disk-size cap are range errors, not panics.
    let mut tx = CowTx::begin(&mut file).expect("begin");
    assert_eq!(tx.write_at(u64::MAX, &[1, 2]), Err(TxError::OutOfRange));
    assert_eq!(tx.write_at(u64::MAX - 3, &[1; 8]), Err(TxError::OutOfRange));
    assert_eq!(
        tx.write_at(crate::cow_proto::MAX_BLOCKS * BLOCK_SIZE as u64, &[1]),
        Err(TxError::OutOfRange)
    );
    assert!(matches!(
        tx.commit(),
        Err(TxError::Rejected(CowError::BadWrite("empty transaction")))
    ));
}

// ----------------------------------------------------- S9-11 (d) cancellation

fn cancel_writes(prior: usize) -> Vec<(u64, Vec<u8>)> {
    let mut rng = SplitMix64(0xCA);
    vec![
        (5, rnd(&mut rng, 20)),
        (prior as u64 * BLOCK_SIZE as u64 + 10, rnd(&mut rng, 30)),
    ]
}

#[test]
fn cancel_before_commit_never_calls_the_backend() {
    for prior in [0usize, 3] {
        let (handle, calls, mut file, model) = counting_file(0xCA1, prior);
        let (c0, o0, bytes0) = (calls.get(), handle.next_ordinal(), handle.volatile_bytes());
        let gen0 = file.generation();
        let mut tx = CowTx::begin(&mut file).expect("begin");
        // Whole-block writes need no read, so the backend stays untouched
        // through the whole lifecycle.
        let blk = vec![7u8; BLOCK_SIZE];
        tx.write_at(0, &blk).expect("write");
        tx.write_at((prior as u64 + 2) * BLOCK_SIZE as u64, &blk)
            .expect("extend");
        assert_eq!(tx.cancel(), Ok(()));
        assert_eq!(tx.state(), TxState::Cancelled);
        assert_eq!(calls.get(), c0, "prior={prior}: backend was called");
        assert_eq!(
            tx.write_at(0, &blk),
            Err(TxError::NotOpen(TxState::Cancelled))
        );
        assert_eq!(
            tx.commit().err(),
            Some(TxError::NotOpen(TxState::Cancelled))
        );
        assert_eq!(tx.cancel(), Ok(()), "cancel is idempotent");
        assert_eq!(calls.get(), c0);
        drop(tx);
        // Partial blocks read, but still write nothing.
        let mut tx = CowTx::begin(&mut file).expect("begin");
        tx.write_at(100, &[1, 2, 3]).expect("rmw");
        tx.cancel().expect("cancel");
        drop(tx);
        assert_eq!(handle.next_ordinal(), o0, "prior={prior}: mutating call");
        assert_eq!(handle.volatile_bytes(), bytes0);
        assert_eq!(file.generation(), gen0);
        assert_bytes(&file, &model, "unchanged");
    }
}

#[test]
fn cancel_inside_commit_before_barrier_1_leaves_the_old_state() {
    for prior in [0usize, 3] {
        let writes = cancel_writes(prior);
        for (cancelled_points, stop) in (0..8usize).enumerate() {
            let (handle, _calls, mut file, model) = counting_file(0xCA2, prior);
            let bytes0 = handle.volatile_bytes();
            let (gen0, end0) = (file.generation(), file.end_offset());
            let flushes0 = flush_count(&handle);
            let mut tx = CowTx::begin(&mut file).expect("begin");
            for (o, d) in &writes {
                tx.write_at(*o, d).expect("write_at");
            }
            let n = tx.dirty_blocks();
            let total = n + 3; // Resize, n x Block, Map, RootCopy
            let mut seen = Vec::new();
            if stop >= total {
                // Never cancelled: the hook is asked exactly once per point,
                // and not after the last one.
                let r = tx.commit_cancellable(&mut |s| {
                    seen.push(s);
                    false
                });
                assert!(r.is_ok(), "prior={prior}: {r:?}");
                let mut want = vec![CommitStep::Resize];
                want.extend((0..n).map(CommitStep::Block));
                want.extend([CommitStep::Map, CommitStep::RootCopy]);
                assert_eq!(seen, want, "cancel points, in order, none after the last");
                // Published: cancel is refused and does not undo anything.
                assert_eq!(tx.cancel(), Err(TxError::AlreadyCommitted));
                assert_eq!(tx.state(), TxState::Committed);
                assert_eq!(
                    tx.commit().err(),
                    Some(TxError::NotOpen(TxState::Committed))
                );
                drop(tx);
                assert_eq!(file.generation(), gen0 + 1);
                let mut target = model.clone();
                for (o, d) in &writes {
                    apply(&mut target, *o as usize, d);
                }
                assert_bytes(&file, &target, "published");
                assert_eq!(cancelled_points, total, "every point was cancelled once");
                break;
            }
            let r = tx.commit_cancellable(&mut |s| {
                seen.push(s);
                seen.len() - 1 == stop
            });
            assert_eq!(
                r.err(),
                Some(TxError::Cancelled),
                "prior={prior} stop={stop}"
            );
            assert_eq!(
                seen.len(),
                stop + 1,
                "the commit stopped at the cancel point"
            );
            assert_eq!(tx.state(), TxState::Cancelled);
            drop(tx);
            // Old state in memory and on media (tail cut back), session
            // usable. The cut is durable (one barrier) as soon as anything
            // was written; a cancel before the first write calls nothing.
            assert_eq!(file.generation(), gen0);
            assert_bytes(&file, &model, "old state");
            assert_eq!(
                handle.volatile_bytes(),
                bytes0,
                "prior={prior} stop={stop}: tail not cut back"
            );
            assert_eq!(handle.volatile_bytes().len() as u64, end0);
            let cut_barrier = usize::from(stop >= 1);
            assert_eq!(
                flush_count(&handle),
                flushes0 + cut_barrier,
                "prior={prior} stop={stop}: barrier 1 never started, only the cut's barrier"
            );
            assert_eq!(handle.pending_len(), 0, "the cut is durable");
            assert!(!file.is_poisoned());
            write_all(&mut file, &[(40, vec![0xEE; 25])]);
            let mut expect = model.clone();
            apply(&mut expect, 40, &[0xEE; 25]);
            assert_bytes(&file, &expect, "session usable after cancel");
        }
    }
}

/// A cancelled commit followed by a crash with any outcome of the pending
/// bytes opens as the old state: no root copy was written, so recovery has
/// nothing to adopt.
#[test]
fn crash_after_a_cancelled_commit_opens_the_old_state() {
    for prior in [0usize, 3] {
        // Resize, one point per dirty block, Map, RootCopy.
        let total = if prior == 0 { 4 } else { 5 };
        for stop in 0..total {
            for policy in policies() {
                let mut sim = TxSim::new(0xCA3 + stop as u64);
                let mut model = Vec::new();
                if prior > 0 {
                    let data = rnd(&mut SplitMix64(3), prior * BLOCK_SIZE);
                    apply(&mut model, 0, &data);
                    sim.tx(&[(0, data)], None).expect("prior");
                }
                let writes = cancel_writes(prior);
                let gen0 = sim.file().generation();
                let mut n = 0usize;
                let mut at_stop = None;
                {
                    let mut tx = CowTx::begin(sim.file()).expect("begin");
                    for (o, d) in &writes {
                        tx.write_at(*o, d).expect("write_at");
                    }
                    let r = tx.commit_cancellable(&mut |step| {
                        n += 1;
                        let hit = n - 1 == stop;
                        if hit {
                            at_stop = Some(step);
                        }
                        hit
                    });
                    assert_eq!(r.err(), Some(TxError::Cancelled));
                }
                if stop == total - 1 {
                    assert_eq!(at_stop, Some(CommitStep::RootCopy), "last point covered");
                }
                let v = sim.must_crash_reopen(policy);
                assert_eq!(v, Verdict::Before);
                assert_eq!(sim.file().generation(), gen0, "prior={prior} {policy:?}");
                assert_eq!(sim.oracle.base(), model);
            }
        }
    }
}

/// The cut after a cancel is itself a step that can fail: then the session is
/// poisoned and the caller gets `Uncertain`, never `Cancelled`. The media is
/// still the old state (no root copy was ever written).
#[test]
fn failed_rollback_of_a_cancelled_commit_is_uncertain() {
    // Mutating calls of the cancelled commit (3 prior blocks, 2 dirty blocks,
    // cancel at Map): 0 resize, 1-2 blocks, 3 cut, 4 barrier of the cut.
    let faults = [
        (3u64, Fault::Resize(std::io::ErrorKind::Other)),
        (4, Fault::Flush(FlushFault::IoNotDurable)),
        (4, Fault::Flush(FlushFault::IoAfterDurable)),
    ];
    for (k, fault) in faults {
        for policy in policies() {
            let (handle, calls, mut file, model) = counting_file(0xCA4, 3);
            let gen0 = file.generation();
            let writes = cancel_writes(3);
            let mut tx = CowTx::begin(&mut file).expect("begin");
            for (o, d) in &writes {
                tx.write_at(*o, d).expect("write_at");
            }
            assert_eq!(tx.dirty_blocks(), 2);
            handle.script(handle.next_ordinal() + k, fault);
            let map_point = 1 + tx.dirty_blocks();
            let mut n = 0usize;
            let err = tx
                .commit_cancellable(&mut |_| {
                    n += 1;
                    n - 1 == map_point
                })
                .expect_err("cancel at Map");
            assert!(
                matches!(err, TxError::Uncertain(CowError::Storage(_))),
                "#{k}/{fault:?}: {err:?}"
            );
            assert_eq!(tx.state(), TxState::Uncertain);
            let c0 = calls.get();
            assert_eq!(tx.cancel(), Err(TxError::ReconcileRequired));
            assert_eq!(tx.commit().err(), Some(TxError::ReconcileRequired));
            drop(tx);
            assert!(file.is_poisoned());
            assert!(matches!(
                CowTx::begin(&mut file),
                Err(TxError::ReconcileRequired)
            ));
            assert_eq!(calls.get(), c0, "no backend call after the failure");
            drop(file);
            handle.crash(policy);
            let storage = CheckedStorage::new(handle.open()).expect("caps");
            let (reopened, _) = CowFile::open(storage).expect("reconcile");
            assert_eq!(reopened.generation(), gen0, "#{k}/{fault:?} {policy:?}");
            assert_eq!(flat(&reopened.snapshot().expect("verified")), model);
        }
    }
}

// --------------------------------------------- S9-11 (e) commit error / retry

/// Any failing step of a commit is uncertain; nothing can be retried on that
/// file (no backend call), and reopening decides old or whole new.
#[test]
fn failed_commit_is_uncertain_and_cannot_be_retried_without_reopen() {
    let writes = |rng: &mut SplitMix64| vec![(5u64, rng.bytes(20)), (1030u64, rng.bytes(700))];
    let ops = measure_ops(3, &writes(&mut SplitMix64(1)));
    let mut cases = 0;
    for (k, kind) in ops.iter().enumerate() {
        for fault in faults_for(*kind).into_iter().filter(|f| is_failing(*f)) {
            cases += 1;
            let (handle, calls, mut file, model) = counting_file(0xE5 + cases, 3);
            let mut rng = SplitMix64(cases);
            let w = writes(&mut rng);
            let mut new = model.clone();
            for (o, d) in &w {
                apply(&mut new, *o as usize, d);
            }
            let label = format!("#{k}/{fault:?}");
            let mut tx = CowTx::begin(&mut file).expect("begin");
            for (o, d) in &w {
                tx.write_at(*o, d).expect("write_at");
            }
            handle.script(handle.next_ordinal() + k as u64, fault);
            let err = tx.commit().expect_err("failing fault");
            assert!(matches!(err, TxError::Uncertain(_)), "{label}: {err:?}");
            assert_eq!(tx.state(), TxState::Uncertain);

            // Nothing may touch the backend now.
            let (c0, o0) = (calls.get(), handle.next_ordinal());
            assert_eq!(
                tx.commit().err(),
                Some(TxError::ReconcileRequired),
                "{label}"
            );
            assert_eq!(tx.write_at(0, &[1]), Err(TxError::ReconcileRequired));
            assert_eq!(tx.cancel(), Err(TxError::ReconcileRequired));
            drop(tx);
            assert!(file.is_poisoned());
            assert!(matches!(
                CowTx::begin(&mut file),
                Err(TxError::ReconcileRequired)
            ));
            let blk = vec![1u8; BLOCK_SIZE];
            assert_eq!(
                file.commit(&[(0, blk.as_slice())]).err(),
                Some(CowError::Poisoned)
            );
            assert_eq!(
                file.commit_cancellable(&[(0, blk.as_slice())], &mut |_| false)
                    .err(),
                Some(CowError::Poisoned)
            );
            assert_eq!((calls.get(), handle.next_ordinal()), (c0, o0), "{label}");

            // Reconcile = drop the session and reopen the crashed media.
            drop(file);
            handle.crash(CrashPolicy::Seeded(cases));
            let storage = CheckedStorage::new(handle.open()).expect("caps");
            let (mut file, _) = CowFile::open(storage).unwrap_or_else(|e| {
                panic!(
                    "{}",
                    handle.report(&format!("{label}: reopen refused {e:?}"))
                )
            });
            let image = flat(&file.snapshot().expect("verified"));
            assert!(
                image == model || image == new,
                "{label}: reopened image is neither old nor whole new"
            );
            // Only after reconcile does a fresh transaction work.
            write_all(&mut file, &[(0, vec![0xAB; 8])]);
            assert_eq!(file.read_block(0).expect("block")[..8], [0xAB; 8]);
        }
    }
    assert!(cases > 15, "{cases}");
}

// ------------------------------------- S9-11 (f) ack order and dependencies

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Step {
    SetLen(u64),
    Write(u64, u64),
    Flush,
    Ack,
}

pub(crate) fn steps(handle: &FaultHandle, from: usize) -> Vec<Step> {
    fn field(line: &str, key: &str) -> Option<u64> {
        let rest = line.split(key).nth(1)?;
        rest.split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()
    }
    handle.trace()[from..]
        .iter()
        .filter_map(|l| {
            if l == "harness: ack" {
                return Some(Step::Ack);
            }
            let (_, kind) = parse_kind(l)?;
            Some(match kind {
                OpKind::SetLen => Step::SetLen(field(l, "len=")?),
                OpKind::Write => Step::Write(field(l, "off=")?, field(l, "len=")?),
                OpKind::Flush => Step::Flush,
            })
        })
        .collect()
}

/// data -> barrier 1 -> root -> barrier 2 -> ack, read from the backend trace,
/// and the durable image at the moment of the ack.
#[test]
fn ack_follows_data_barrier_root_barrier_in_the_trace() {
    let mut rng = SplitMix64(0xF1);
    let mut sim = TxSim::new(0xF1);
    sim.tx(&[(0, rnd(&mut rng, 3 * BLOCK_SIZE))], None)
        .expect("prior");
    let (old_end, old_gen) = (sim.file().end_offset(), sim.file().generation());
    let from = sim.handle.trace().len();
    sim.tx(&[(5, rnd(&mut rng, 20)), (1030, rnd(&mut rng, 700))], None)
        .expect("tx");
    let s = steps(&sim.handle, from);
    let new_gen = old_gen + 1;

    assert_eq!(s.last(), Some(&Step::Ack), "{s:?}");
    assert_eq!(s.iter().filter(|x| **x == Step::Ack).count(), 1);
    let flushes: Vec<usize> = (0..s.len()).filter(|&i| s[i] == Step::Flush).collect();
    assert_eq!(flushes.len(), 2, "{s:?}");
    let (b1, b2) = (flushes[0], flushes[1]);
    assert_eq!(
        b2 + 1,
        s.len() - 1,
        "nothing between barrier 2 and the ack: {s:?}"
    );
    // Before barrier 1: one resize, then only appends beyond the published end
    // (published bytes are never overwritten); the last write is the root copy
    // in the last sector of the new file.
    let Step::SetLen(new_len) = s[0] else {
        panic!("first step must be the resize: {s:?}")
    };
    assert!(new_len > old_end);
    let writes: Vec<(u64, u64)> = s[1..b1]
        .iter()
        .map(|x| match x {
            Step::Write(o, l) => (*o, *l),
            other => panic!("unexpected step before barrier 1: {other:?}"),
        })
        .collect();
    assert!(writes.len() >= 3, "data, map, copy: {writes:?}");
    assert!(writes.iter().all(|(o, _)| *o >= old_end), "{writes:?}");
    let (copy_off, copy_len) = *writes.last().expect("copy");
    assert_eq!((copy_off + copy_len, copy_len), (new_len, 512));
    // Between the barriers: exactly the root, in the slot of the new generation.
    assert_eq!(
        s[b1 + 1..b2],
        [Step::Write(slot_offset(new_gen), 512)],
        "{s:?}"
    );
    assert!(slot_offset(new_gen) < HEADER_LEN);
    // At the ack nothing is pending: every byte is durable.
    assert_eq!(sim.handle.pending_len(), 0);
    assert_eq!(sim.handle.durable_bytes(), sim.handle.volatile_bytes());

    // Dependencies before the root: when the root write itself fails, the
    // whole candidate is already durable (barrier 1 covered it) and the slot
    // of the new generation still holds its old bytes.
    let mut sim = TxSim::new(0xF2);
    sim.tx(&[(0, rnd(&mut rng, 3 * BLOCK_SIZE))], None)
        .expect("prior");
    let (old_end, cur_gen) = (sim.file().end_offset() as usize, sim.file().generation());
    let slot = slot_offset(cur_gen + 1) as usize;
    let slot_before = sim.handle.volatile_bytes()[slot..slot + 512].to_vec();
    let writes = vec![(5u64, rnd(&mut rng, 20)), (1030u64, rnd(&mut rng, 700))];
    let ops = measure_ops(3, &writes);
    let b1 = ops.iter().position(|k| *k == OpKind::Flush).expect("flush");
    let fail = Fault::Write(crate::testing::fault::WriteFault::Fail(
        std::io::ErrorKind::Other,
    ));
    let r = sim.tx(&writes, Some(((b1 + 1) as u64, fail)));
    assert!(matches!(r, Err(TxError::Uncertain(_))), "{r:?}");
    assert_eq!(sim.handle.pending_len(), 0, "nothing after barrier 1 yet");
    let durable = sim.handle.durable_bytes();
    assert!(durable.len() > old_end, "candidate is on media");
    assert_eq!(durable, sim.handle.volatile_bytes());
    assert_eq!(
        durable[slot..slot + 512],
        slot_before[..],
        "root not written"
    );
}

#[test]
fn a_broken_order_is_caught_by_the_ack_durability_check() {
    // Control: the correct commit leaves nothing pending at the ack.
    let mut rng = SplitMix64(0xF3);
    let handle = FaultHandle::new(&[], 0xF3);
    let (mut file, _) =
        CowFile::create(CheckedStorage::new(handle.open()).expect("caps")).expect("create");
    let blk = rnd(&mut rng, BLOCK_SIZE);
    file.commit(&[(0, blk.as_slice())]).expect("correct order");
    assert_eq!(handle.pending_len(), 0);
    // Broken: the ack of barrier 1 is returned while the root is still pending.
    let _ack = file
        .commit_with_order(&[(0, blk.as_slice())], CommitOrder::AckBeforeBarrier2)
        .expect("returns");
    assert_eq!(
        handle.pending_len(),
        1,
        "root write not covered by any barrier"
    );
    assert_ne!(handle.durable_bytes(), handle.volatile_bytes());
}

// ------------------------------------- S9-11 old generation stays usable

#[test]
fn published_objects_are_immutable_and_the_old_generation_stays_usable() {
    let mut rng = SplitMix64(0xD1);
    let mut sim = TxSim::new(0xD1);
    let base = rnd(&mut rng, 4 * BLOCK_SIZE);
    sim.tx(&[(0, base)], None).expect("g2");
    let old_model = sim.oracle.base();
    let (old_gen, old_end) = (sim.file().generation(), sim.file().end_offset() as usize);
    let before = sim.handle.volatile_bytes();
    sim.tx(
        &[(100, rnd(&mut rng, 600)), (2000, rnd(&mut rng, 10))],
        None,
    )
    .expect("g3");
    let after = sim.handle.volatile_bytes();
    let new_gen = sim.file().generation();
    assert_eq!(new_gen, old_gen + 1);

    // Everything below the old published end is byte-identical, except the one
    // slot the new generation lives in (it held generation N-1).
    let slot = slot_offset(new_gen) as usize;
    for at in 0..old_end {
        if (slot..slot + 512).contains(&at) {
            continue;
        }
        assert_eq!(after[at], before[at], "published byte {at} changed");
    }
    // The old root is intact and still opens on its own: cut the file back to
    // its old end and wipe the new slot (what a lost publication looks like).
    let mut image = after[..old_end].to_vec();
    image[slot..slot + 512].fill(0);
    let h = FaultHandle::new(&image, 1);
    let (view, report) = CowView::salvage(CheckedStorage::new(h.open()).expect("caps"))
        .expect("old generation opens");
    assert_eq!(view.generation(), old_gen);
    assert_eq!(flat(&view.snapshot().expect("verified")), old_model);
    assert!(report.possible_lost_commit, "{report:?}");
}
