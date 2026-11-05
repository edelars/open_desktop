//! Crash recovery against the durable watermark (S9-12).
//!
//! The oracle is the byte-level one from `cow_tx_tests`: the last acknowledged
//! disk image as an independent `Vec<u8>`, plus the target image of a commit
//! whose result was uncertain. "Durable watermark" = the last acknowledged
//! commit: recovery must show it exactly, or the target of a commit that may
//! have reached media; anything else is a failure, never tolerated data loss.
//!
//! Scope and semantics under test (declared, prototype):
//!
//! * recovery is **idempotent**: after one open the state is the same for every
//!   further open, and a state that needs no repair is opened with zero
//!   mutating backend calls;
//! * the only mutation recovery ever makes is the roll-forward repair of one
//!   root slot (one write plus one barrier); an unreferenced tail is ignored,
//!   not cleaned (nothing is *proved* garbage that is worth a write, and there
//!   is no reclaim in the prototype);
//! * [`CowView::inspect`] shows what a writable open would do, without writing.
//!
//! Not covered: real power loss, reclaim/GC, snapshots.

use crate::cow_proto::{
    BLOCK_SIZE, CommitOrder, CowError, CowFile, CowView, OpenClass, OpenReport, slot_offset,
};
use crate::cow_proto_tests::{OpKind, faults_for, is_failing, policies, writable};
use crate::cow_tx::{CowTx, TxError};
use crate::cow_tx_tests::{
    ByteOracle, Step, TxSim, Verdict, apply, flat, measure_ops, peek, steps,
};
use crate::io::CheckedStorage;
use crate::testing::fault::{CrashPolicy, Fault, FaultHandle, FlushFault, WriteFault};
use crate::testing::rng::SplitMix64;

// ------------------------------------------------------------------ helpers

fn image_of<B: crate::io::StorageBackend>(file: &CowFile<B>) -> Vec<u8> {
    flat(&file.snapshot().expect("verified at open"))
}

/// A writable open of the handle's current media; returns the report, the
/// generation, the image and the mutating backend calls the open made.
struct Opened {
    report: OpenReport,
    generation: u64,
    image: Vec<u8>,
    ops: Vec<Step>,
}

fn open_writable(h: &FaultHandle) -> Result<Opened, CowError> {
    let from = h.trace().len();
    let (file, report) = writable(h)?;
    Ok(Opened {
        report,
        generation: file.generation(),
        image: image_of(&file),
        ops: steps(h, from),
    })
}

fn rnd(rng: &mut SplitMix64, n: usize) -> Vec<u8> {
    rng.bytes(n)
}

/// An acknowledged history of `txs` transactions (random partial and
/// extending ranges) on a fresh file. Returns the sim and every acknowledged
/// image, oldest first (the first one is the empty disk).
fn history(seed: u64, txs: u64) -> (TxSim, Vec<Vec<u8>>) {
    let mut sim = TxSim::new(seed);
    let mut rng = SplitMix64(seed);
    let mut images = vec![Vec::new()];
    for t in 0..txs {
        let writes = if t == 0 {
            vec![(0u64, rnd(&mut rng, 3 * BLOCK_SIZE))]
        } else {
            vec![
                (rng.below(1500), rnd(&mut rng, 300)),
                (BLOCK_SIZE as u64 * (2 + t) + 7, rnd(&mut rng, 40)),
            ]
        };
        sim.tx(&writes, None).expect("acknowledged tx");
        images.push(sim.oracle.base());
    }
    (sim, images)
}

/// Media of a commit whose root write was torn after barrier 1: the candidate
/// (with its root copy) is durable, the slot of the new generation is damaged.
struct Torn {
    media: Vec<u8>,
    old: Vec<u8>,
    new: Vec<u8>,
    old_generation: u64,
}

fn torn_root(seed: u64) -> Torn {
    let (mut sim, _) = history(seed, 2);
    let mut rng = SplitMix64(seed ^ 0x70);
    let old = sim.oracle.base();
    let old_generation = sim.file().generation();
    let writes = vec![
        (5u64, rnd(&mut rng, 20)),
        (1030, rnd(&mut rng, 700)),
        (2400, rnd(&mut rng, 10)),
    ];
    let mut new = old.clone();
    for (o, d) in &writes {
        apply(&mut new, *o as usize, d);
    }
    let ops = measure_ops(3, &writes);
    let b1 = ops
        .iter()
        .position(|k| *k == OpKind::Flush)
        .expect("barrier 1");
    let torn = Fault::Write(WriteFault::TornIo { written: 100 });
    let r = sim.tx(&writes, Some(((b1 + 1) as u64, torn)));
    assert!(matches!(r, Err(TxError::Uncertain(_))), "{r:?}");
    sim.file = None;
    sim.handle.crash(CrashPolicy::KeepPending);
    Torn {
        media: sim.handle.durable_bytes(),
        old,
        new,
        old_generation,
    }
}

// --------------------------------------------------------- (a) idempotence

/// open -> recovery -> close -> open gives the same generation and bytes; a
/// state that needs no repair is opened with zero mutating backend calls and
/// leaves the media byte-identical.
#[test]
fn reopen_is_idempotent_and_a_committed_state_writes_nothing() {
    let (mut sim, images) = history(0xA1, 4);
    let last = images.last().expect("image").clone();
    sim.file = None;
    sim.handle.crash(CrashPolicy::DropPending);
    let h = sim.handle.clone();
    let (ord, bytes) = (h.next_ordinal(), h.volatile_bytes());
    let first = open_writable(&h).expect("open");
    assert_eq!(first.report.class(), OpenClass::Committed);
    assert_eq!(first.image, last, "acknowledged state, exactly");
    for round in 0..5 {
        let again = open_writable(&h).expect("reopen");
        assert_eq!(again.generation, first.generation, "round {round}");
        assert_eq!(again.image, first.image, "round {round}");
        assert_eq!(again.report, first.report, "round {round}");
        assert!(again.ops.is_empty(), "round {round}: {:?}", again.ops);
    }
    assert_eq!(h.next_ordinal(), ord, "no mutating call at all");
    assert_eq!(h.volatile_bytes(), bytes, "media untouched");

    // An unfinished candidate (crash before barrier 1, everything kept) is
    // ignored, not cleaned: same state every time, still no write.
    let (mut sim, images) = history(0xA2, 3);
    let acked = images.last().expect("image").clone();
    let writes = vec![(9u64, rnd(&mut SplitMix64(1), 600))];
    let ops = measure_ops(3, &writes);
    let b1 = ops
        .iter()
        .position(|k| *k == OpKind::Flush)
        .expect("barrier 1");
    let r = sim.tx(
        &writes,
        Some((b1 as u64, Fault::Flush(FlushFault::IoNotDurable))),
    );
    assert!(matches!(r, Err(TxError::Uncertain(_))));
    sim.file = None;
    sim.handle.crash(CrashPolicy::KeepPending);
    let h = sim.handle.clone();
    let (ord, bytes) = (h.next_ordinal(), h.volatile_bytes());
    let first = open_writable(&h).expect("open with tail");
    assert_eq!(first.report.class(), OpenClass::UnfinishedCandidate);
    assert_eq!(first.image, acked);
    for _ in 0..3 {
        let again = open_writable(&h).expect("reopen");
        assert_eq!(
            (again.generation, &again.image, again.report),
            (first.generation, &first.image, first.report)
        );
    }
    assert_eq!((h.next_ordinal(), h.volatile_bytes()), (ord, bytes));

    // Roll-forward: the first open repairs exactly one slot (write + barrier);
    // every later open is a plain Committed open with zero calls, same state.
    let torn = torn_root(0xA3);
    let h = FaultHandle::new(&torn.media, 0xA3);
    let first = open_writable(&h).expect("rolled forward");
    assert_eq!(first.report.class(), OpenClass::RolledForward);
    assert_eq!(first.generation, torn.old_generation + 1);
    assert_eq!(first.image, torn.new);
    assert_eq!(
        first.ops,
        [Step::Write(slot_offset(first.generation), 512), Step::Flush],
        "the repair is one slot write and one barrier"
    );
    for round in 0..3 {
        let again = open_writable(&h).expect("reopen");
        assert_eq!(again.report.class(), OpenClass::Committed, "round {round}");
        assert_eq!(
            (again.generation, &again.image),
            (first.generation, &first.image)
        );
        assert!(again.ops.is_empty(), "round {round}: {:?}", again.ops);
    }
    // And it stays so across a crash: the repair was behind its own barrier.
    h.crash(CrashPolicy::DropPending);
    let after = open_writable(&h).expect("after crash");
    assert_eq!(after.report.class(), OpenClass::Committed);
    assert_eq!(after.image, torn.new);
}

// ------------------------------------------------- (b) crash during recovery

/// Checks that, whatever the previous (crashed) recoveries did, a clean open
/// lands on the rolled-forward state, and a further open is a no-op.
fn assert_converges(h: &FaultHandle, torn: &Torn, label: &str) {
    let first =
        open_writable(h).unwrap_or_else(|e| panic!("{}", h.report(&format!("{label}: {e:?}"))));
    assert_eq!(first.generation, torn.old_generation + 1, "{label}");
    assert_eq!(
        first.image, torn.new,
        "{label}: state changed by a crashed recovery"
    );
    assert!(
        matches!(
            first.report.class(),
            OpenClass::RolledForward | OpenClass::Committed
        ),
        "{label}: {:?}",
        first.report
    );
    let (ord, bytes) = (h.next_ordinal(), h.volatile_bytes());
    let second = open_writable(h).expect("second");
    assert_eq!(second.report.class(), OpenClass::Committed, "{label}");
    assert_eq!(
        (second.generation, &second.image),
        (first.generation, &first.image)
    );
    assert_eq!(
        (h.next_ordinal(), h.volatile_bytes()),
        (ord, bytes),
        "{label}"
    );
}

/// A fault on every mutating step of the recovery (the slot repair write and
/// its barrier) x every fault kind x every crash outcome: the next open always
/// reaches the same state, and nothing is lost.
#[test]
fn crash_during_recovery_converges_to_the_same_state() {
    let torn = torn_root(0xB1);
    let mut cases = 0;
    // Recovery's mutating steps: ordinal 0 = repair write, 1 = its barrier.
    for (k, kind) in [(0u64, OpKind::Write), (1, OpKind::Flush)] {
        for fault in faults_for(kind) {
            for policy in policies() {
                cases += 1;
                let h = FaultHandle::new(&torn.media, 0xB100 + cases);
                h.script(h.next_ordinal() + k, fault);
                let attempt = writable(&h);
                if is_failing(fault) {
                    assert!(
                        matches!(attempt, Err(CowError::Storage(_))),
                        "{kind:?} {fault:?}: {:?}",
                        attempt.as_ref().err()
                    );
                } else {
                    assert!(attempt.is_ok(), "short write is completed by the wrapper");
                }
                drop(attempt);
                h.crash(policy);
                // The read-only view agrees on the state before any repair.
                let seen = peek(&h).unwrap_or_else(|e| {
                    panic!(
                        "{}",
                        h.report(&format!("{kind:?} {fault:?} {policy:?}: peek {e:?}"))
                    )
                });
                assert_eq!(seen, torn.new, "{kind:?} {fault:?} {policy:?}");
                assert_converges(&h, &torn, &format!("{kind:?} {fault:?} {policy:?}"));
            }
        }
    }
    assert!(cases > 50, "{cases}");
}

/// Three crashed recoveries in a row, random faults and crash outcomes.
#[test]
fn repeated_crashed_recoveries_converge() {
    let torn = torn_root(0xB2);
    for seed in 0..60u64 {
        let mut rng = SplitMix64(0xB200 + seed);
        let h = FaultHandle::new(&torn.media, seed);
        for _ in 0..3 {
            let (k, kind) = if rng.below(2) == 0 {
                (0, OpKind::Write)
            } else {
                (1, OpKind::Flush)
            };
            let choices = faults_for(kind);
            let fault = choices[rng.below(choices.len() as u64) as usize];
            h.script(h.next_ordinal() + k, fault);
            let _ = writable(&h);
            let policy = match rng.below(3) {
                0 => CrashPolicy::DropPending,
                1 => CrashPolicy::KeepPending,
                _ => CrashPolicy::Seeded(rng.next_u64()),
            };
            h.crash(policy);
        }
        assert_converges(&h, &torn, &format!("seed {seed}"));
    }
}

// ------------------------------------------------------------ (c) delayed writes

/// Writes accepted after a barrier but not yet behind one are the only thing
/// a crash may still decide. After an ack there are none; before barrier 2
/// there is exactly the root; before barrier 1 the whole candidate. The oracle
/// accepts only the allowed images in every case.
#[test]
fn only_unflushed_writes_are_decided_by_a_crash() {
    let writes = |rng: &mut SplitMix64| vec![(5u64, rnd(rng, 20)), (1030u64, rnd(rng, 700))];
    let ops = measure_ops(3, &writes(&mut SplitMix64(1)));
    let b1 = ops
        .iter()
        .position(|k| *k == OpKind::Flush)
        .expect("barrier 1");
    let b2 = ops.len() - 1;
    let mut seen_before = 0;
    let mut seen_after = 0;
    for (name, k, fault) in [
        (
            "barrier 1 fails",
            b1,
            Fault::Flush(FlushFault::IoNotDurable),
        ),
        (
            "barrier 2 fails",
            b2,
            Fault::Flush(FlushFault::IoNotDurable),
        ),
    ] {
        for policy in (0..48u64)
            .map(CrashPolicy::Seeded)
            .chain([CrashPolicy::DropPending, CrashPolicy::KeepPending])
        {
            let mut sim = TxSim::new(0xC1);
            let mut rng = SplitMix64(0xC1);
            sim.tx(&[(0, rnd(&mut rng, 3 * BLOCK_SIZE))], None)
                .expect("prior");
            let old = sim.oracle.base();
            let w = writes(&mut rng);
            let r = sim.tx(&w, Some((k as u64, fault)));
            assert!(matches!(r, Err(TxError::Uncertain(_))), "{name}: {r:?}");
            // What is pending right now: the candidate, or only the root.
            let pending = sim.handle.pending_len();
            if k == b2 {
                assert_eq!(pending, 1, "only the root write is behind no barrier");
            } else {
                assert!(pending >= 4, "{pending}: whole candidate is pending");
            }
            let v = sim.must_crash_reopen(policy);
            match (k == b2, v) {
                (false, Verdict::After) => {
                    panic!("{name} {policy:?}: crash before barrier 1 published the commit")
                }
                (_, Verdict::Before) => seen_before += 1,
                (_, Verdict::After) => seen_after += 1,
            }
            if k == b2 {
                match policy {
                    CrashPolicy::DropPending => assert_eq!(v, Verdict::Before),
                    CrashPolicy::KeepPending => assert_eq!(v, Verdict::After),
                    CrashPolicy::Seeded(_) => {}
                }
            } else {
                assert_eq!(sim.oracle.base(), old, "{name} {policy:?}");
            }
        }
    }
    assert!(
        seen_before > 0 && seen_after > 0,
        "{seen_before}/{seen_after}"
    );

    // After an ack nothing is pending: no policy can take the commit back.
    for policy in policies() {
        let mut sim = TxSim::new(0xC2);
        let mut rng = SplitMix64(0xC2);
        sim.tx(&[(0, rnd(&mut rng, 3 * BLOCK_SIZE))], None)
            .expect("prior");
        sim.tx(&writes(&mut rng), None).expect("acked");
        assert_eq!(sim.handle.pending_len(), 0);
        assert_eq!(sim.must_crash_reopen(policy), Verdict::Before);
    }
}

/// The pending writes of an abandoned attempt are stale; they are overwritten
/// by the next commit and cannot corrupt it, even if a crash keeps a mixture
/// of old and new pieces.
#[test]
fn stale_pending_writes_of_an_abandoned_attempt_do_not_leak() {
    for seed in 0..32u64 {
        let mut sim = TxSim::new(0xC3);
        let mut rng = SplitMix64(0xC3);
        sim.tx(&[(0, rnd(&mut rng, 3 * BLOCK_SIZE))], None)
            .expect("prior");
        let a = vec![(100u64, rnd(&mut rng, 900))];
        let ops = measure_ops(3, &a);
        let b1 = ops
            .iter()
            .position(|k| *k == OpKind::Flush)
            .expect("barrier 1");
        // Attempt A dies at barrier 1; its writes stay pending. Reopen with no
        // crash (the process restarted but the volatile bytes survived).
        let r = sim.tx(
            &a,
            Some((b1 as u64, Fault::Flush(FlushFault::IoNotDurable))),
        );
        assert!(matches!(r, Err(TxError::Uncertain(_))));
        sim.file = None;
        let storage = CheckedStorage::new(sim.handle.open()).expect("caps");
        let (file, _) = CowFile::open(storage).expect("reopen on volatile bytes");
        let seen = sim.oracle.observe(&image_of(&file));
        assert_eq!(seen, Ok(Verdict::Before), "A is not visible");
        sim.file = Some(file);
        // Transaction B on the same blocks fails at its barrier 1 as well; a
        // crash now keeps any mix of A's and B's pending pieces.
        let b = vec![(150u64, rnd(&mut rng, 600))];
        let r = sim.tx(
            &b,
            Some((b1 as u64, Fault::Flush(FlushFault::IoNotDurable))),
        );
        assert!(matches!(r, Err(TxError::Uncertain(_))));
        let v = sim.must_crash_reopen(CrashPolicy::Seeded(seed));
        // B never reached its root write, and A's root never existed: old.
        assert_eq!(v, Verdict::Before, "seed {seed}");
        // A later acknowledged commit is exact.
        let c = vec![(150u64, rnd(&mut rng, 600))];
        sim.tx(&c, None).expect("C");
        sim.must_crash_reopen(CrashPolicy::DropPending);
    }
}

/// Review scenario: attempt A of the first commit dies at barrier 1 and a
/// crash keeps its root copy but not all of its blocks; recovery ignores it as
/// speculative (the tail stays). Retry C of the same writes is cancelled after
/// it wrote blocks; a later crash must not complete A's blocks out of C's and
/// adopt the stale copy. The cancel's cut is behind a barrier, so the tail is
/// gone: the old state always.
#[test]
fn a_cancelled_retry_is_not_resurrected_by_a_stale_tail_copy() {
    let writes = vec![(0u64, rnd(&mut SplitMix64(0x51), 2 * BLOCK_SIZE))];
    let b1 = measure_ops(0, &writes)
        .iter()
        .position(|k| *k == OpKind::Flush)
        .expect("barrier 1");
    // Resize, 2 blocks, Map, RootCopy: stops 1..=4 have written something.
    let mut ignored_tails = 0;
    let mut cases = 0;
    for seed in 0..96u64 {
        for stop in 1..=4usize {
            for policy in policies() {
                let mut sim = TxSim::new(0x5100);
                let r = sim.tx(
                    &writes,
                    Some((b1 as u64, Fault::Flush(FlushFault::IoNotDurable))),
                );
                assert!(matches!(r, Err(TxError::Uncertain(_))));
                // Crash A; only the runs where the stale copy is ignored count.
                match sim.crash_reopen(CrashPolicy::Seeded(seed)) {
                    Ok(Verdict::Before)
                        if sim.last_class == Some(OpenClass::UnfinishedCandidate) => {}
                    Ok(_) => continue,
                    Err(m) => sim.fail(&m),
                }
                ignored_tails += 1;
                let mut n = 0usize;
                {
                    let mut tx = CowTx::begin(sim.file()).expect("begin");
                    for (o, d) in &writes {
                        tx.write_at(*o, d).expect("write_at");
                    }
                    let r = tx.commit_cancellable(&mut |_| {
                        n += 1;
                        n - 1 == stop
                    });
                    assert_eq!(r.err(), Some(TxError::Cancelled));
                }
                cases += 1;
                let v = sim.must_crash_reopen(policy);
                assert_eq!(v, Verdict::Before, "seed {seed} stop {stop} {policy:?}");
                assert_eq!(
                    sim.file().generation(),
                    1,
                    "seed {seed} stop {stop} {policy:?}"
                );
                assert!(sim.oracle.base().is_empty());
            }
        }
    }
    assert!(
        ignored_tails > 100 && cases == ignored_tails,
        "{ignored_tails}/{cases}"
    );
}

// -------------------------------------------- (d) corrupt committed extent

fn flip(media: &[u8], at: usize) -> Vec<u8> {
    let mut b = media.to_vec();
    b[at] ^= 0x01;
    b
}

/// A bit flip in any block of the newest mapping refuses both opens with the
/// exact logical index, writes nothing and never falls back to the older root
/// (which would still verify). A block that only an older generation
/// references is unreachable and does not matter.
#[test]
fn corrupt_block_of_a_multi_extent_commit_refuses_without_fallback() {
    let (mut sim, images) = history(0xD1, 2);
    let mut rng = SplitMix64(0xD1);
    // Third commit: three non-adjacent extents, overwriting blocks 0 and 2 and
    // appending block 4 after a gap block.
    let old_image = images.last().expect("image").clone();
    let old_phys: Vec<u64> = (0..sim.file().block_count())
        .map(|b| sim.file().block_offset(b).expect("mapped"))
        .collect();
    let writes = vec![
        (3u64, rnd(&mut rng, 10)),
        (2 * BLOCK_SIZE as u64 + 100, rnd(&mut rng, 50)),
        (4 * BLOCK_SIZE as u64 + 1, rnd(&mut rng, 20)),
    ];
    let mut new_image = old_image.clone();
    for (o, d) in &writes {
        apply(&mut new_image, *o as usize, d);
    }
    sim.tx(&writes, None).expect("multi-extent commit");
    let map: Vec<u64> = (0..sim.file().block_count())
        .map(|b| sim.file().block_offset(b).expect("mapped"))
        .collect();
    sim.file = None;
    sim.handle.crash(CrashPolicy::DropPending);
    let media = sim.handle.durable_bytes();
    assert!(map.len() >= 5);

    for (index, phys) in map.iter().enumerate() {
        for byte in [0usize, 255, 511] {
            let bad = flip(&media, *phys as usize + byte);
            let h = FaultHandle::new(&bad, 0xD2);
            let w = writable(&h).err().expect("writable refuses");
            let s = CowView::inspect(CheckedStorage::new(h.open()).expect("caps"))
                .err()
                .expect("read-only refuses");
            let want = CowError::BlockCorrupt {
                index: index as u64,
            };
            assert_eq!(w, want, "block {index} +{byte}");
            assert_eq!(s, want, "block {index} +{byte}");
            assert_eq!(h.next_ordinal(), 0, "a refusal writes nothing");
            assert_eq!(h.volatile_bytes(), bad);
        }
    }
    // Blocks that only generation N-1 references (replaced by the commit) are
    // garbage as far as the newest root is concerned: opens fine, same image.
    let replaced: Vec<u64> = old_phys
        .iter()
        .copied()
        .filter(|p| !map.contains(p))
        .collect();
    assert!(!replaced.is_empty(), "the commit replaced published blocks");
    for phys in replaced {
        let h = FaultHandle::new(&flip(&media, phys as usize + 9), 0xD3);
        let opened = open_writable(&h).expect("unreferenced damage is irrelevant");
        assert_eq!(opened.image, new_image);
        assert!(opened.ops.is_empty());
    }

    // The same for a rolled-forward candidate: its blocks are proven complete
    // by the root copy, so damage is a refusal and the older root is not used.
    let torn = torn_root(0xD4);
    let probe: Vec<u64> = {
        let h = FaultHandle::new(&torn.media, 1);
        let (f, _) = CowView::inspect(CheckedStorage::new(h.open()).expect("caps"))
            .expect("inspect rolled-forward");
        (0..f.block_count())
            .map(|b| f.block_offset(b).expect("mapped"))
            .collect()
    };
    assert!(probe.len() >= 4);
    for (index, phys) in probe.iter().enumerate() {
        let h = FaultHandle::new(&flip(&torn.media, *phys as usize + 3), 3);
        let w = writable(&h).err().expect("refuses");
        assert_eq!(
            w,
            CowError::BlockCorrupt {
                index: index as u64
            }
        );
        assert_eq!(h.next_ordinal(), 0, "no repair on top of damage");
    }
}

// ------------------------------------------ (e) unack-but-committed root

/// The root became durable but the ack was lost: the commit returns an error,
/// yet the media holds the new state. Open shows it, as a plain committed
/// state; the oracle accepts it as "possibly committed" and the commit was
/// never acknowledged, so the old image is acceptable too.
#[test]
fn unacknowledged_but_committed_root_is_a_legal_outcome() {
    let writes = |rng: &mut SplitMix64| vec![(5u64, rnd(rng, 20)), (1030u64, rnd(rng, 700))];
    let ops = measure_ops(3, &writes(&mut SplitMix64(1)));
    let b2 = ops.len() - 1;
    for policy in policies() {
        let mut sim = TxSim::new(0xE1);
        let mut rng = SplitMix64(0xE1);
        sim.tx(&[(0, rnd(&mut rng, 3 * BLOCK_SIZE))], None)
            .expect("prior");
        let w = writes(&mut rng);
        let r = sim.tx(
            &w,
            Some((b2 as u64, Fault::Flush(FlushFault::IoAfterDurable))),
        );
        assert!(matches!(r, Err(TxError::Uncertain(_))), "{r:?}");
        // The error says nothing about the media: the root is durable.
        assert_eq!(sim.handle.pending_len(), 0);
        let v = sim.must_crash_reopen(policy);
        assert_eq!(
            v,
            Verdict::After,
            "{policy:?}: the durable root is the state"
        );
        assert_eq!(sim.last_class, Some(OpenClass::Committed));
        // Reconciled: later transactions build on the new state.
        let expect = sim.oracle.base();
        sim.tx(&[(0, vec![0x42; 4])], None)
            .expect("after reconcile");
        let mut want = expect;
        apply(&mut want, 0, &[0x42; 4]);
        assert_eq!(sim.oracle.base(), want);
    }

    // The lost ack at barrier 1 leaves the candidate durable but unpublished:
    // the old state (this is not the unack-but-committed case).
    for policy in policies() {
        let mut sim = TxSim::new(0xE2);
        let mut rng = SplitMix64(0xE2);
        sim.tx(&[(0, rnd(&mut rng, 3 * BLOCK_SIZE))], None)
            .expect("prior");
        let b1 = ops
            .iter()
            .position(|k| *k == OpKind::Flush)
            .expect("barrier 1");
        let w = writes(&mut rng);
        let r = sim.tx(
            &w,
            Some((b1 as u64, Fault::Flush(FlushFault::IoAfterDurable))),
        );
        assert!(matches!(r, Err(TxError::Uncertain(_))));
        assert_eq!(sim.must_crash_reopen(policy), Verdict::Before);
        assert_eq!(sim.last_class, Some(OpenClass::UnfinishedCandidate));
    }
}

// ----------------------------------- (f) lost acknowledged write fails the oracle

/// An older valid root that has lost an acknowledged write must fail the
/// oracle, while the same image is accepted when the write was never
/// acknowledged. (S9-08 holds the full detection runs of the broken orders;
/// this is the short form, plus one real broken-order run through the
/// transaction layer's file.)
#[test]
fn an_older_valid_root_with_a_lost_acknowledged_write_fails_the_oracle() {
    let (mut sim, images) = history(0xF1, 3);
    let older = images[2].clone();
    let newest = images[3].clone();
    assert_ne!(older, newest);
    // The older generation's media is perfectly valid: it opens cleanly.
    let (sim_old, _) = history(0xF1, 2);
    let h = FaultHandle::new(&sim_old.handle.durable_bytes(), 1);
    let opened = open_writable(&h).expect("older root opens");
    assert_eq!(opened.image, older);

    let mut acked = ByteOracle::new();
    acked.ack(newest.clone());
    let err = acked.check(&opened.image).expect_err("lost acked write");
    assert!(err.contains("neither the acknowledged"), "{err}");

    let mut uncertain = ByteOracle::new();
    uncertain.ack(older.clone());
    uncertain.begin(newest.clone());
    assert_eq!(uncertain.check(&opened.image), Ok(Verdict::Before));
    assert_eq!(uncertain.check(&newest), Ok(Verdict::After));

    // Real run: a commit that acknowledges before barrier 2 loses the write
    // when the pending root is dropped; the oracle notices.
    let blk = rnd(&mut SplitMix64(5), BLOCK_SIZE);
    let file = sim.file();
    file.commit_with_order(&[(0, blk.as_slice())], CommitOrder::AckBeforeBarrier2)
        .expect("early ack");
    let mut target = newest.clone();
    apply(&mut target, 0, &blk);
    sim.oracle.ack(target);
    let verdict = sim.crash_reopen(CrashPolicy::DropPending);
    let msg = verdict.expect_err("acknowledged write lost");
    assert!(msg.contains("neither the acknowledged"), "{msg}");
}

// --------------------------------------------- (g) side-effect-free inspect

/// `CowView::inspect` never changes a byte or calls a mutating operation, on
/// any kind of media, and its report predicts the writable open: a
/// roll-forward means exactly one slot write plus one barrier, anything else
/// means no write at all; refusals agree.
#[test]
fn inspect_is_side_effect_free_and_predicts_the_writable_open() {
    let (mut clean, _) = history(0x91, 3);
    clean.file = None;
    clean.handle.crash(CrashPolicy::DropPending);
    let clean = clean.handle.durable_bytes();

    // Unfinished candidate: crash before barrier 1, everything kept.
    let tail = {
        let (mut sim, _) = history(0x92, 3);
        let w = vec![(9u64, rnd(&mut SplitMix64(1), 600))];
        let b1 = measure_ops(3, &w)
            .iter()
            .position(|k| *k == OpKind::Flush)
            .expect("barrier 1");
        let _ = sim.tx(
            &w,
            Some((b1 as u64, Fault::Flush(FlushFault::IoNotDurable))),
        );
        sim.file = None;
        sim.handle.crash(CrashPolicy::KeepPending);
        sim.handle.durable_bytes()
    };
    let torn = torn_root(0x93);
    // Torn root without a usable copy: unexplained damage.
    let lost = {
        let mut b = torn.media.clone();
        let n = b.len();
        b[n - 512..].fill(0);
        b
    };
    let mut rotten = clean.clone();
    {
        let h = FaultHandle::new(&clean, 1);
        let (v, _) = CowView::inspect(CheckedStorage::new(h.open()).expect("caps")).expect("v");
        let at = v.block_offset(1).expect("block") as usize;
        rotten[at + 4] ^= 0x80;
    }

    type Case<'a> = (&'a str, Vec<u8>, Result<OpenClass, ()>);
    let cases: Vec<Case> = vec![
        ("committed", clean, Ok(OpenClass::Committed)),
        ("unfinished tail", tail, Ok(OpenClass::UnfinishedCandidate)),
        (
            "torn root with copy",
            torn.media.clone(),
            Ok(OpenClass::RolledForward),
        ),
        (
            "torn root without copy",
            lost,
            Ok(OpenClass::PossibleLostCommit),
        ),
        ("corrupt block", rotten, Err(())),
        ("empty file", Vec::new(), Err(())),
    ];
    for (name, bytes, want) in cases {
        let h = FaultHandle::new(&bytes, 0x94);
        let inspected = CowView::inspect(CheckedStorage::new(h.open()).expect("caps"));
        assert_eq!(h.next_ordinal(), 0, "{name}: inspect made a mutating call");
        assert_eq!(h.pending_len(), 0, "{name}");
        assert_eq!(h.volatile_bytes(), bytes, "{name}: inspect changed bytes");
        match (&inspected, want) {
            (Ok((_, report)), Ok(class)) => assert_eq!(report.class(), class, "{name}"),
            (Err(_), Err(())) => {}
            (got, want) => panic!(
                "{name}: inspect {:?}, wanted {want:?}",
                got.as_ref().map(|r| r.1)
            ),
        }

        let h2 = FaultHandle::new(&bytes, 0x95);
        let opened = open_writable(&h2);
        match (&inspected, &opened) {
            (Ok((view, report)), Ok(o)) => {
                assert_eq!(o.generation, view.generation(), "{name}");
                assert_eq!(o.image, flat(&view.snapshot().expect("verified")), "{name}");
                let expect_ops = if report.rolled_forward {
                    vec![Step::Write(slot_offset(o.generation), 512), Step::Flush]
                } else {
                    Vec::new()
                };
                assert_eq!(o.ops, expect_ops, "{name}: report predicted the mutation");
            }
            (Ok((_, report)), Err(e)) => {
                // Only unexplained damage is read-only; the writable open
                // refuses it without writing.
                assert!(report.possible_lost_commit, "{name}: {e:?}");
                assert!(
                    matches!(e, CowError::DamagedOtherSlot { .. }),
                    "{name}: {e:?}"
                );
                assert_eq!(h2.next_ordinal(), 0, "{name}: refusal wrote");
            }
            (Err(a), Err(b)) => {
                assert_eq!(a, b, "{name}: inspect and writable disagree");
                assert_eq!(h2.next_ordinal(), 0, "{name}: refusal wrote");
            }
            (Err(a), Ok(_)) => panic!("{name}: inspect refused ({a:?}) but writable opened"),
        }
    }

    // The salvaged older state of the "without copy" media is the old image
    // and is flagged: it can never be mistaken for a clean open.
    let mut b = torn.media.clone();
    let n = b.len();
    b[n - 512..].fill(0);
    let h = FaultHandle::new(&b, 7);
    let (view, report) =
        CowView::inspect(CheckedStorage::new(h.open()).expect("caps")).expect("salvage");
    assert!(report.possible_lost_commit);
    assert_eq!(flat(&view.snapshot().expect("verified")), torn.old);
}
