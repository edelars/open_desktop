//! Tests for create, open and root selection of the CoW prototype (S9-10).
//!
//! `cow_proto_tests` proves the commit protocol against a snapshot oracle.
//! This file checks the other half of the contract: *which state an open
//! decides on* and which media it refuses. Expectations are written as
//! (generation, [`OpenClass`], snapshot) triples or as the refusal kind. `create`
//! leaves two empty roots (generations 1 and 2) and the first commit is
//! generation 3, so the snapshot of generation `g` is `snaps[g - 2]` (and the
//! empty `snaps[0]` also stands for generation 1).
//!
//! Every media image comes from a real run on the fault backend, optionally
//! followed by an explicit byte mutation, so the layout is never re-derived
//! here except for the few forged images that exist to be hostile.

use crate::cow_parse::{Capabilities, Features};
use crate::cow_proto::{
    BLOCK_SIZE, CommitOrder, CowError, CowFile, CowView, HEADER_LEN, OpenClass, OpenReport,
    SLOT_SIZE, SlotState, crc32, forge_root_sector, slot_offset,
};
use crate::cow_proto_tests::{
    Block, OpKind, Sim, Snap, block, faults_for, forged, history, is_failing, policies, tx_ops,
    with_bytes, writable,
};
use crate::io::CheckedStorage;
use crate::testing::fault::{CrashPolicy, Fault, FaultHandle, FlushFault, WriteFault};
use crate::testing::rng::SplitMix64;
use std::collections::BTreeMap;

// ---------------------------------------------------------------- helpers

type Opened = Result<(u64, Snap, OpenReport), CowError>;
type Mutation = Box<dyn Fn(&mut Vec<u8>)>;
type SlotEdit = Box<dyn Fn(&mut [u8])>;
type Pred = fn(&CowError) -> bool;

fn do_salvage(h: &FaultHandle) -> Opened {
    CowView::salvage(CheckedStorage::new(h.open()).expect("caps")).map(|(v, r)| {
        let snap = v.snapshot().expect("verified at open");
        (v.generation(), snap, r)
    })
}

fn do_writable(h: &FaultHandle) -> Opened {
    writable(h).map(|(f, r)| {
        let snap = f.snapshot().expect("verified at open");
        (f.generation(), snap, r)
    })
}

#[derive(Clone, Copy, Debug)]
enum Want {
    /// Opens at this generation with this class.
    Open(u64, OpenClass),
    NoRoot,
    /// Writable refusal: an unfinished create (the read-only open of the same
    /// media is `NoRoot`).
    Incomplete,
    /// Writable refusal: damaged other slot, with the generation that was
    /// chosen.
    DamagedOther(u64),
}

fn expect(label: &str, who: &str, got: &Opened, want: Want, snaps: &[Snap]) {
    let ok = match (got, want) {
        (Ok((g, snap, r)), Want::Open(wg, class)) => {
            *g == wg
                && r.class() == class
                && r.chosen_slot == (wg & 1) as usize
                && snaps.get(wg.saturating_sub(2) as usize) == Some(snap)
        }
        (Err(CowError::NoValidRoot { .. }), Want::NoRoot) => true,
        (Err(CowError::IncompleteCreate { .. }), Want::Incomplete) => true,
        (
            Err(CowError::DamagedOtherSlot {
                chosen_generation, ..
            }),
            Want::DamagedOther(g),
        ) => *chosen_generation == g,
        _ => false,
    };
    assert!(
        ok,
        "{label} [{who}]: wanted {want:?}, got {:?}",
        got.as_ref()
            .map(|(g, s, r)| (*g, s.len(), r.class(), r.tail_len))
    );
}

/// Opens `bytes` read-only and writable, each on a fresh media copy, and
/// checks both outcomes and that neither refusal nor salvage wrote anything.
/// Returns the durable bytes after the writable open (and a crash).
fn check_open(label: &str, bytes: &[u8], snaps: &[Snap], w: Want, s: Want) -> Vec<u8> {
    let h = FaultHandle::new(bytes, 0x5A1);
    let got = do_salvage(&h);
    assert_eq!(h.next_ordinal(), 0, "{label}: salvage must not write");
    assert_eq!(h.volatile_bytes(), bytes, "{label}: salvage changed bytes");
    assert_eq!(h.pending_len(), 0, "{label}");
    expect(label, "salvage", &got, s, snaps);

    let h = FaultHandle::new(bytes, 0x5A2);
    let got = do_writable(&h);
    if got.is_err() {
        assert_eq!(h.next_ordinal(), 0, "{label}: refusal must not write");
    }
    expect(label, "writable", &got, w, snaps);
    h.crash(CrashPolicy::DropPending);
    h.durable_bytes()
}

fn rows(len: usize, rng: &mut SplitMix64) -> Vec<(u64, Block)> {
    let append = len as u64;
    if len == 0 {
        vec![(0, block(rng)), (1, block(rng))]
    } else {
        vec![(0, block(rng)), (append, block(rng))]
    }
}

fn first_flush(ops: &[OpKind]) -> usize {
    ops.iter().position(|k| *k == OpKind::Flush).expect("flush")
}

// ---------------------------------------------------------- create / open

/// What `create` leaves behind, byte for byte, and how it opens.
#[test]
fn create_makes_two_empty_roots_under_one_barrier() {
    let mut sim = Sim::new(0x10, CommitOrder::Correct);
    assert!(sim.create(None));
    // Ordered backend calls: size, two root writes, one barrier; the ack is the
    // barrier's, so nothing is pending when create returns.
    let ops: Vec<OpKind> = sim
        .handle
        .trace()
        .iter()
        .filter_map(|l| crate::cow_proto_tests::parse_kind(l))
        .map(|(_, k)| k)
        .collect();
    assert_eq!(
        ops,
        [OpKind::SetLen, OpKind::Write, OpKind::Write, OpKind::Flush]
    );
    assert_eq!(sim.handle.pending_len(), 0);

    let bytes = sim.handle.durable_bytes();
    assert_eq!(bytes.len(), HEADER_LEN as usize);
    let empty = |g| forge_root_sector(g, 0, HEADER_LEN, 0, HEADER_LEN);
    assert_eq!(bytes[..SLOT_SIZE], empty(2));
    assert_eq!(bytes[SLOT_SIZE..], empty(1));

    let snaps = vec![Vec::new()];
    check_open(
        "fresh create",
        &bytes,
        &snaps,
        Want::Open(2, OpenClass::Committed),
        Want::Open(2, OpenClass::Committed),
    );
    // The created state reports the companion root and no tail.
    let (_, _, report) = do_salvage(&FaultHandle::new(&bytes, 1)).expect("open");
    assert_eq!(report.other_slot, SlotState::Valid { generation: 1 });
    assert_eq!(report.tail_len, 0);
    assert!(!report.rolled_forward && !report.possible_lost_commit);

    // create refuses a backend that already holds bytes.
    let h = FaultHandle::new(&bytes, 2);
    let r = CowFile::create(CheckedStorage::new(h.open()).expect("caps"));
    assert!(matches!(r, Err(CowError::BadWrite(_))));
    assert_eq!(h.next_ordinal(), 0);
}

#[derive(Clone, Copy, Debug)]
enum Sector {
    Blank,
    Written,
    Torn,
}

/// Every combination of (blank / written / torn) for the two root sectors of a
/// header-sized file, judged by a plain rule that does not look at the code
/// under test: both roots written is the clean generation 2; exactly one is an
/// unfinished create (refused by a writable open, `NoValidRoot` read-only);
/// none is `NoValidRoot`. A shorter file never opens.
#[test]
fn every_sector_state_of_an_unfinished_create() {
    let mut sim = Sim::new(0x11, CommitOrder::Correct);
    assert!(sim.create(None));
    let created = sim.handle.durable_bytes();
    let snaps: Vec<Snap> = vec![Vec::new()];
    let states = [Sector::Blank, Sector::Written, Sector::Torn];
    for s0 in states {
        for s1 in states {
            for (how, torn_edit) in [
                ("magic", (7usize, 0xFFu8)),
                ("crc", (SLOT_SIZE - 1, 0x01)),
                ("body", (20, 0x40)),
            ] {
                let mut bytes = created.clone();
                for (i, st) in [s0, s1].into_iter().enumerate() {
                    let at = i * SLOT_SIZE;
                    match st {
                        Sector::Written => {}
                        Sector::Blank => bytes[at..at + SLOT_SIZE].fill(0),
                        Sector::Torn => bytes[at + torn_edit.0] ^= torn_edit.1,
                    }
                }
                let written = [s0, s1]
                    .iter()
                    .filter(|s| matches!(s, Sector::Written))
                    .count();
                let (w, r) = match written {
                    2 => (
                        Want::Open(2, OpenClass::Committed),
                        Want::Open(2, OpenClass::Committed),
                    ),
                    1 => (Want::Incomplete, Want::NoRoot),
                    _ => (Want::NoRoot, Want::NoRoot),
                };
                check_open(&format!("{s0:?}/{s1:?} ({how})"), &bytes, &snaps, w, r);
            }
        }
    }
    // The slot states of an unfinished create say what is missing.
    let mut bytes = created.clone();
    bytes[..SLOT_SIZE].fill(0);
    match do_writable(&FaultHandle::new(&bytes, 3)) {
        Err(CowError::IncompleteCreate { slots }) => {
            assert_eq!(
                slots,
                [SlotState::Blank, SlotState::Valid { generation: 1 }]
            );
        }
        other => panic!("{:?}", other.map(|r| r.0)),
    }
    // A file shorter than the header, or never written, has no root at all.
    for len in [0usize, 1, 1023] {
        check_open(
            &format!("len {len}"),
            &created[..len],
            &snaps,
            Want::NoRoot,
            Want::NoRoot,
        );
    }
    check_open("zeros", &[0u8; 1024], &snaps, Want::NoRoot, Want::NoRoot);
}

// ------------------------------------------------------- power cut sweeps

#[derive(Clone, Copy, Debug)]
struct Expected<'a> {
    /// Generation and snapshot before the step (`None`: nothing existed).
    prev: Option<(u64, &'a Snap)>,
    target: (u64, &'a Snap),
}

/// Crashes the sim, opens the media read-only then writable, and checks both
/// agree and land on `prev` or `target` only. Returns the outcome key.
fn crash_and_check(sim: &mut Sim, policy: CrashPolicy, exp: Expected, label: &str) -> String {
    sim.file = None;
    sim.handle.crash(policy);
    let h = sim.handle.clone();
    let ordinal = h.next_ordinal();
    let volatile = h.volatile_bytes();
    let salvaged = do_salvage(&h);
    assert_eq!(h.next_ordinal(), ordinal, "{label}: salvage wrote");
    assert_eq!(
        h.volatile_bytes(),
        volatile,
        "{label}: salvage changed bytes"
    );
    let opened = do_writable(&h);
    let fail = |msg: String| -> ! { panic!("{label} {policy:?}: {msg}\n{}", h.report("")) };

    let key = match (&salvaged, &opened) {
        (
            Err(CowError::NoValidRoot { slots: a }),
            Err(CowError::NoValidRoot { slots: b } | CowError::IncompleteCreate { slots: b }),
        ) => {
            if exp.prev.is_some() {
                fail("no valid root although a committed state existed".into());
            }
            assert_eq!(a, b);
            if matches!(opened, Err(CowError::IncompleteCreate { .. })) {
                "IncompleteCreate".to_string()
            } else {
                "NoRoot".to_string()
            }
        }
        (Ok((sg, ss, sr)), Ok((wg, ws, wr))) => {
            if (sg, ss, sr.rolled_forward) != (wg, ws, wr.rolled_forward) {
                fail(format!("salvage ({sg}) and writable ({wg}) disagree"));
            }
            if sr.possible_lost_commit || wr.possible_lost_commit {
                fail(format!("possible_lost_commit {sr:?} {wr:?}"));
            }
            if wr.chosen_slot != (*wg & 1) as usize {
                fail(format!("report slot {wr:?} for generation {wg}"));
            }
            let side = if (*wg, ws) == exp.target {
                "target"
            } else if Some((*wg, ws)) == exp.prev {
                "prev"
            } else {
                fail(format!(
                    "state generation {wg} ({} blocks) is neither previous {:?} nor target",
                    ws.len(),
                    exp.prev.map(|p| p.0)
                ))
            };
            let class = wr.class();
            if class == OpenClass::UnfinishedCandidate
                && !matches!(wr.other_slot, SlotState::Valid { .. })
            {
                fail(format!(
                    "UnfinishedCandidate with a {:?} slot",
                    wr.other_slot
                ));
            }
            let class_ok = match side {
                "target" => matches!(class, OpenClass::Committed | OpenClass::RolledForward),
                _ => matches!(class, OpenClass::Committed | OpenClass::UnfinishedCandidate),
            };
            if !class_ok {
                fail(format!("class {class:?} for {side}"));
            }
            format!("{side}:{class:?}")
        }
        _ => fail(format!(
            "salvage and writable disagree: {:?} / {:?}",
            salvaged.as_ref().map(|r| r.0),
            opened.as_ref().map(|r| r.0)
        )),
    };

    // A recovered file keeps working: one more commit, one more crash.
    if let Ok((g, snap, _)) = &opened {
        let (mut file, _) = writable(&h).expect("second open");
        let mut rng = SplitMix64(*g);
        let more = rows(snap.len(), &mut rng);
        let refs: Vec<(u64, &[u8])> = more.iter().map(|(i, d)| (*i, d.as_slice())).collect();
        file.commit(&refs).expect("commit after recovery");
        assert_eq!(file.generation(), g + 1);
        drop(file);
        h.crash(CrashPolicy::DropPending);
        let (g2, snap2, _) = do_writable(&h).expect("reopen");
        assert_eq!(g2, g + 1, "{label}");
        let mut expect_snap = snap.clone();
        for (i, d) in &more {
            if (*i as usize) < expect_snap.len() {
                expect_snap[*i as usize] = d.clone();
            } else {
                expect_snap.push(d.clone());
            }
        }
        assert_eq!(snap2, expect_snap, "{label}");
    }
    key
}

/// Every step of `create`, every fault kind at that step, every crash policy:
/// only no root, an unfinished create, or the clean empty generation 2. An
/// acknowledged create always opens; nothing else is ever visible.
#[test]
fn power_cut_at_every_create_step() {
    let empty: Snap = Vec::new();
    let exp = Expected {
        prev: None,
        target: (2, &empty),
    };
    let ops = [OpKind::SetLen, OpKind::Write, OpKind::Write, OpKind::Flush];
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    for (k, kind) in ops.iter().enumerate() {
        for fault in faults_for(*kind) {
            for policy in policies() {
                let mut sim = Sim::new(0xC000 + k as u64, CommitOrder::Correct);
                let acked = sim.create(Some((k as u64, fault)));
                assert_eq!(acked, !is_failing(fault), "#{k} {fault:?}");
                let label = format!("create #{k} {fault:?}");
                let key = crash_and_check(&mut sim, policy, exp, &label);
                if acked {
                    assert_eq!(key, "target:Committed", "{label}: acked create must open");
                }
                if k == 0 {
                    assert_eq!(key, "NoRoot", "{label}: no root before the first write");
                }
                *seen.entry(key).or_default() += 1;
            }
        }
    }
    for needed in ["NoRoot", "IncompleteCreate", "target:Committed"] {
        assert!(seen.contains_key(needed), "never saw {needed}: {seen:?}");
    }
    assert_eq!(seen.len(), 3, "unexpected outcome: {seen:?}");
}

/// Every step of publication generations 3..=6 (both slot parities; 3 is the
/// first commit after `create`), every
/// fault kind, every crash policy: open yields exactly the previous state or
/// exactly the target, with no mixture, no possible-loss flag, and a working
/// file afterwards.
#[test]
fn power_cut_at_every_publication_step() {
    let ops = tx_ops(CommitOrder::Correct, 2);
    let b1 = first_flush(&ops);
    let b2 = b1 + 2;
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    let mut first_commit: BTreeMap<String, u32> = BTreeMap::new();
    let mut cases = 0u32;
    for prior in 0..4u64 {
        for (k, kind) in ops.iter().enumerate() {
            for fault in faults_for(*kind) {
                for policy in policies() {
                    cases += 1;
                    let seed = 0xD000 + cases as u64;
                    let mut sim = Sim::new(seed, CommitOrder::Correct);
                    let mut rng = SplitMix64(seed);
                    assert!(sim.create(None));
                    for _ in 0..prior {
                        let w = rows(sim.oracle.base().len(), &mut rng);
                        assert!(sim.commit(&w, None));
                    }
                    let base = sim.oracle.base();
                    let writes = rows(base.len(), &mut rng);
                    let target = sim.oracle.target_after(&writes);
                    let generation = prior + 2; // create leaves generation 2
                    let acked = sim.commit(&writes, Some((k as u64, fault)));
                    assert_eq!(acked, !is_failing(fault), "#{k} {fault:?}");
                    let exp = Expected {
                        prev: Some((generation, &base)),
                        target: (generation + 1, &target),
                    };
                    let label = format!("publish gen {} #{k} {fault:?}", generation + 1);
                    let key = crash_and_check(&mut sim, policy, exp, &label);
                    if acked {
                        assert_eq!(key, "target:Committed", "{label}: ack lost");
                    } else if k <= b1 && prior > 0 {
                        assert!(key.starts_with("prev:"), "{label}: root not written yet");
                    } else if k == b2 && fault == Fault::Flush(FlushFault::IoAfterDurable) {
                        assert!(key.starts_with("target:"), "{label}: lost ack, durable");
                    }
                    if prior == 0 {
                        *first_commit.entry(key.clone()).or_default() += 1;
                    }
                    *seen.entry(key).or_default() += 1;
                }
            }
        }
    }
    // The first commit (generation 3) has the same three outcomes as any
    // other: generation 2 with an ignored tail, roll-forward, or generation 3.
    for needed in [
        "prev:UnfinishedCandidate",
        "target:RolledForward",
        "target:Committed",
    ] {
        assert!(
            first_commit.contains_key(needed),
            "first commit never gave {needed}: {first_commit:?}"
        );
    }
    for needed in [
        "prev:Committed",
        "prev:UnfinishedCandidate",
        "target:Committed",
        "target:RolledForward",
    ] {
        assert!(seen.contains_key(needed), "never saw {needed}: {seen:?}");
    }
    assert_eq!(seen.len(), 4, "unexpected outcome: {seen:?}");
    eprintln!("publication cut cases: {cases} {seen:?}");
}

// ------------------------------------------------------- damaged carriers

#[derive(Clone, Copy, Debug)]
enum Damage {
    Flip(usize),
    Zero,
}

fn damage(bytes: &mut [u8], at: usize, how: Damage) {
    match how {
        Damage::Flip(byte) => bytes[at + byte] ^= 0x01,
        Damage::Zero => bytes[at..at + SLOT_SIZE].fill(0),
    }
}

const DAMAGES: [Damage; 5] = [
    Damage::Flip(0),
    Damage::Flip(20),
    Damage::Flip(100),
    Damage::Flip(SLOT_SIZE - 1),
    Damage::Zero,
];

/// A committed generation `g` has three carriers of its root: slot `g`, slot
/// `g - 1` (the previous root) and the root copy in the last sector. Damage
/// every subset, in every way, for generations of both parities.
#[test]
fn damaged_root_carriers_of_a_committed_state() {
    for txs in 1..=4u64 {
        let g = txs + 2; // create leaves generation 2
        let (sim, snaps) = history(0xA0 + txs, txs);
        let original = sim.handle.durable_bytes();
        let at = [
            slot_offset(g) as usize,
            slot_offset(g - 1) as usize,
            original.len() - SLOT_SIZE,
        ];
        for subset in 0..8u32 {
            let (dg, dp, dc) = (subset & 1 != 0, subset & 2 != 0, subset & 4 != 0);
            // (writable, salvage) per subset of damaged carriers.
            let (w, s) = match (dg, dp, dc) {
                (false, false, _) => (
                    Want::Open(g, OpenClass::Committed),
                    Want::Open(g, OpenClass::Committed),
                ),
                // Newest slot lost, previous root intact, copy intact: the
                // copy explains the damage.
                (true, false, false) => (
                    Want::Open(g, OpenClass::RolledForward),
                    Want::Open(g, OpenClass::RolledForward),
                ),
                // Newest slot lost and nothing explains it: latest cannot be
                // established; salvage shows the previous root, flagged.
                (true, false, true) => (
                    Want::DamagedOther(g - 1),
                    Want::Open(g - 1, OpenClass::PossibleLostCommit),
                ),
                // Previous root damaged: unexplained (a copy of `g` is not a
                // candidate for `g + 1`).
                (false, true, _) => (
                    Want::DamagedOther(g),
                    Want::Open(g, OpenClass::PossibleLostCommit),
                ),
                // Both slots damaged: no root, whatever happened to the copy.
                (true, true, _) => (Want::NoRoot, Want::NoRoot),
            };
            for &how in &DAMAGES {
                let mut bytes = original.clone();
                for (flag, &pos) in [dg, dp, dc].iter().zip(&at) {
                    if *flag {
                        damage(&mut bytes, pos, how);
                    }
                }
                let label = format!("G={g} damaged(slot g:{dg} slot g-1:{dp} copy:{dc}) {how:?}");
                let after = check_open(&label, &bytes, &snaps, w, s);
                if matches!(w, Want::Open(_, OpenClass::RolledForward)) {
                    assert_eq!(after, original, "{label}: slot not repaired on media");
                }
            }
        }
    }
}

/// A complete candidate whose root was not published, or whose root write was
/// torn, next to a committed state of generation `g`: the candidate may be
/// adopted only when the previous committed root is intact.
#[test]
fn torn_inactive_root_needs_an_intact_previous_root() {
    let ops = tx_ops(CommitOrder::Correct, 2);
    let root_write = first_flush(&ops) + 1;
    for prior in 0..4u64 {
        let g = prior + 2; // committed generation; candidate is g + 1
        for torn in [false, true] {
            let seed = 0xE000 + prior * 2 + u64::from(torn);
            let mut sim = Sim::new(seed, CommitOrder::Correct);
            let mut rng = SplitMix64(seed);
            assert!(sim.create(None));
            let mut snaps: Vec<Snap> = vec![Vec::new()];
            for _ in 0..prior {
                let w = rows(sim.oracle.base().len(), &mut rng);
                assert!(sim.commit(&w, None));
                snaps.push(sim.oracle.base());
            }
            let writes = rows(sim.oracle.base().len(), &mut rng);
            snaps.push(sim.oracle.target_after(&writes));
            let (fault, policy) = if torn {
                (
                    Fault::Write(WriteFault::TornIo { written: 100 }),
                    CrashPolicy::KeepPending,
                )
            } else {
                (
                    Fault::Write(WriteFault::Fail(std::io::ErrorKind::Other)),
                    CrashPolicy::DropPending,
                )
            };
            assert!(!sim.commit(&writes, Some((root_write as u64, fault))));
            sim.file = None;
            sim.handle.crash(policy);
            let media = sim.handle.durable_bytes();
            let at = [
                slot_offset(g) as usize,     // previous committed root
                slot_offset(g + 1) as usize, // inactive slot of the candidate
                media.len() - SLOT_SIZE,     // root copy of the candidate
            ];
            let t = if torn { "torn" } else { "unwritten" };

            // Intact previous root: the candidate is adopted (torn) or ignored
            // (unwritten), never anything else, for the first commit too.
            let (w, s) = if torn {
                (
                    Want::Open(g + 1, OpenClass::RolledForward),
                    Want::Open(g + 1, OpenClass::RolledForward),
                )
            } else {
                (
                    Want::Open(g, OpenClass::UnfinishedCandidate),
                    Want::Open(g, OpenClass::UnfinishedCandidate),
                )
            };
            let label = format!("{t} root, previous intact, G={g}");
            let after = check_open(&label, &media, &snaps, w, s);
            if torn {
                // Repaired: the media now open as a plain committed state.
                check_open(
                    &format!("{label} (after repair)"),
                    &after,
                    &snaps,
                    Want::Open(g + 1, OpenClass::Committed),
                    Want::Open(g + 1, OpenClass::Committed),
                );
            }

            // The copy is not needed while the inactive slot is intact, and
            // fatal to a torn root when it is gone too.
            let mut bytes = media.clone();
            damage(&mut bytes, at[2], Damage::Flip(20));
            let (w, s) = if torn {
                (
                    Want::DamagedOther(g),
                    Want::Open(g, OpenClass::PossibleLostCommit),
                )
            } else {
                (
                    Want::Open(g, OpenClass::UnfinishedCandidate),
                    Want::Open(g, OpenClass::UnfinishedCandidate),
                )
            };
            check_open(
                &format!("{t} root, copy damaged, G={g}"),
                &bytes,
                &snaps,
                w,
                s,
            );

            // Previous committed root damaged as well: a torn candidate is no
            // longer explained by the model.
            let mut bytes = media.clone();
            damage(&mut bytes, at[0], Damage::Flip(20));
            let (w, s) = if torn {
                (Want::NoRoot, Want::NoRoot)
            } else {
                // Generation `g - 1` survives in the inactive slot; nothing
                // explains why `g` is damaged.
                (
                    Want::DamagedOther(g - 1),
                    Want::Open(g - 1, OpenClass::PossibleLostCommit),
                )
            };
            check_open(
                &format!("{t} root, previous damaged, G={g}"),
                &bytes,
                &snaps,
                w,
                s,
            );
        }
    }
}

// -------------------------------------------------- generation selection

#[test]
fn same_generation_in_both_slots_is_a_conflict_not_a_choice() {
    let refused = |label: &str, bytes: &[u8], generation: u64| {
        let h = FaultHandle::new(bytes, 7);
        for result in [do_writable(&h), do_salvage(&h)] {
            assert!(
                matches!(
                    result,
                    Err(CowError::GenerationConflict { generation: g }) if g == generation
                ),
                "{label}: {:?}",
                result.map(|r| r.0)
            );
        }
        assert_eq!(h.next_ordinal(), 0, "{label}: refused open must not write");
    };
    let (sim, _) = history(0xB1, 2); // generations 2..=4: slot 0 = 4, slot 1 = 3
    let bytes = sim.handle.durable_bytes();
    let (s0, s1) = (
        bytes[..SLOT_SIZE].to_vec(),
        bytes[SLOT_SIZE..2 * SLOT_SIZE].to_vec(),
    );

    // The newest root duplicated into the other slot.
    let b = with_bytes(&sim, |b| b[SLOT_SIZE..2 * SLOT_SIZE].copy_from_slice(&s0));
    refused("newest in both", &b.durable_bytes(), 4);
    // The previous root duplicated over the newest.
    let b = with_bytes(&sim, |b| b[..SLOT_SIZE].copy_from_slice(&s1));
    refused("previous in both", &b.durable_bytes(), 3);

    // Forged: two roots of generation 2, same CRC-valid content.
    let crc0 = crc32(&[]);
    let root = forge_root_sector(2, 0, HEADER_LEN, crc0, HEADER_LEN);
    let mut img = vec![0u8; HEADER_LEN as usize];
    img[..SLOT_SIZE].copy_from_slice(&root);
    img[SLOT_SIZE..].copy_from_slice(&root);
    refused("forged 2/2", &img, 2);

    // A CRC-valid root in a slot that is not its own is refused even without
    // a twin: it is a misdirected write, not a damaged slot.
    let other = forge_root_sector(6, 0, HEADER_LEN, crc0, HEADER_LEN);
    let b = with_bytes(&sim, |b| {
        b[SLOT_SIZE..2 * SLOT_SIZE].copy_from_slice(&other)
    });
    let h = FaultHandle::new(&b.durable_bytes(), 8);
    for result in [do_writable(&h), do_salvage(&h)] {
        assert!(
            matches!(result, Err(CowError::RootInconsistent(_))),
            "{:?}",
            result.map(|r| r.0)
        );
    }
    // Generation zero is never a root.
    let zero = forge_root_sector(0, 0, HEADER_LEN, crc0, HEADER_LEN);
    let b = with_bytes(&sim, |b| b[..SLOT_SIZE].copy_from_slice(&zero));
    assert!(matches!(
        do_writable(&FaultHandle::new(&b.durable_bytes(), 9)),
        Err(CowError::RootInconsistent("generation zero"))
    ));
}

#[test]
fn generation_overflow_is_refused_and_never_falls_back() {
    let crc0 = crc32(&[]);
    let root = |g: u64| forge_root_sector(g, 0, HEADER_LEN, crc0, HEADER_LEN);

    // u64::MAX cannot be incremented: a root of that generation is refused,
    // and so is the open; the valid MAX - 1 next to it is not a fallback.
    let img = forged(HEADER_LEN as usize, &[root(u64::MAX), root(u64::MAX - 1)]);
    let h = FaultHandle::new(&img, 1);
    for result in [do_writable(&h), do_salvage(&h)] {
        assert!(
            matches!(
                result,
                Err(CowError::RootInconsistent("generation at maximum"))
            ),
            "{:?}",
            result.map(|r| r.0)
        );
    }
    assert_eq!(h.next_ordinal(), 0);

    // The last usable generation is MAX - 1: commits reach it and the next
    // one is rejected before any I/O; a reopen at MAX - 1 behaves the same.
    let img = forged(
        HEADER_LEN as usize,
        &[root(u64::MAX - 3), root(u64::MAX - 4)],
    );
    let h = FaultHandle::new(&img, 2);
    let (mut file, report) = writable(&h).expect("open at MAX - 3");
    assert_eq!(report.generation, u64::MAX - 3);
    let data = vec![9u8; BLOCK_SIZE];
    file.commit(&[(0, data.as_slice())]).expect("MAX - 2");
    file.commit(&[(0, data.as_slice())]).expect("MAX - 1");
    assert_eq!(file.generation(), u64::MAX - 1);
    drop(file);
    h.crash(CrashPolicy::DropPending);
    let (mut file, report) = writable(&h).expect("reopen at MAX - 1");
    assert_eq!(report.generation, u64::MAX - 1);
    let ordinal = h.next_ordinal();
    assert!(matches!(
        file.commit(&[(0, data.as_slice())]),
        Err(CowError::RootInconsistent("generation exhausted"))
    ));
    assert_eq!(h.next_ordinal(), ordinal);
}

// ---------------------------------------- reachable metadata and data

/// Rewrites the newest root of `sim`'s media with a map edited by `edit`
/// (map CRC recomputed so the map stays CRC-valid), leaving the previous slot
/// untouched.
fn with_edited_map(
    sim: &Sim,
    edit: impl FnOnce(&mut Vec<u8>),
    block_count: Option<u64>,
) -> Vec<u8> {
    let mut bytes = sim.handle.durable_bytes();
    let (file, _) = writable(&with_bytes(sim, |_| {})).expect("clean open");
    let (g, count, map_at) = (
        file.generation(),
        file.block_count(),
        file.map_offset() as usize,
    );
    let end = file.end_offset();
    drop(file);
    let mut map = bytes[map_at..map_at + count as usize * 12].to_vec();
    edit(&mut map);
    bytes[map_at..map_at + map.len()].copy_from_slice(&map);
    let root = forge_root_sector(
        g,
        block_count.unwrap_or(count),
        map_at as u64,
        crc32(&map),
        end,
    );
    let at = slot_offset(g) as usize;
    bytes[at..at + SLOT_SIZE].copy_from_slice(&root);
    bytes
}

/// A structurally valid newest header over an invalid map is refused by both
/// opens; the intact previous root is never used instead.
#[test]
fn valid_header_with_invalid_map_is_refused() {
    let (sim, _) = history(0xE5, 3);
    let refuse = |label: &str, bytes: &[u8], pred: fn(&CowError) -> bool| {
        let h = FaultHandle::new(bytes, 11);
        for (who, result) in [("writable", do_writable(&h)), ("salvage", do_salvage(&h))] {
            let err = result
                .err()
                .unwrap_or_else(|| panic!("{label} [{who}]: opened"));
            assert!(pred(&err), "{label} [{who}]: {err:?}");
        }
        assert_eq!(h.next_ordinal(), 0, "{label}: refused open must not write");
    };
    // The previous slot is intact in every image below.
    let intact = do_writable(&with_bytes(&sim, |_| {})).expect("clean").0;
    assert_eq!(intact, 5);

    // Map bytes changed without updating the root's map CRC.
    let mut bytes = sim.handle.durable_bytes();
    let (file, _) = writable(&with_bytes(&sim, |_| {})).expect("clean");
    bytes[file.map_offset() as usize + 5] ^= 0x10;
    drop(file);
    refuse("map crc mismatch", &bytes, |e| {
        matches!(e, CowError::MapCorrupt)
    });

    // Map CRC-valid, entries hostile.
    refuse(
        "entry unaligned",
        &with_edited_map(
            &sim,
            |m| m[0..8].copy_from_slice(&(HEADER_LEN + 1).to_le_bytes()),
            None,
        ),
        |e| matches!(e, CowError::MapEntryOutOfRange { index: 0 }),
    );
    refuse(
        "entry inside header",
        &with_edited_map(
            &sim,
            |m| m[12..20].copy_from_slice(&0u64.to_le_bytes()),
            None,
        ),
        |e| matches!(e, CowError::MapEntryOutOfRange { index: 1 }),
    );
    refuse(
        "entry beyond file",
        &with_edited_map(
            &sim,
            |m| m[0..8].copy_from_slice(&(1u64 << 40).to_le_bytes()),
            None,
        ),
        |e| matches!(e, CowError::MapEntryOutOfRange { index: 0 }),
    );
    refuse(
        "two entries name one block",
        &with_edited_map(
            &sim,
            |m| {
                let first: Vec<u8> = m[0..8].to_vec();
                m[12..20].copy_from_slice(&first);
            },
            None,
        ),
        |e| matches!(e, CowError::MapEntryCollision { index: 1 }),
    );
    // Header claims more blocks than the map has, fewer than it has (the map
    // CRC no longer matches), or more than the limit.
    refuse(
        "block_count above limit",
        &with_edited_map(&sim, |_| {}, Some((1 << 16) + 1)),
        |e| matches!(e, CowError::RootInconsistent(_)),
    );
    refuse(
        "block_count too small for map crc",
        &with_edited_map(&sim, |_| {}, Some(2)),
        |e| matches!(e, CowError::MapCorrupt),
    );
    refuse(
        "block_count too large for file",
        &with_edited_map(&sim, |_| {}, Some(1 << 15)),
        |e| matches!(e, CowError::RootInconsistent(_)),
    );
}

/// The newest root is checksum-valid and so is its map, but a block it
/// references is damaged. No fallback to the older root, no zero-fill; damage
/// to a block that only an older generation referenced is not reachable and
/// does not matter.
#[test]
fn checksum_valid_newest_root_with_damaged_block_does_not_fall_back() {
    let (sim, snaps) = history(0xB7, 3);
    let newest = snaps.last().cloned().expect("history");
    let (file, _) = writable(&with_bytes(&sim, |_| {})).expect("clean");
    let referenced: Vec<u64> = (0..file.block_count())
        .map(|i| file.block_offset(i).expect("block"))
        .collect();
    drop(file);

    // The same history one transaction shorter: blocks the older root used.
    let (older_sim, _) = history(0xB7, 2);
    let (older, _) = writable(&with_bytes(&older_sim, |_| {})).expect("older");
    let older_only: Vec<u64> = (0..older.block_count())
        .map(|i| older.block_offset(i).expect("block"))
        .filter(|p| !referenced.contains(p))
        .collect();
    drop(older);
    assert!(!older_only.is_empty(), "history must overwrite a block");

    for (index, &phys) in referenced.iter().enumerate() {
        let broken = with_bytes(&sim, |b| b[phys as usize + 17] ^= 0x40);
        let h = FaultHandle::new(&broken.durable_bytes(), 12);
        for (who, result) in [("writable", do_writable(&h)), ("salvage", do_salvage(&h))] {
            assert!(
                matches!(
                    &result,
                    Err(CowError::BlockCorrupt { index: i }) if *i == index as u64
                ),
                "block {index} [{who}]: {:?}",
                result.map(|r| r.0)
            );
        }
        assert_eq!(h.next_ordinal(), 0);
    }
    for &phys in &older_only {
        let broken = with_bytes(&sim, |b| b[phys as usize + 17] ^= 0x40);
        let (g, snap, report) = do_writable(&FaultHandle::new(&broken.durable_bytes(), 13))
            .expect("unreachable damage is irrelevant");
        assert_eq!((g, snap), (5, newest.clone()));
        assert_eq!(report.class(), OpenClass::Committed);
    }
}

// ------------------------------------------------------- no usable root

#[test]
fn no_usable_root_is_a_refusal_in_every_form() {
    let (sim, snaps) = history(0x4E, 2);
    let original = sim.handle.durable_bytes();
    let cases: Vec<(&str, Mutation)> = vec![
        ("empty file", Box::new(|b| b.clear())),
        ("shorter than header", Box::new(|b| b.truncate(1023))),
        ("blank header", Box::new(|b| b[..1024].fill(0))),
        (
            "both slots junk",
            Box::new(|b| b[..1024].copy_from_slice(&SplitMix64(5).bytes(1024))),
        ),
        (
            "both slots bit-rotted, copy intact",
            Box::new(|b| {
                b[20] ^= 1;
                b[SLOT_SIZE + 20] ^= 1;
            }),
        ),
    ];
    for (label, mutate) in &cases {
        let mut bytes = original.clone();
        mutate(&mut bytes);
        check_open(label, &bytes, &snaps, Want::NoRoot, Want::NoRoot);
    }
    // A pristine file is the control.
    check_open(
        "control",
        &original,
        &snaps,
        Want::Open(4, OpenClass::Committed),
        Want::Open(4, OpenClass::Committed),
    );
}

// ------------------------------------------------ writable vs read-only

fn set_slot_fields(bytes: &mut [u8], at: usize, edit: impl FnOnce(&mut [u8])) {
    edit(&mut bytes[at..at + SLOT_SIZE]);
    let crc = crc32(&bytes[at..at + SLOT_SIZE - 4]);
    bytes[at + SLOT_SIZE - 4..at + SLOT_SIZE].copy_from_slice(&crc.to_le_bytes());
}

#[test]
fn mandatory_features_are_checked_per_access_mode() {
    let (sim, _) = history(0xF3, 2); // generations 2..=4, newest in slot 0
    let newest = slot_offset(4) as usize;
    let older = slot_offset(3) as usize;
    let set = |bytes: &mut Vec<u8>, at: usize, mandatory: u64, optional: u64| {
        set_slot_fields(bytes, at, |s| {
            s[52..60].copy_from_slice(&mandatory.to_le_bytes());
            s[60..68].copy_from_slice(&optional.to_le_bytes());
        })
    };
    let open_with = |h: &FaultHandle, caps: Capabilities| {
        CowFile::open_with(CheckedStorage::new(h.open()).expect("caps"), &caps)
    };
    let salvage_with = |h: &FaultHandle, caps: Capabilities| {
        CowView::salvage_with(CheckedStorage::new(h.open()).expect("caps"), &caps)
    };
    let cap = |read: u64, write: u64| Capabilities {
        read_mandatory: read,
        write_mandatory: write,
    };

    let mut bytes = sim.handle.durable_bytes();
    set(&mut bytes, newest, 1 << 3, 0);
    let h = FaultHandle::new(&bytes, 1);
    // This build knows no features: neither access mode opens it.
    assert!(matches!(
        do_writable(&h),
        Err(CowError::UnsupportedMandatoryFeature(3))
    ));
    assert!(matches!(
        do_salvage(&h),
        Err(CowError::UnsupportedMandatoryFeature(3))
    ));
    // Understood but not writable: read-only works, writable refuses.
    assert!(salvage_with(&h, cap(1 << 3, 0)).is_ok());
    assert!(matches!(
        open_with(&h, cap(1 << 3, 0)),
        Err(CowError::UnsupportedMandatoryFeature(3))
    ));
    // Writable only counts bits that are also readable.
    assert!(matches!(
        open_with(&h, cap(0, 1 << 3)),
        Err(CowError::UnsupportedMandatoryFeature(3))
    ));
    assert_eq!(h.next_ordinal(), 0, "refusals and salvage write nothing");
    // Writable: opens, and the next commit carries the bit forward, so a
    // build that cannot write it refuses the result.
    let (mut file, _) = open_with(&h, cap(1 << 3, 1 << 3)).expect("full caps");
    assert_eq!(
        file.features(),
        Features {
            mandatory: 1 << 3,
            optional: 0
        }
    );
    let data = vec![3u8; BLOCK_SIZE];
    file.commit(&[(0, data.as_slice())]).expect("commit");
    assert_eq!(file.features().mandatory, 1 << 3);
    drop(file);
    h.crash(CrashPolicy::DropPending);
    assert!(matches!(
        do_writable(&h),
        Err(CowError::UnsupportedMandatoryFeature(3))
    ));
    let (view, report) = salvage_with(&h, cap(1 << 3, 0)).expect("read-only");
    assert_eq!((view.generation(), report.generation), (5, 5));

    // The lowest offending bit is reported.
    let mut bytes = sim.handle.durable_bytes();
    set(&mut bytes, newest, (1 << 9) | (1 << 5), 0);
    assert!(matches!(
        do_writable(&FaultHandle::new(&bytes, 2)),
        Err(CowError::UnsupportedMandatoryFeature(5))
    ));

    // An unknown bit in the *older* root refuses too: no slot is skipped.
    let mut bytes = sim.handle.durable_bytes();
    set(&mut bytes, older, 1 << 40, 0);
    assert!(matches!(
        do_writable(&FaultHandle::new(&bytes, 3)),
        Err(CowError::UnsupportedMandatoryFeature(40))
    ));

    // Optional bits never block, in either mode.
    let mut bytes = sim.handle.durable_bytes();
    set(&mut bytes, newest, 0, u64::MAX);
    let h = FaultHandle::new(&bytes, 4);
    let (file, _) = writable(&h).expect("optional bits");
    assert_eq!(
        file.features(),
        Features {
            mandatory: 0,
            optional: u64::MAX
        }
    );
    assert!(do_salvage(&h).is_ok());
}

/// CRC-valid roots this build cannot or will not interpret are refused, not
/// treated as damage that a root copy could "repair".
#[test]
fn crc_valid_unusable_roots_are_refusals_not_damage() {
    let (sim, _) = history(0xF4, 3);
    let newest = slot_offset(5) as usize;
    let cases: Vec<(&str, SlotEdit, Pred)> = vec![
        (
            "version 2",
            Box::new(|s| s[8..12].copy_from_slice(&2u32.to_le_bytes())),
            |e| matches!(e, CowError::UnsupportedRoot(_)),
        ),
        (
            "block size 4096",
            Box::new(|s| s[12..16].copy_from_slice(&4096u32.to_le_bytes())),
            |e| matches!(e, CowError::UnsupportedRoot(_)),
        ),
        ("reserved byte set", Box::new(|s| s[100] = 1), |e| {
            matches!(e, CowError::RootInconsistent("reserved bytes not zero"))
        }),
    ];
    for (label, edit, pred) in cases {
        let mut bytes = sim.handle.durable_bytes();
        set_slot_fields(&mut bytes, newest, |s| edit(s));
        let h = FaultHandle::new(&bytes, 5);
        for (who, result) in [("writable", do_writable(&h)), ("salvage", do_salvage(&h))] {
            let err = result
                .err()
                .unwrap_or_else(|| panic!("{label} [{who}]: opened"));
            assert!(pred(&err), "{label} [{who}]: {err:?}");
        }
        assert_eq!(h.next_ordinal(), 0, "{label}");
    }
}

/// The scenario of ISSUE-057 under the two-root create: the first commit
/// (generation 3, slot 1) was acknowledged and its slot is then zeroed.
/// Intact root copy: roll-forward, repaired on media, and the next commit
/// keeps the acknowledged data. Copy damaged as well: writable refuses and
/// salvage shows generation 2 flagged as a possible lost commit. In no case
/// is a refused open allowed to write over the evidence (`check_open` compares
/// the bytes).
#[test]
fn zeroed_slot_of_the_first_commit_is_a_loss_or_a_roll_forward() {
    let (sim, snaps) = history(0x61, 1); // generations 1, 2 (create), 3
    let original = sim.handle.durable_bytes();
    let copy_at = original.len() - SLOT_SIZE;
    let slot3 = slot_offset(3) as usize;

    let mut bytes = original.clone();
    damage(&mut bytes, slot3, Damage::Zero);
    let after = check_open(
        "zeroed slot, copy intact",
        &bytes,
        &snaps,
        Want::Open(3, OpenClass::RolledForward),
        Want::Open(3, OpenClass::RolledForward),
    );
    assert_eq!(after, original);
    let h = FaultHandle::new(&bytes, 21);
    let (mut file, report) = writable(&h).expect("open");
    assert!(report.rolled_forward);
    let data = vec![0x5Au8; BLOCK_SIZE];
    file.commit(&[(0, data.as_slice())]).expect("commit");
    let snap = file.snapshot().expect("snapshot");
    assert_eq!(snap.first(), Some(&data));
    assert_eq!(
        snap.get(1),
        snaps[1].get(1),
        "acknowledged block 1 survives"
    );
    assert_eq!(snap.len(), snaps[1].len());

    for how in DAMAGES {
        let mut bytes = original.clone();
        damage(&mut bytes, slot3, Damage::Zero);
        damage(&mut bytes, copy_at, how);
        check_open(
            &format!("zeroed slot, copy {how:?}"),
            &bytes,
            &snaps,
            Want::DamagedOther(2),
            Want::Open(2, OpenClass::PossibleLostCommit),
        );
    }

    // The same zeroing of a freshly created file has nothing to lose: it is an
    // unfinished create, and neither mode shows a state.
    let (created, snaps0) = history(0x62, 0);
    let mut bytes = created.handle.durable_bytes();
    damage(&mut bytes, slot_offset(2) as usize, Damage::Zero);
    check_open(
        "zeroed slot of a created file",
        &bytes,
        &snaps0,
        Want::Incomplete,
        Want::NoRoot,
    );
}

/// For a confirmed generation `k` (2 is the created file), damage or zero any
/// one or two of the sectors that carry its root (slot 0, slot 1, the root
/// copy when the file has one). A writable open either refuses or returns a
/// state no older than `k`; a read-only open may show an older generation only
/// with `possible_lost_commit` set. Nothing the open returns is ever a mixture.
#[test]
fn damage_to_one_or_two_root_sectors_never_yields_an_older_writable_state() {
    let (mut refused, mut opened, mut flagged) = (0u32, 0u32, 0u32);
    for k in 2..=5u64 {
        let (sim, snaps) = history(0xD0 + k, k - 2);
        let original = sim.handle.durable_bytes();
        let mut carriers = vec![0usize, SLOT_SIZE];
        if original.len() > HEADER_LEN as usize {
            carriers.push(original.len() - SLOT_SIZE);
        }
        for mask in 1u32..(1 << carriers.len()) {
            if mask.count_ones() > 2 {
                continue;
            }
            for how in DAMAGES {
                let mut bytes = original.clone();
                for (i, &at) in carriers.iter().enumerate() {
                    if mask & (1 << i) != 0 {
                        damage(&mut bytes, at, how);
                    }
                }
                let label = format!("k={k} mask={mask:#b} {how:?}");
                let snap_ok =
                    |g: u64, snap: &Snap| snaps.get(g.saturating_sub(2) as usize) == Some(snap);

                let h = FaultHandle::new(&bytes, 31);
                if let Ok((g, snap, r)) = do_salvage(&h) {
                    assert!(snap_ok(g, &snap), "{label}: salvage state is a mixture");
                    if g < k {
                        assert!(r.possible_lost_commit, "{label}: silent older state");
                        flagged += 1;
                    }
                }
                assert_eq!(h.next_ordinal(), 0, "{label}: salvage wrote");
                assert_eq!(h.volatile_bytes(), bytes, "{label}: salvage changed bytes");

                let h = FaultHandle::new(&bytes, 32);
                match do_writable(&h) {
                    Ok((g, snap, r)) => {
                        assert!(g >= k, "{label}: writable open returned generation {g}");
                        assert!(snap_ok(g, &snap), "{label}: writable state is a mixture");
                        assert!(!r.possible_lost_commit, "{label}");
                        if r.class() == OpenClass::UnfinishedCandidate {
                            assert!(
                                matches!(r.other_slot, SlotState::Valid { .. }),
                                "{label}: UnfinishedCandidate with {:?}",
                                r.other_slot
                            );
                        }
                        opened += 1;
                    }
                    Err(_) => {
                        assert_eq!(h.next_ordinal(), 0, "{label}: refusal wrote");
                        assert_eq!(h.volatile_bytes(), bytes, "{label}: refusal changed bytes");
                        refused += 1;
                    }
                }
            }
        }
    }
    assert!(
        refused > 0 && opened > 0 && flagged > 0,
        "{refused}/{opened}/{flagged}"
    );
}
