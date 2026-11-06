//! Tests for [`crate::cow_salvage`] (S9-21): what is valid, lost, unknown and
//! conflicting under a damaged map, a damaged block, stale blocks with no
//! provenance, unordered roots and a missing parent; that nothing unprovable
//! reaches the copy; and that the source never changes.

use crate::cow_export::StateSelection;
use crate::cow_inspect::Outcome;
use crate::cow_proto::{BLOCK_SIZE, CowError, CowView, crc32};
use crate::cow_salvage::{
    CopyStatus, RangeClass, Recoverability, SalvageError, SalvageOptions, SalvageReport,
    salvage_to_copy,
};
use crate::cow_snapshot::{NoParents, ParentRef, ParentResolver, ResolveError, SnapFile};
use crate::io::CheckedStorage;
use crate::testing::fault::{
    CrashPolicy, Fault, FaultBackend, FaultHandle, FlushFault, WriteFault,
};
use crate::testing::rng::SplitMix64;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::io::ErrorKind;

const BS: usize = BLOCK_SIZE;

fn st(h: &FaultHandle) -> CheckedStorage<FaultBackend> {
    CheckedStorage::new(h.open()).expect("capabilities are consistent")
}

#[derive(Clone, Default)]
struct Model {
    bytes: Vec<u8>,
}

impl Model {
    fn write(&mut self, offset: usize, data: &[u8]) {
        let end = offset + data.len();
        let blocks = end.div_ceil(BS);
        if self.bytes.len() < blocks * BS {
            self.bytes.resize(blocks * BS, 0);
        }
        self.bytes[offset..end].copy_from_slice(data);
    }

    fn block(&self, b: usize) -> Option<&[u8]> {
        self.bytes.get(b * BS..(b + 1) * BS)
    }
}

/// A base image (uid 1) and the model of the head after every commit.
struct Base {
    h: FaultHandle,
    /// Model after each of the writes, newest last.
    history: Vec<Model>,
    /// Model at the moment of each snapshot.
    snaps: BTreeMap<u64, Model>,
    /// Image bytes right after write number 1 (generation 3).
    early_bytes: Vec<u8>,
}

/// Writes: A (blocks 0..2), B (block 6, a hole before it), snapshot 1,
/// overwrite of block 0 (so block 0 of snapshot 1 is stale in the head),
/// snapshot 2, C (block 9).
fn base(seed: u64) -> Base {
    let h = FaultHandle::new(&[], seed);
    let (mut f, _) = SnapFile::create_base(st(&h), 1).expect("create");
    let mut m = Model::default();
    let mut history = Vec::new();
    let mut snaps = BTreeMap::new();
    let mut put = |f: &mut SnapFile<FaultBackend>, m: &mut Model, at: usize, data: Vec<u8>| {
        f.write_at(at as u64, &data).expect("write");
        m.write(at, &data);
        history.push(m.clone());
    };
    put(&mut f, &mut m, 0, SplitMix64(seed).bytes(2 * BS));
    h.crash(CrashPolicy::DropPending);
    let early_bytes = h.durable_bytes();
    // The crash killed `f`'s backend; continue on a fresh writable open.
    drop(f);
    let (mut f, _) = SnapFile::open(st(&h), &mut NoParents).expect("reopen");
    put(&mut f, &mut m, 6 * BS + 10, SplitMix64(seed + 1).bytes(BS));
    let (id, _) = f.create_snapshot().expect("snapshot 1");
    snaps.insert(id, m.clone());
    put(&mut f, &mut m, 0, SplitMix64(seed + 2).bytes(BS));
    let (id, _) = f.create_snapshot().expect("snapshot 2");
    snaps.insert(id, m.clone());
    put(&mut f, &mut m, 9 * BS, SplitMix64(seed + 3).bytes(BS));
    drop(f);
    h.crash(CrashPolicy::DropPending);
    Base {
        h,
        history,
        snaps,
        early_bytes,
    }
}

impl Base {
    fn head(&self) -> &Model {
        self.history.last().expect("history")
    }
}

struct Run {
    report: SalvageReport,
    copy: Option<SnapFile<FaultBackend>>,
    dst: FaultHandle,
    /// `make_target` was called.
    requested: bool,
}

struct FsResolver<'a>(&'a BTreeMap<u64, FaultHandle>);

impl ParentResolver for FsResolver<'_> {
    type Backend = FaultBackend;

    fn open_parent(&mut self, p: &ParentRef) -> Result<CheckedStorage<FaultBackend>, ResolveError> {
        let h = self.0.get(&p.file_uid).ok_or(ResolveError::Missing)?;
        CheckedStorage::new(h.open()).map_err(ResolveError::Storage)
    }
}

/// Salvages `src` into a fresh empty target and proves the source did not
/// change (bytes and number of mutating calls).
fn salvage_with<R: ParentResolver>(
    src: &FaultHandle,
    sel: StateSelection,
    accept_partial: bool,
    resolver: &mut R,
) -> Run {
    let dst = FaultHandle::new(&[], 500);
    let (before, ord) = (src.volatile_bytes(), src.next_ordinal());
    let requested = Cell::new(false);
    let out = salvage_to_copy(
        st(src),
        resolver,
        &SalvageOptions {
            selection: sel,
            publish_partial: accept_partial,
        },
        4242,
        || {
            requested.set(true);
            Ok(st(&dst))
        },
    )
    .expect("analysis");
    assert_eq!(src.volatile_bytes(), before, "salvage changed the source");
    assert_eq!(src.durable_bytes(), before, "salvage changed the source");
    assert_eq!(
        src.next_ordinal(),
        ord,
        "salvage mutated the source backend"
    );
    Run {
        report: out.report,
        copy: out.copy,
        dst,
        requested: requested.get(),
    }
}

fn salvage(src: &FaultHandle, sel: StateSelection, accept_partial: bool) -> Run {
    salvage_with(src, sel, accept_partial, &mut NoParents)
}

/// Class and basis of every block, expanded from the extents.
fn per_block(r: &SalvageReport) -> Vec<(RangeClass, &'static str)> {
    let mut v = Vec::new();
    for e in &r.extents {
        assert_eq!(e.offset % BS as u64, 0);
        assert_eq!(e.len % BS as u64, 0);
        assert_eq!(e.offset, v.len() as u64 * BS as u64, "extents must tile");
        for _ in 0..e.len / BS as u64 {
            v.push((e.class, e.basis));
        }
    }
    assert_eq!(v.len() as u64 * BS as u64, r.logical_size.unwrap_or(0));
    v
}

fn copy_bytes(run: &Run) -> Vec<u8> {
    let copy = run.copy.as_ref().expect("published copy");
    let mut back = vec![0u8; copy.disk_size() as usize];
    copy.read_at(0, &mut back).expect("read copy");
    back
}

fn flip(h: &FaultHandle, at: u64) -> FaultHandle {
    let mut bytes = h.durable_bytes();
    bytes[at as usize] ^= 0x40;
    FaultHandle::new(&bytes, 77)
}

fn pristine_view(h: &FaultHandle) -> CowView<FaultBackend> {
    CowView::salvage(st(h)).expect("pristine").0
}

fn finding<'a>(r: &'a SalvageReport, subject: &str) -> Vec<(&'a str, Option<u64>)> {
    r.findings
        .iter()
        .filter(|f| f.subject == subject)
        .map(|f| (f.status, f.detail))
        .collect()
}

// -------------------------------------------------------------------- tests

#[test]
fn an_intact_image_is_complete_and_its_copy_equals_the_model() {
    let b = base(1);
    let run = salvage(&b.h, StateSelection::Head, false);
    let r = &run.report;
    assert_eq!(
        r.recoverability,
        Recoverability::Complete,
        "{}",
        r.render(64)
    );
    assert_eq!((r.reason, r.outcome()), ("ok", Outcome::Ok));
    assert!(!r.possible_lost_commit);
    assert_eq!(r.copy, CopyStatus::Published);
    assert_eq!(r.logical_size, Some(b.head().bytes.len() as u64));
    assert!(per_block(r).iter().all(|(c, _)| *c == RangeClass::Valid));
    // Holes read zero by the format and say so; stored blocks say CRC.
    let basis = per_block(r);
    assert_eq!(basis[3].1, "hole_zero");
    assert_eq!(basis[0].1, "crc_verified");
    assert_eq!(r.bytes(RangeClass::Lost) + r.bytes(RangeClass::Unknown), 0);
    assert!(r.snapshots.iter().all(|s| s.status == "intact"));
    assert_eq!(r.snapshots.len(), 2);
    // The copy is an ordinary, opening base image with the same bytes.
    assert_eq!(copy_bytes(&run), b.head().bytes);
    drop(run.copy);
    run.dst.crash(CrashPolicy::DropPending);
    let (img, _) = SnapFile::open(st(&run.dst), &mut NoParents).expect("copy opens");
    let mut back = vec![0u8; img.disk_size() as usize];
    img.read_at(0, &mut back).expect("read");
    assert_eq!(back, b.head().bytes);
    assert_eq!(img.file_uid(), 4242);
    // Every snapshot of the source can be salvaged on its own, too.
    for (id, model) in &b.snaps {
        let run = salvage(&b.h, StateSelection::Snapshot(*id), false);
        assert_eq!(run.report.recoverability, Recoverability::Complete, "{id}");
        assert_eq!(copy_bytes(&run), model.bytes, "snapshot {id}");
    }
}

#[test]
fn a_damaged_map_is_unrecoverable_with_a_listing_and_no_copy() {
    let b = base(2);
    let view = pristine_view(&b.h);
    let bad = flip(&b.h, view.map_offset() + 2);
    drop(view);
    let run = salvage(&bad, StateSelection::Head, true);
    let r = &run.report;
    assert_eq!(r.recoverability, Recoverability::Unrecoverable);
    assert_eq!(r.reason, "map_crc_mismatch");
    assert_eq!(r.outcome(), Outcome::Corrupt);
    assert!(r.extents.is_empty() && r.logical_size.is_none());
    assert_eq!(r.bytes(RangeClass::Valid), 0, "nothing declared recovered");
    // The listing says what was looked at: both roots are fine, the map of
    // the newest is not, and the older root's map is *not* used as a fallback.
    assert_eq!(finding(r, "slot0").len(), 1);
    assert_eq!(finding(r, "slot1").len(), 1);
    let statuses: Vec<_> = r
        .findings
        .iter()
        .filter(|f| f.subject.starts_with("slot"))
        .map(|f| f.status)
        .collect();
    assert_eq!(statuses, ["valid", "valid"]);
    assert_eq!(finding(r, "map")[0].0, "map_crc_mismatch");
    assert_eq!(r.copy, CopyStatus::NotCreated("unrecoverable"));
    assert!(!run.requested, "a target was created for nothing");
    assert!(run.copy.is_none());
    assert!(r.render(8).contains("recoverability=unrecoverable\n"));
}

#[test]
fn unreadable_metadata_of_every_kind_is_unrecoverable_never_zero_data() {
    let b = base(3);
    let view = pristine_view(&b.h);
    let header_at = view.block_offset(0).expect("header block");
    let presence_at = view.block_offset(1).expect("presence block");
    drop(view);
    let bytes = b.h.durable_bytes();
    let case = |name: &str, mutate: &dyn Fn(&mut Vec<u8>), reason: &str, outcome: Outcome| {
        let mut m = bytes.clone();
        mutate(&mut m);
        let h = FaultHandle::new(&m, 5);
        let run = salvage(&h, StateSelection::Head, true);
        let r = &run.report;
        assert_eq!(r.recoverability, Recoverability::Unrecoverable, "{name}");
        assert_eq!(r.reason, reason, "{name}");
        assert_eq!(r.outcome(), outcome, "{name}");
        assert_eq!(r.copy, CopyStatus::NotCreated("unrecoverable"), "{name}");
        assert!(!run.requested && run.copy.is_none(), "{name}");
        assert!(r.extents.is_empty(), "{name}");
    };
    case(
        "layer header block damaged",
        &|m| m[header_at as usize + 5] ^= 1,
        "layer_header_lost",
        Outcome::Corrupt,
    );
    case(
        "presence bitmap damaged",
        &|m| m[presence_at as usize + 1] ^= 1,
        "presence_lost",
        Outcome::Corrupt,
    );
    case(
        "no valid root slot",
        &|m| {
            m[..1024].fill(0xEE);
            let n = m.len();
            m[n - 512..].fill(0);
        },
        "no_valid_root",
        Outcome::Corrupt,
    );
    case(
        "foreign bytes",
        &|m| *m = vec![0xEE; 8192],
        "no_valid_root",
        Outcome::Corrupt,
    );
    case(
        "a file shorter than the root slots",
        &|m| m.truncate(300),
        "no_valid_root",
        Outcome::Corrupt,
    );
    case(
        "unknown mandatory feature",
        &|m| {
            for s in 0..2 {
                let at = s * 512;
                m[at + 52..at + 60].copy_from_slice(&(1u64 << 40).to_le_bytes());
                let crc = crc32(&m[at..at + 508]);
                m[at + 508..at + 512].copy_from_slice(&crc.to_le_bytes());
            }
        },
        "unsupported_mandatory_feature",
        Outcome::Unsupported,
    );
}

#[test]
fn a_damaged_head_block_is_lost_and_the_stale_older_version_is_not_used() {
    let b = base(4);
    let view = pristine_view(&b.h);
    // Disk block 0 was overwritten after snapshots 1 (and 2 saw the new one):
    // the head's block 0 is the newest version, snapshot 1 holds the older.
    let newest_phys = view.block_offset(2).expect("head block 0");
    drop(view);
    let bad = flip(&b.h, newest_phys + 9);
    let stale = &b.snaps[&1];
    let head = b.head();
    assert_ne!(
        stale.block(0),
        head.block(0),
        "fixture needs a stale version"
    );

    // The normal read-only path refuses the whole file; salvage does not.
    assert!(matches!(
        CowView::salvage(st(&bad)),
        Err(CowError::BlockCorrupt { .. })
    ));
    let run = salvage(&bad, StateSelection::Head, false);
    let r = &run.report;
    assert_eq!(r.recoverability, Recoverability::Partial);
    assert_eq!(r.reason, "block_crc_mismatch");
    assert_eq!(r.outcome(), Outcome::Corrupt);
    let blocks = per_block(r);
    for (i, (class, basis)) in blocks.iter().enumerate() {
        if i == 0 {
            assert_eq!((*class, *basis), (RangeClass::Lost, "block_crc_mismatch"));
        } else {
            assert_eq!(*class, RangeClass::Valid, "block {i}");
        }
    }
    assert_eq!(r.bytes(RangeClass::Lost), BS as u64);
    // A partial recovery is not copied unless asked for: no target at all.
    assert_eq!(r.copy, CopyStatus::NotCreated("partial_not_accepted"));
    assert!(!run.requested && run.copy.is_none());

    // Asked for: the copy holds the verified blocks, and the lost block is
    // zeros, not the older version that still verifies in snapshot 1.
    let run = salvage(&bad, StateSelection::Head, true);
    assert_eq!(run.report.copy, CopyStatus::Published);
    let copy = copy_bytes(&run);
    assert_eq!(copy.len(), head.bytes.len());
    assert!(
        copy[..BS].iter().all(|x| *x == 0),
        "lost block must be zero"
    );
    assert_ne!(Some(&copy[..BS]), stale.block(0), "stale bytes leaked in");
    assert_ne!(Some(&copy[..BS]), head.block(0));
    assert_eq!(&copy[BS..], &head.bytes[BS..]);
}

#[test]
fn a_damaged_block_of_one_snapshot_is_lost_in_that_snapshot_only() {
    let b = base(5);
    let (f, _) = SnapFile::open(st(&b.h), &mut NoParents).expect("open");
    let only1: Vec<u64> = f
        .snapshot_data_blocks(1)
        .expect("snapshot 1")
        .difference(&f.snapshot_data_blocks(2).expect("snapshot 2"))
        .copied()
        .collect();
    drop(f);
    b.h.crash(CrashPolicy::DropPending);
    assert_eq!(only1.len(), 1, "exactly the overwritten block 0");
    let bad = flip(&b.h, only1[0] + 100);

    // Snapshot 1: block 0 is lost, the rest valid by range.
    let run = salvage(&bad, StateSelection::Snapshot(1), false);
    let r = &run.report;
    assert_eq!(r.recoverability, Recoverability::Partial);
    let blocks = per_block(r);
    assert_eq!(blocks[0], (RangeClass::Lost, "block_crc_mismatch"));
    assert!(blocks[1..].iter().all(|(c, _)| *c == RangeClass::Valid));
    assert_eq!(r.logical_size, Some(b.snaps[&1].bytes.len() as u64));

    // Snapshot 2 and the head do not reference it: complete. The report of
    // the head still says which snapshot is damaged.
    for sel in [StateSelection::Snapshot(2), StateSelection::Head] {
        let run = salvage(&bad, sel, false);
        assert_eq!(
            run.report.recoverability,
            Recoverability::Complete,
            "{sel:?}"
        );
        let snaps: Vec<_> = run
            .report
            .snapshots
            .iter()
            .map(|s| (s.id, s.status, s.lost_blocks))
            .collect();
        assert_eq!(snaps, [(1, "damaged", 1), (2, "intact", 0)], "{sel:?}");
    }
}

#[test]
fn unordered_roots_that_disagree_are_conflicting_and_neither_version_is_copied() {
    let b = base(6);
    // Put the early root (generation 4, after the first write) back into the
    // slot of the previous generation.
    let early = &b.early_bytes;
    let mut bytes = b.h.durable_bytes();
    let (newest_slot, other_slot) = {
        let v = pristine_view(&b.h);
        let g = v.generation();
        (((g & 1) as usize) * 512, (((g - 1) & 1) as usize) * 512)
    };
    assert_eq!(other_slot, 0, "the fixture expects slot 0 to be replaced");
    assert_ne!(newest_slot, other_slot);
    bytes[other_slot..other_slot + 512].copy_from_slice(&early[other_slot..other_slot + 512]);
    let h = FaultHandle::new(&bytes, 9);
    // The ordinary recovery has no answer (generations not consecutive).
    assert!(matches!(
        CowView::salvage(st(&h)),
        Err(CowError::RootInconsistent(_))
    ));
    let early_model = &b.history[0];
    let late_model = b.head();

    let run = salvage(&h, StateSelection::Head, false);
    let r = &run.report;
    assert_eq!(r.recoverability, Recoverability::Partial);
    assert_eq!(r.root_origin, "ambiguous");
    assert_eq!(r.root_generations.len(), 2);
    assert!(r.root_generations[0].abs_diff(r.root_generations[1]) > 1);
    assert_eq!(r.reason, "roots_disagree");
    let blocks = per_block(r);
    assert_eq!(blocks.len(), late_model.bytes.len() / BS);
    for (i, (class, basis)) in blocks.iter().enumerate() {
        let agree = early_model
            .block(i)
            .is_some_and(|e| Some(e) == late_model.block(i));
        let expect = if agree {
            (RangeClass::Valid, "agreed_by_both_roots")
        } else if early_model.block(i).is_none() {
            (RangeClass::Conflicting, "roots_disagree_on_size")
        } else {
            (RangeClass::Conflicting, "roots_disagree")
        };
        assert_eq!((*class, *basis), expect, "block {i}");
    }
    assert!(r.bytes(RangeClass::Conflicting) > 0);
    assert!(r.bytes(RangeClass::Valid) > 0, "agreed blocks stay valid");
    assert_eq!(r.copy, CopyStatus::NotCreated("partial_not_accepted"));
    assert!(!run.requested);

    // Accepted: agreed blocks are copied; a conflicting block is zero, it
    // holds neither of the two candidate versions.
    let run = salvage(&h, StateSelection::Head, true);
    let copy = copy_bytes(&run);
    for (i, (class, _)) in per_block(&run.report).iter().enumerate() {
        let got = &copy[i * BS..(i + 1) * BS];
        match class {
            RangeClass::Valid => assert_eq!(Some(got), late_model.block(i), "block {i}"),
            RangeClass::Conflicting => {
                assert!(got.iter().all(|x| *x == 0), "block {i} leaked data");
            }
            other => panic!("block {i}: {other:?}"),
        }
    }
}

#[test]
fn a_possible_lost_commit_is_an_older_root_and_says_so() {
    let b = base(7);
    let g = pristine_view(&b.h).generation();
    let slot = ((g & 1) as usize) * 512;
    let mut bytes = b.h.durable_bytes();
    bytes[slot..slot + 512].fill(0);

    // The copy of the newest root is intact: recovery rolls forward, nothing
    // is lost, and the origin says where the root came from.
    let torn = FaultHandle::new(&bytes, 10);
    let run = salvage(&torn, StateSelection::Head, false);
    let r = &run.report;
    assert_eq!(
        r.recoverability,
        Recoverability::Complete,
        "{}",
        r.render(8)
    );
    assert_eq!(r.root_origin, "root_copy");
    assert_eq!(copy_bytes(&run), b.head().bytes);

    // The copy is gone too: only the previous root remains.
    let n = bytes.len();
    bytes[n - 512..].fill(0);
    let lost = FaultHandle::new(&bytes, 11);
    let run = salvage(&lost, StateSelection::Head, false);
    let r = &run.report;
    assert_eq!(r.recoverability, Recoverability::Partial);
    assert!(r.possible_lost_commit);
    assert_eq!(r.reason, "possible_lost_commit");
    assert!(
        per_block(r)
            .iter()
            .all(|(c, basis)| *c == RangeClass::Valid && *basis != "crc_verified")
    );
    assert!(r.render(4).contains("possible_lost_commit=true\n"));
    assert!(!run.requested);
    // Accepted: the older state (before the last write), not the newest.
    let run = salvage(&lost, StateSelection::Head, true);
    let older = &b.history[b.history.len() - 2];
    assert_eq!(copy_bytes(&run), older.bytes);
    assert_ne!(older.bytes, b.head().bytes);
}

#[test]
fn a_child_without_its_parent_has_unknown_holes_not_zeros() {
    let b = base(8);
    let (f, _) = SnapFile::open(st(&b.h), &mut NoParents).expect("open");
    let pref = f.snapshot_ref(2).expect("ref");
    drop(f);
    b.h.crash(CrashPolicy::DropPending);
    let fs = BTreeMap::from([(1u64, b.h.clone())]);
    let hc = FaultHandle::new(&[], 20);
    let (mut c, _) =
        SnapFile::create_child(st(&hc), &mut FsResolver(&fs), &pref, 2).expect("child");
    let patch = SplitMix64(31).bytes(BS);
    c.write_at(2 * BS as u64, &patch).expect("write child");
    drop(c);
    hc.crash(CrashPolicy::DropPending);
    let mut model = b.snaps[&2].clone();
    model.write(2 * BS, &patch);

    // Parent unknown: only the child's own block is valid; everything the
    // parent would have shown is unknown, not zero.
    let run = salvage(&hc, StateSelection::Head, false);
    let r = &run.report;
    assert_eq!(r.recoverability, Recoverability::Partial);
    assert_eq!(r.reason, "parent_unavailable");
    let blocks = per_block(r);
    assert_eq!(blocks[2], (RangeClass::Valid, "crc_verified"));
    for (i, (class, basis)) in blocks.iter().enumerate() {
        if i != 2 {
            assert_eq!(
                (*class, *basis),
                (RangeClass::Unknown, "parent_unavailable"),
                "{i}"
            );
        }
    }
    assert_eq!(finding(r, "parent"), [("parent_missing", Some(1))]);
    assert!(!run.requested);

    // A different file under the recorded uid is not the parent either.
    let other = FaultHandle::new(&[], 21);
    let (mut o, _) = SnapFile::create_base(st(&other), 1).expect("other");
    o.write_at(0, &[9u8; 600]).expect("write");
    o.create_snapshot().expect("snapshot");
    drop(o);
    other.crash(CrashPolicy::DropPending);
    let wrong = BTreeMap::from([(1u64, other)]);
    let run = salvage_with(&hc, StateSelection::Head, false, &mut FsResolver(&wrong));
    assert_eq!(run.report.recoverability, Recoverability::Partial);
    assert!(
        finding(&run.report, "parent")[0].0.starts_with("snapshot_")
            || finding(&run.report, "parent")[0].0.starts_with("parent_")
    );
    assert!(
        per_block(&run.report)
            .iter()
            .any(|(c, basis)| *c == RangeClass::Unknown && *basis == "parent_unavailable")
    );

    // With the real parent the holes resolve and the copy is flattened.
    let run = salvage_with(&hc, StateSelection::Head, false, &mut FsResolver(&fs));
    assert_eq!(run.report.recoverability, Recoverability::Complete);
    assert_eq!(copy_bytes(&run), model.bytes);
    assert!(
        per_block(&run.report)
            .iter()
            .any(|(_, basis)| *basis == "parent_chain")
    );
}

#[test]
fn a_target_problem_never_leaves_a_published_copy() {
    let b = base(9);
    let new = |dst: &FaultHandle, uid: u64, make: Result<(), ErrorKind>| {
        let out = salvage_to_copy(
            st(&b.h),
            &mut NoParents,
            &SalvageOptions {
                selection: StateSelection::Head,
                publish_partial: false,
            },
            uid,
            || match make {
                Ok(()) => Ok(st(dst)),
                Err(kind) => Err(std::io::Error::from(kind)),
            },
        )
        .expect("analysis");
        (out.report, out.copy.is_some())
    };
    // The host refuses an existing path.
    let dst = FaultHandle::new(&[], 40);
    let (r, copy) = new(&dst, 5, Err(ErrorKind::AlreadyExists));
    assert_eq!(r.copy, CopyStatus::Failed("target_exists"));
    assert!(!copy);
    assert_eq!(r.recoverability, Recoverability::Complete);
    // A target that already holds bytes is refused and left alone.
    let old = SplitMix64(1).bytes(BS);
    let dst = FaultHandle::new(&old, 41);
    let (r, copy) = new(&dst, 5, Ok(()));
    assert_eq!(r.copy, CopyStatus::Failed("target_not_empty"));
    assert!(!copy);
    assert_eq!(dst.durable_bytes(), old);
    assert_eq!(dst.next_ordinal(), 0);
    // The copy may not claim the source's own identity, or none.
    let dst = FaultHandle::new(&[], 42);
    let (r, _) = new(&dst, 1, Ok(()));
    assert_eq!(r.copy, CopyStatus::Failed("uid_not_new"));
    assert_eq!(dst.next_ordinal(), 0);
    let err = salvage_to_copy(
        st(&b.h),
        &mut NoParents,
        &SalvageOptions {
            selection: StateSelection::Head,
            publish_partial: false,
        },
        0,
        || Ok(st(&dst)),
    )
    .err();
    assert_eq!(err, Some(SalvageError::BadUid));

    // A fault at every mutating step of the copy: failed, and the target
    // does not open (or, if the fault came after the publishing commit, it is
    // complete).
    let clean = FaultHandle::new(&[], 43);
    let (r, _) = new(&clean, 5, Ok(()));
    assert_eq!(r.copy, CopyStatus::Published);
    let ops: Vec<&str> = clean
        .trace()
        .iter()
        .filter_map(|l| {
            let op = l.strip_prefix('#')?.split_once(' ')?.1;
            ["write", "set_len", "flush"]
                .into_iter()
                .find(|k| op.starts_with(k))
        })
        .collect();
    for (k, op) in ops.iter().enumerate() {
        let (fault, reason) = match *op {
            "write" => (
                Fault::Write(WriteFault::Fail(ErrorKind::StorageFull)),
                "no_space",
            ),
            "set_len" => (Fault::Resize(ErrorKind::StorageFull), "no_space"),
            _ => (Fault::Flush(FlushFault::IoNotDurable), "target_io_error"),
        };
        let dst = FaultHandle::new(&[], 44);
        dst.script(k as u64, fault);
        let (r, copy) = new(&dst, 5, Ok(()));
        assert!(!copy, "{}", dst.report(&format!("step {k} {op}")));
        assert!(
            matches!(r.copy, CopyStatus::Failed(_)),
            "{}",
            dst.report(&format!("step {k} {op}: {:?}", r.copy))
        );
        if reason == "no_space" {
            assert_eq!(r.copy, CopyStatus::Failed("no_space"), "step {k} {op}");
        }
        dst.crash(CrashPolicy::Seeded(k as u64 + 3));
        match SnapFile::open(st(&dst), &mut NoParents) {
            Err(_) => {}
            Ok((img, _)) => {
                let mut back = vec![0u8; img.disk_size() as usize];
                img.read_at(0, &mut back).expect("read");
                assert_eq!(back, b.head().bytes, "{}", dst.report("opened but wrong"));
            }
        }
    }
}

#[test]
fn reports_are_bounded_and_carry_no_payload() {
    let b = base(10);
    let run = salvage(&b.h, StateSelection::Head, false);
    let text = run.report.render(2);
    assert!(text.is_ascii());
    assert!(text.contains("extents_total="), "{text}");
    // Many alternating extents: the list is cut, the totals are not.
    let h = FaultHandle::new(&[], 50);
    let (mut f, _) = SnapFile::create_base(st(&h), 1).expect("create");
    for i in 0..40u64 {
        f.write_at(i * 2 * BS as u64, &SplitMix64(i).bytes(BS))
            .expect("write");
    }
    drop(f);
    h.crash(CrashPolicy::DropPending);
    let run = salvage(&h, StateSelection::Head, false);
    let r = &run.report;
    assert!(r.extents.len() > 40, "{}", r.extents.len());
    let text = r.render(5);
    assert!(text.contains("extents_listed=false\n"), "{text}");
    assert_eq!(text.matches("extent=").count(), 5);
    assert!(text.contains(&format!("valid_bytes={}\n", r.bytes(RangeClass::Valid))));
    // No byte of guest data appears in the rendering.
    let payload = SplitMix64(3).bytes(BS);
    let hex = payload
        .iter()
        .take(8)
        .map(|x| format!("{x:02x}"))
        .collect::<String>();
    assert!(!text.contains(&hex));
}
