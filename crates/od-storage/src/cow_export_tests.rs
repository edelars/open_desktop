//! Tests for [`crate::cow_export`] (S9-20): RAW export is compared with a
//! plain `Vec<u8>` model that never touched the image parser, holes, zeros and
//! the partly written last block, an existing target, cancellation, a fault
//! sweep over every mutating step of the target, and import round trips.

use crate::cow_export::{
    ExportError, ExportOptions, ImageReader, ImportError, StateSelection, export_raw, import_raw,
};
use crate::cow_inspect::Outcome;
use crate::cow_proto::{BLOCK_SIZE, CowError};
use crate::cow_snapshot::{
    MAX_DISK_BLOCKS, NoParents, ParentRef, ParentResolver, ResolveError, SnapError, SnapFile,
};
use crate::io::{CheckedStorage, StorageCapabilities};
use crate::testing::fault::{
    CrashPolicy, Fault, FaultBackend, FaultHandle, FlushFault, WriteFault,
};
use crate::testing::memory::MemoryStorage;
use crate::testing::rng::SplitMix64;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::io::ErrorKind;

const BS: usize = BLOCK_SIZE;

fn st(h: &FaultHandle) -> CheckedStorage<FaultBackend> {
    CheckedStorage::new(h.open()).expect("capabilities are consistent")
}

/// Plain model of a disk: nothing but bytes.
#[derive(Clone, Default)]
struct Model {
    bytes: Vec<u8>,
}

impl Model {
    fn write(&mut self, offset: usize, data: &[u8]) {
        let end = offset + data.len();
        // The layer grows in whole blocks.
        let blocks = end.div_ceil(BS);
        if self.bytes.len() < blocks * BS {
            self.bytes.resize(blocks * BS, 0);
        }
        self.bytes[offset..end].copy_from_slice(data);
    }
}

/// An image together with the model of its head and of every snapshot.
struct Fixture {
    h: FaultHandle,
    head: Model,
    snaps: BTreeMap<u64, Model>,
}

/// Writes at odd offsets (partial blocks), leaves holes, snapshots twice and
/// ends with a partly written last block.
fn fixture(seed: u64) -> Fixture {
    let h = FaultHandle::new(&[], seed);
    let (mut f, _) = SnapFile::create_base(st(&h), 1).expect("create");
    let mut m = Model::default();
    let mut snaps = BTreeMap::new();
    let put = |f: &mut SnapFile<FaultBackend>, m: &mut Model, at: usize, data: Vec<u8>| {
        f.write_at(at as u64, &data).expect("write");
        m.write(at, &data);
    };
    put(&mut f, &mut m, 3, SplitMix64(seed).bytes(700));
    // A hole of several blocks, then data.
    put(
        &mut f,
        &mut m,
        9 * BS + 100,
        SplitMix64(seed + 1).bytes(2 * BS),
    );
    let (id, _) = f.create_snapshot().expect("snapshot 1");
    snaps.insert(id, m.clone());
    put(&mut f, &mut m, 0, vec![0u8; BS]); // explicit zeros over data
    put(&mut f, &mut m, 5 * BS + 7, SplitMix64(seed + 2).bytes(40));
    let (id, _) = f.create_snapshot().expect("snapshot 2");
    snaps.insert(id, m.clone());
    // Last block only partly written: 5 bytes at the start of block 14.
    put(&mut f, &mut m, 14 * BS, vec![0xAB; 5]);
    drop(f);
    h.crash(CrashPolicy::DropPending);
    Fixture { h, head: m, snaps }
}

fn target() -> FaultHandle {
    FaultHandle::new(&[], 99)
}

fn export(
    src: &FaultHandle,
    sel: StateSelection,
    dst: &FaultHandle,
) -> Result<crate::cow_export::ExportReport, ExportError> {
    export_raw(
        st(src),
        &mut NoParents,
        sel,
        || Ok(st(dst)),
        &ExportOptions::default(),
        &|| false,
    )
}

#[test]
fn export_matches_the_independent_model_for_head_and_every_snapshot() {
    let fx = fixture(1);
    let selections = [
        (StateSelection::Head, fx.head.bytes.clone()),
        (StateSelection::Snapshot(1), fx.snaps[&1].bytes.clone()),
        (StateSelection::Snapshot(2), fx.snaps[&2].bytes.clone()),
    ];
    for (sel, expected) in selections {
        let dst = target();
        let rep = export(&fx.h, sel, &dst).unwrap_or_else(|e| panic!("{sel:?}: {e:?}"));
        // The RAW is read as plain bytes: no parser, no image code.
        assert_eq!(dst.durable_bytes(), expected, "{sel:?}");
        assert_eq!(rep.logical_size, expected.len() as u64);
        assert!(rep.file_durable);
        assert_eq!(rep.blocks, (expected.len() / BS) as u64);
        let zero = expected
            .chunks(BS)
            .filter(|b| b.iter().all(|x| *x == 0))
            .count() as u64;
        assert_eq!(rep.zero_blocks, zero, "{sel:?}");
        // Nothing but the exported bytes is volatile: a crash keeps them.
        assert_eq!(dst.crash(CrashPolicy::DropPending), expected, "{sel:?}");
    }
}

#[test]
fn holes_zeros_and_a_partly_written_last_block_export_exactly() {
    let fx = fixture(2);
    // Sanity of the fixture: it has holes, explicit zeros, and a last block
    // that is almost entirely zero.
    let head = &fx.head.bytes;
    assert_eq!(head.len(), 15 * BS);
    assert!(head[4 * BS..5 * BS].iter().all(|x| *x == 0), "a hole");
    assert!(head[..BS].iter().all(|x| *x == 0), "explicit zeros");
    assert_eq!(
        &head[14 * BS..14 * BS + 6],
        &[0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0]
    );
    let dst = target();
    export(&fx.h, StateSelection::Head, &dst).expect("export");
    assert_eq!(dst.durable_bytes(), *head);

    // A completely empty image exports an empty file of exactly size 0.
    let h = FaultHandle::new(&[], 3);
    SnapFile::create_base(st(&h), 5).expect("create");
    h.crash(CrashPolicy::DropPending);
    let dst = target();
    let rep = export(&h, StateSelection::Head, &dst).expect("empty export");
    assert_eq!((rep.logical_size, rep.blocks), (0, 0));
    assert!(dst.durable_bytes().is_empty());
}

struct FsResolver<'a>(&'a BTreeMap<u64, FaultHandle>);

impl ParentResolver for FsResolver<'_> {
    type Backend = FaultBackend;

    fn open_parent(&mut self, p: &ParentRef) -> Result<CheckedStorage<FaultBackend>, ResolveError> {
        let h = self.0.get(&p.file_uid).ok_or(ResolveError::Missing)?;
        CheckedStorage::new(h.open()).map_err(ResolveError::Storage)
    }
}

#[test]
fn a_child_is_flattened_through_its_verified_chain_and_needs_the_parent() {
    let fx = fixture(4);
    let (b, _) = SnapFile::open(st(&fx.h), &mut NoParents).expect("open base");
    let pref = b.snapshot_ref(2).expect("snapshot 2 ref");
    drop(b);
    fx.h.crash(CrashPolicy::DropPending);
    let fs = BTreeMap::from([(1u64, fx.h.clone())]);
    let hc = FaultHandle::new(&[], 7);
    let (mut c, _) =
        SnapFile::create_child(st(&hc), &mut FsResolver(&fs), &pref, 2).expect("child");
    let mut model = fx.snaps[&2].clone();
    let patch = SplitMix64(11).bytes(300);
    c.write_at(2 * BS as u64 + 50, &patch).expect("write child");
    model.write(2 * BS + 50, &patch);
    drop(c);
    hc.crash(CrashPolicy::DropPending);

    let dst = target();
    export_raw(
        st(&hc),
        &mut FsResolver(&fs),
        StateSelection::Head,
        || Ok(st(&dst)),
        &ExportOptions::default(),
        &|| false,
    )
    .expect("export child");
    assert_eq!(dst.durable_bytes(), model.bytes);

    // No parent: refused before anything is written to the target.
    let dst = target();
    let ord = dst.next_ordinal();
    let err = export(&hc, StateSelection::Head, &dst).expect_err("no parent");
    assert_eq!(err, ExportError::Source(SnapError::ParentMissing));
    assert_eq!(err.classify().1, "parent_missing");
    assert_eq!(dst.next_ordinal(), ord, "the target was touched");
}

#[test]
fn a_missing_snapshot_and_a_source_that_may_have_lost_a_commit_are_refused() {
    let fx = fixture(5);
    let dst = target();
    let err = export(&fx.h, StateSelection::Snapshot(9), &dst).expect_err("missing");
    assert_eq!(
        err,
        ExportError::Source(SnapError::SnapshotMissing { id: 9 })
    );
    assert_eq!(dst.next_ordinal(), 0);

    // The newest root slot and its copy are gone: a newer acknowledged commit
    // may be lost, so an older state must not be exported as current.
    let mut bytes = fx.h.durable_bytes();
    let r = ImageReader::open(st(&fx.h), &mut NoParents).expect("reader");
    let gen_slot = ((r.generation() & 1) as usize) * 512;
    drop(r);
    bytes[gen_slot..gen_slot + 512].fill(0);
    let len = bytes.len();
    bytes[len - 512..].fill(0);
    let lost = FaultHandle::new(&bytes, 6);
    let err = export(&lost, StateSelection::Head, &dst).expect_err("lost commit");
    assert!(
        matches!(
            err,
            ExportError::Source(SnapError::Cow(CowError::DamagedOtherSlot { .. }))
        ),
        "{err:?}"
    );
    assert_eq!(err.classify().0, Outcome::Corrupt);
    assert_eq!(dst.next_ordinal(), 0, "the target was touched");
}

#[test]
fn an_existing_target_is_refused_and_its_bytes_do_not_change() {
    let fx = fixture(6);
    let old = SplitMix64(1).bytes(3 * BS);
    let dst = FaultHandle::new(&old, 8);
    let ord = dst.next_ordinal();
    let err = export(&fx.h, StateSelection::Head, &dst).expect_err("existing");
    assert_eq!(
        err,
        ExportError::TargetNotEmpty {
            len: old.len() as u64
        }
    );
    assert_eq!(dst.durable_bytes(), old);
    assert_eq!(dst.volatile_bytes(), old);
    assert_eq!(dst.next_ordinal(), ord, "no mutating call was made");
}

#[test]
fn the_target_is_created_only_after_the_source_verified_and_an_existing_name_is_refused() {
    let fx = fixture(12);
    let called = Cell::new(0u32);
    let make = |kind: Option<ErrorKind>, dst: &FaultHandle| {
        called.set(called.get() + 1);
        match kind {
            None => Ok(st(dst)),
            Some(k) => Err(std::io::Error::from(k)),
        }
    };
    // The host reports an existing name: nothing is written, reason is named.
    let dst = target();
    let err = export_raw(
        st(&fx.h),
        &mut NoParents,
        StateSelection::Head,
        || make(Some(ErrorKind::AlreadyExists), &dst),
        &ExportOptions::default(),
        &|| false,
    )
    .expect_err("existing name");
    assert_eq!(err, ExportError::TargetCreate(ErrorKind::AlreadyExists));
    assert_eq!(err.classify(), (Outcome::Io, "target_exists", None));
    assert_eq!(called.get(), 1);
    assert_eq!(dst.next_ordinal(), 0);
    // A missing snapshot or an unusable source never reaches `make_target`.
    called.set(0);
    let err = export_raw(
        st(&fx.h),
        &mut NoParents,
        StateSelection::Snapshot(42),
        || make(None, &dst),
        &ExportOptions::default(),
        &|| false,
    )
    .expect_err("missing snapshot");
    assert!(matches!(
        err,
        ExportError::Source(SnapError::SnapshotMissing { id: 42 })
    ));
    let junk = FaultHandle::new(&[0xEE; 4096], 13);
    let err = export_raw(
        st(&junk),
        &mut NoParents,
        StateSelection::Head,
        || make(None, &dst),
        &ExportOptions::default(),
        &|| false,
    )
    .expect_err("not an image");
    assert_eq!(err.classify().0, Outcome::Corrupt);
    assert_eq!(
        called.get(),
        0,
        "a target was created for an unusable source"
    );
}

#[test]
fn a_target_that_cannot_be_durable_is_refused_before_any_write() {
    let fx = fixture(7);
    let plain = StorageCapabilities::read_write(1 << 20);
    let target =
        |caps| CheckedStorage::new(MemoryStorage::with_capabilities(&[], caps)).expect("st");
    let err = export_raw(
        st(&fx.h),
        &mut NoParents,
        StateSelection::Head,
        || Ok(target(plain)),
        &ExportOptions::default(),
        &|| false,
    )
    .expect_err("no durability");
    assert_eq!(err, ExportError::DurabilityUnavailable);
    // A read-only target is unsuitable.
    let err = export_raw(
        st(&fx.h),
        &mut NoParents,
        StateSelection::Head,
        || Ok(target(StorageCapabilities::read_only(1 << 20))),
        &ExportOptions::default(),
        &|| false,
    )
    .expect_err("read-only");
    assert_eq!(err, ExportError::TargetUnsuitable);
    // Explicitly allowed: the export works and says it was not made durable.
    let rep = export_raw(
        st(&fx.h),
        &mut NoParents,
        StateSelection::Head,
        || Ok(target(plain)),
        &ExportOptions {
            require_file_durability: false,
        },
        &|| false,
    )
    .expect("export");
    assert!(!rep.file_durable);
}

/// Kinds of the mutating calls of one clean export, from its trace.
fn mutating_ops(dst: &FaultHandle) -> Vec<&'static str> {
    dst.trace()
        .iter()
        .filter_map(|l| {
            let rest = l.strip_prefix('#')?;
            let (_, op) = rest.split_once(' ')?;
            if op.starts_with("write") {
                Some("write")
            } else if op.starts_with("set_len") {
                Some("resize")
            } else if op.starts_with("flush") {
                Some("flush")
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn a_fault_at_every_mutating_step_of_the_target_never_reports_a_completed_export() {
    let fx = fixture(8);
    let expected = fx.head.bytes.clone();
    let clean = target();
    export(&fx.h, StateSelection::Head, &clean).expect("clean export");
    let ops = mutating_ops(&clean);
    assert!(ops.len() >= 3, "{ops:?}");
    assert_eq!(ops[0], "resize");
    assert_eq!(ops[ops.len() - 1], "flush");

    for (k, op) in ops.iter().enumerate() {
        let faults: Vec<Fault> = match *op {
            "write" => vec![
                Fault::Write(WriteFault::Fail(ErrorKind::StorageFull)),
                Fault::Write(WriteFault::TornIo { written: 100 }),
            ],
            "resize" => vec![Fault::Resize(ErrorKind::StorageFull)],
            _ => vec![
                Fault::Flush(FlushFault::IoNotDurable),
                Fault::Flush(FlushFault::IoAfterDurable),
            ],
        };
        for fault in faults {
            let dst = target();
            dst.script(k as u64, fault);
            let result = export(&fx.h, StateSelection::Head, &dst);
            let msg = |m: &str| dst.report(&format!("step {k} {op} {fault:?}: {m}"));
            let err = result.as_ref().expect_err(&msg("export reported success"));
            // ENOSPC is named as such.
            if matches!(
                fault,
                Fault::Write(WriteFault::Fail(ErrorKind::StorageFull))
                    | Fault::Resize(ErrorKind::StorageFull)
            ) {
                assert_eq!(err.classify().1, "no_space", "{}", msg("reason"));
            }
            assert_eq!(err.classify().0, Outcome::Io);
            // Whatever the crash keeps, the caller never got a report that
            // would let it publish; and bytes that did land are never claimed.
            let after = dst.crash(CrashPolicy::Seeded(k as u64 + 1));
            assert!(after.len() <= expected.len());
        }
    }

    // No fault: every step succeeds and a crash keeps the exact bytes.
    let dst = target();
    export(&fx.h, StateSelection::Head, &dst).expect("export");
    assert_eq!(dst.crash(CrashPolicy::DropPending), expected);
}

#[test]
fn cancellation_at_every_poll_stops_with_cancelled_and_never_succeeds() {
    let fx = fixture(9);
    let polls = Cell::new(0u32);
    let dst = target();
    export_raw(
        st(&fx.h),
        &mut NoParents,
        StateSelection::Head,
        || Ok(st(&dst)),
        &ExportOptions::default(),
        &|| {
            polls.set(polls.get() + 1);
            false
        },
    )
    .expect("uncancelled");
    let total = polls.get();
    assert!(total >= 3, "{total}");
    for n in 1..=total {
        let seen = Cell::new(0u32);
        let dst = target();
        let result = export_raw(
            st(&fx.h),
            &mut NoParents,
            StateSelection::Head,
            || Ok(st(&dst)),
            &ExportOptions::default(),
            &|| {
                seen.set(seen.get() + 1);
                seen.get() == n
            },
        );
        assert_eq!(result, Err(ExportError::Cancelled), "poll {n}");
    }
}

#[test]
fn a_source_changed_after_the_reader_fixed_its_root_is_noticed() {
    let fx = fixture(10);
    let reader = ImageReader::open(st(&fx.h), &mut NoParents).expect("reader");
    assert!(reader.root_unchanged().expect("check"));
    let before = reader.generation();
    // A writer appears (the lease was not honoured): the reader's fixed root
    // is no longer the newest.
    let (mut w, _) = SnapFile::open(st(&fx.h), &mut NoParents).expect("writer");
    w.write_at(0, &[1, 2, 3]).expect("write");
    drop(w);
    assert!(!reader.root_unchanged().expect("check"));
    // What the reader fixed is still readable and is the old state.
    let mut buf = vec![0u8; fx.head.bytes.len()];
    reader
        .read(StateSelection::Head, 0, &mut buf)
        .expect("read");
    assert_eq!(buf, fx.head.bytes);
    assert_eq!(reader.generation(), before);
}

#[test]
fn a_corrupt_source_block_fails_the_export_without_a_report() {
    let fx = fixture(11);
    let (f, _) = SnapFile::open(st(&fx.h), &mut NoParents).expect("open");
    let blocks: Vec<u64> = f
        .rootset()
        .expect("rootset")
        .data_blocks
        .into_iter()
        .collect();
    drop(f);
    let mut bytes = fx.h.durable_bytes();
    let newest = *blocks.last().expect("a block");
    bytes[newest as usize + 3] ^= 0x40;
    let bad = FaultHandle::new(&bytes, 12);
    let dst = target();
    let err = export(&bad, StateSelection::Head, &dst).expect_err("corrupt");
    assert_eq!(err.classify().0, Outcome::Corrupt);
}

// ------------------------------------------------------------------ import

fn raw_of(bytes: &[u8]) -> CheckedStorage<MemoryStorage> {
    CheckedStorage::new(MemoryStorage::new_read_only(bytes)).expect("raw")
}

#[test]
fn import_then_export_round_trips_and_an_independent_reader_agrees() {
    // 300 blocks crosses both the import commit size and the export chunk.
    let mut raw = SplitMix64(21).bytes(300 * BS);
    raw[40 * BS..60 * BS].fill(0); // a run of zero blocks
    let dst = FaultHandle::new(&[], 30);
    let (image, rep) = import_raw(&raw_of(&raw), || Ok(st(&dst)), 77, &|| false).expect("import");
    assert_eq!(
        (rep.logical_size, rep.blocks, rep.file_uid),
        (raw.len() as u64, 300, 77)
    );
    drop(image);
    dst.crash(CrashPolicy::DropPending);

    // It opens, and its content equals the RAW byte for byte.
    let (img, _) = SnapFile::open(st(&dst), &mut NoParents).expect("open imported");
    assert_eq!(img.disk_size(), raw.len() as u64);
    let mut back = vec![0u8; raw.len()];
    img.read_at(0, &mut back).expect("read");
    assert_eq!(back, raw);
    drop(img);
    dst.crash(CrashPolicy::DropPending);

    // And exports back to the same RAW.
    let out = target();
    export(&dst, StateSelection::Head, &out).expect("export");
    assert_eq!(out.durable_bytes(), raw);
}

#[test]
fn import_refuses_unaligned_oversized_and_existing_targets_without_touching_them() {
    let dst = FaultHandle::new(&[], 31);
    // A size that is refused never creates a target: `make_target` would turn
    // the result into `TargetCreate`.
    let never = || -> Result<CheckedStorage<FaultBackend>, std::io::Error> {
        Err(std::io::Error::other("target created for a refused source"))
    };
    let err = import_raw(&raw_of(&vec![1u8; BS + 1]), never, 5, &|| false).err();
    assert_eq!(
        err,
        Some(ImportError::NotBlockAligned { len: BS as u64 + 1 })
    );
    assert_eq!(dst.next_ordinal(), 0);

    let big = vec![0u8; (MAX_DISK_BLOCKS as usize + 1) * BS];
    let err = import_raw(&raw_of(&big), never, 5, &|| false).err();
    assert_eq!(
        err,
        Some(ImportError::TooLarge {
            blocks: MAX_DISK_BLOCKS + 1,
            limit: MAX_DISK_BLOCKS
        })
    );
    assert_eq!(dst.next_ordinal(), 0);

    let old = SplitMix64(3).bytes(2 * BS);
    let existing = FaultHandle::new(&old, 32);
    let err = import_raw(&raw_of(&[7u8; BS]), || Ok(st(&existing)), 5, &|| false).err();
    assert_eq!(
        err,
        Some(ImportError::TargetNotEmpty {
            len: old.len() as u64
        })
    );
    assert_eq!(existing.durable_bytes(), old);
    assert_eq!(existing.next_ordinal(), 0);

    // A zero uid is refused too (identity of the new file).
    let err = import_raw(&raw_of(&[7u8; BS]), || Ok(st(&dst)), 0, &|| false).err();
    assert!(matches!(err, Some(ImportError::Image(_))), "{err:?}");
}

#[test]
fn an_import_target_that_cannot_be_durable_is_refused_before_any_write() {
    use crate::cow_salvage_tests::Weakened;
    let raw = SplitMix64(23).bytes(3 * BS);
    let run = |dst: &FaultHandle, durable: bool, writable: bool| {
        let target = CheckedStorage::new(Weakened {
            inner: dst.open(),
            durable,
            writable,
        })
        .expect("capabilities are consistent");
        import_raw(&raw_of(&raw), || Ok(target), 9, &|| false).err()
    };
    let dst = FaultHandle::new(&[], 34);
    assert_eq!(
        run(&dst, false, true),
        Some(ImportError::DurabilityUnavailable)
    );
    assert_eq!(dst.next_ordinal(), 0, "the target was written to");
    assert!(
        dst.volatile_bytes().is_empty(),
        "a building target was left"
    );
    let dst = FaultHandle::new(&[], 35);
    assert_eq!(run(&dst, true, false), Some(ImportError::TargetUnsuitable));
    assert_eq!(dst.next_ordinal(), 0);
    assert!(dst.volatile_bytes().is_empty());
    let dst = FaultHandle::new(&[], 36);
    assert_eq!(run(&dst, true, true), None);
}

#[test]
fn an_import_that_stops_early_leaves_a_target_that_does_not_open() {
    let raw = SplitMix64(22).bytes(40 * BS);
    // Cancelled before publication.
    let dst = FaultHandle::new(&[], 33);
    let polls = Cell::new(0u32);
    let err = import_raw(&raw_of(&raw), || Ok(st(&dst)), 9, &|| {
        polls.set(polls.get() + 1);
        polls.get() == 2
    })
    .err();
    assert_eq!(err, Some(ImportError::Cancelled));
    dst.crash(CrashPolicy::DropPending);
    let r = SnapFile::open(st(&dst), &mut NoParents).err();
    assert_eq!(r, Some(SnapError::BuildInProgress));

    // A fault at every mutating step: either an error and no opening image,
    // or success and a complete one.
    let clean = FaultHandle::new(&[], 34);
    import_raw(&raw_of(&raw), || Ok(st(&clean)), 9, &|| false).expect("clean import");
    let ops = mutating_ops(&clean);
    for (k, op) in ops.iter().enumerate() {
        let fault = match *op {
            "write" => Fault::Write(WriteFault::Fail(ErrorKind::StorageFull)),
            "resize" => Fault::Resize(ErrorKind::StorageFull),
            _ => Fault::Flush(FlushFault::IoNotDurable),
        };
        let dst = FaultHandle::new(&[], 35);
        dst.script(k as u64, fault);
        let result = import_raw(&raw_of(&raw), || Ok(st(&dst)), 9, &|| false);
        assert!(result.is_err(), "{}", dst.report(&format!("step {k} {op}")));
        drop(result);
        dst.crash(CrashPolicy::Seeded(k as u64 + 7));
        match SnapFile::open(st(&dst), &mut NoParents) {
            Err(_) => {}
            Ok((img, _)) => {
                // Only an image that never reached the publishing commit can
                // open without being complete; it must then be a *complete*
                // one (the fault hit after the commit's own barrier).
                let mut back = vec![0u8; raw.len()];
                img.read_at(0, &mut back).expect("read");
                assert_eq!(back, raw, "{}", dst.report(&format!("step {k} {op}")));
            }
        }
    }
}
