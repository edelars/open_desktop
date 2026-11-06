//! Tests for [`crate::cow_inspect`] (S9-19): outcomes, exit-relevant
//! classification, read-only behaviour and bounded, payload-free output.

use crate::cow_inspect::{
    ChainLink, InspectMode, InspectReport, LayerInfo, LayerKind, MAX_REPORT_BYTES, Outcome,
    inspect_storage,
};
use crate::cow_proto::{BLOCK_SIZE, CowFile, OpenClass, crc32, slot_offset};
use crate::cow_snapshot::{Header, NoParents, ParentRef, ParentResolver, ResolveError, SnapFile};
use crate::io::{BackendError, CheckedStorage, StorageBackend, StorageCapabilities};
use crate::testing::fault::{CrashPolicy, FaultBackend, FaultHandle};
use crate::testing::rng::SplitMix64;
use std::collections::BTreeMap;

const BS: usize = BLOCK_SIZE;

fn st(h: &FaultHandle) -> CheckedStorage<FaultBackend> {
    CheckedStorage::new(h.open()).expect("capabilities are consistent")
}

/// Base file (uid 1): 3 blocks, snapshot 1, overwrite, snapshot 2, more data.
fn make_base(h: &FaultHandle, payload: &[u8]) -> Vec<u64> {
    let (mut f, _) = SnapFile::create_base(st(h), 1).expect("create");
    f.write_at(0, payload).expect("write");
    f.write_at(2 * BS as u64, &SplitMix64(1).bytes(BS))
        .expect("write");
    f.create_snapshot().expect("snapshot 1");
    f.write_at(0, &SplitMix64(2).bytes(BS)).expect("overwrite");
    f.create_snapshot().expect("snapshot 2");
    f.write_at(BS as u64, &SplitMix64(3).bytes(BS))
        .expect("write");
    let blocks = f
        .rootset()
        .expect("rootset")
        .data_blocks
        .into_iter()
        .collect();
    drop(f);
    h.crash(CrashPolicy::DropPending);
    blocks
}

struct Fs(BTreeMap<u64, FaultHandle>);

struct FsResolver<'a> {
    fs: &'a Fs,
    busy: bool,
}

impl ParentResolver for FsResolver<'_> {
    type Backend = FaultBackend;

    fn open_parent(&mut self, p: &ParentRef) -> Result<CheckedStorage<FaultBackend>, ResolveError> {
        if self.busy {
            return Err(ResolveError::Busy);
        }
        let h = self.fs.0.get(&p.file_uid).ok_or(ResolveError::Missing)?;
        CheckedStorage::new(h.open()).map_err(ResolveError::Storage)
    }
}

/// Inspects without a parent resolver and proves nothing was written.
fn inspect(h: &FaultHandle, mode: InspectMode) -> InspectReport {
    let (before, ord) = (h.volatile_bytes(), h.next_ordinal());
    let rep = inspect_storage(st(h), &mut NoParents, mode);
    assert_eq!(h.volatile_bytes(), before, "inspection changed the media");
    assert_eq!(h.next_ordinal(), ord, "inspection mutated the backend");
    rep
}

fn forge(bytes: &mut [u8], f: impl Fn(&mut [u8])) {
    f(bytes)
}

fn set_mandatory(bytes: &mut [u8], slot: usize, bit: u32) {
    let at = slot * 512;
    bytes[at + 52..at + 60].copy_from_slice(&(1u64 << bit).to_le_bytes());
    let crc = crc32(&bytes[at..at + 508]);
    bytes[at + 508..at + 512].copy_from_slice(&crc.to_le_bytes());
}

fn flip(bytes: &mut [u8], at: u64) {
    bytes[at as usize] ^= 0x40;
}

#[test]
fn a_valid_base_image_reports_its_identity_in_both_modes() {
    let h = FaultHandle::new(&[], 1);
    make_base(&h, &SplitMix64(9).bytes(2 * BS));
    for mode in [InspectMode::Info, InspectMode::Check] {
        let r = inspect(&h, mode);
        assert_eq!(r.outcome, Outcome::Ok, "{mode:?}: {}", r.reason);
        assert_eq!(r.reason, "ok");
        assert_eq!(r.open_class, Some(OpenClass::Committed));
        assert_eq!(r.other_slot, Some("valid"));
        assert!(!r.repair_needed);
        assert_eq!(r.cow_version, 1);
        assert_eq!(r.layer_version, 1);
        let l = r.layer.as_ref().expect("layer");
        assert_eq!(l.kind, LayerKind::Base);
        assert_eq!((l.file_uid, l.depth, l.disk_blocks), (1, 0, 3));
        assert_eq!(l.logical_size, 3 * BS as u64);
        assert_eq!(l.snapshot_ids, vec![1, 2]);
        assert_eq!(l.next_snapshot_id, 3);
        assert!(r.chain.is_empty());
        assert_eq!(r.chain_verified, mode == InspectMode::Check);
        assert!(r.root_generation.is_some());
        assert_eq!(r.file_len, Some(h.durable_bytes().len() as u64));
        let text = r.render();
        assert!(text.len() <= MAX_REPORT_BYTES);
        assert!(
            text.contains("outcome=ok\n") && text.contains("snapshots=1,2\n"),
            "{text}"
        );
    }
}

#[test]
fn a_child_chain_is_listed_and_verified_only_in_check_mode() {
    let hb = FaultHandle::new(&[], 1);
    make_base(&hb, &SplitMix64(9).bytes(2 * BS));
    let (b, _) = SnapFile::open(st(&hb), &mut NoParents).expect("base");
    let pref = b.snapshot_ref(2).expect("ref");
    drop(b);
    hb.crash(CrashPolicy::DropPending);
    let mut fs = Fs(BTreeMap::from([(1, hb.clone())]));
    let hc = FaultHandle::new(&[], 2);
    let (mut c, _) = SnapFile::create_child(
        st(&hc),
        &mut FsResolver {
            fs: &fs,
            busy: false,
        },
        &pref,
        2,
    )
    .expect("child");
    c.write_at(5, &[1, 2, 3]).expect("write");
    drop(c);
    hc.crash(CrashPolicy::DropPending);

    // Info lists the recorded link without trusting it.
    let r = inspect_storage(st(&hc), &mut NoParents, InspectMode::Info);
    assert_eq!(r.outcome, Outcome::Ok);
    assert_eq!(r.layer.as_ref().map(|l| l.kind), Some(LayerKind::Child));
    assert_eq!(
        r.chain,
        vec![ChainLink {
            file_uid: 1,
            snapshot_id: 2
        }]
    );
    assert!(!r.chain_verified);

    // Check resolves and verifies the chain.
    let r = inspect_storage(
        st(&hc),
        &mut FsResolver {
            fs: &fs,
            busy: false,
        },
        InspectMode::Check,
    );
    assert_eq!(r.outcome, Outcome::Ok, "{}", r.reason);
    assert!(r.chain_verified);
    assert_eq!(
        r.chain,
        vec![ChainLink {
            file_uid: 1,
            snapshot_id: 2
        }]
    );
    assert_eq!(r.layer.as_ref().map(|l| l.depth), Some(1));

    // Without the parent, or with another one, or while it is held: typed.
    let r = inspect_storage(st(&hc), &mut NoParents, InspectMode::Check);
    assert_eq!((r.outcome, r.reason), (Outcome::Corrupt, "parent_missing"));
    let r = inspect_storage(
        st(&hc),
        &mut FsResolver {
            fs: &fs,
            busy: true,
        },
        InspectMode::Check,
    );
    assert_eq!((r.outcome, r.reason), (Outcome::Busy, "parent_busy"));
    let other = FaultHandle::new(&[], 3);
    let (mut o, _) = SnapFile::create_base(st(&other), 77).expect("other");
    o.write_at(0, &[1u8; 10]).expect("write");
    o.create_snapshot().expect("snapshot");
    drop(o);
    other.crash(CrashPolicy::DropPending);
    fs.0.insert(1, other);
    let r = inspect_storage(
        st(&hc),
        &mut FsResolver {
            fs: &fs,
            busy: false,
        },
        InspectMode::Check,
    );
    assert_eq!(
        (r.outcome, r.reason),
        (Outcome::Corrupt, "parent_identity_mismatch")
    );
}

#[test]
fn a_corrupt_block_is_named_and_a_snapshot_only_block_needs_check_mode() {
    let h = FaultHandle::new(&[], 1);
    let blocks = make_base(&h, &SplitMix64(9).bytes(2 * BS));
    // Newest head block: found by the parser's own verification.
    let newest = *blocks.last().expect("a block");
    let mut bytes = h.durable_bytes();
    flip(&mut bytes, newest + 3);
    let bad = FaultHandle::new(&bytes, 2);
    for mode in [InspectMode::Info, InspectMode::Check] {
        let r = inspect(&bad, mode);
        assert_eq!(
            (r.outcome, r.reason),
            (Outcome::Corrupt, "block_corrupt"),
            "{mode:?}"
        );
        assert!(r.detail.is_some());
    }

    // A block only snapshot 1 still reaches (the head overwrote it).
    let (f, _) = SnapFile::open(st(&h), &mut NoParents).expect("open");
    let only_snap = *f
        .snapshot_data_blocks(1)
        .expect("blocks")
        .difference(&f.snapshot_data_blocks(2).expect("blocks"))
        .next()
        .expect("a snapshot-only block");
    drop(f);
    h.crash(CrashPolicy::DropPending);
    let mut bytes = h.durable_bytes();
    flip(&mut bytes, only_snap + 7);
    let bad = FaultHandle::new(&bytes, 3);
    assert_eq!(inspect(&bad, InspectMode::Info).outcome, Outcome::Ok);
    let r = inspect(&bad, InspectMode::Check);
    assert_eq!(
        (r.outcome, r.reason, r.detail),
        (Outcome::Corrupt, "snapshot_block_corrupt", Some(1))
    );
}

#[test]
fn an_unsupported_mandatory_feature_is_unsupported_not_corrupt() {
    let h = FaultHandle::new(&[], 1);
    make_base(&h, &SplitMix64(9).bytes(BS));
    let mut bytes = h.durable_bytes();
    forge(&mut bytes, |b| {
        set_mandatory(b, 0, 40);
        set_mandatory(b, 1, 40);
    });
    let r = inspect(&FaultHandle::new(&bytes, 2), InspectMode::Info);
    assert_eq!(
        (r.outcome, r.reason, r.detail),
        (
            Outcome::Unsupported,
            "unsupported_mandatory_feature",
            Some(40)
        )
    );
}

#[test]
fn foreign_empty_and_unpublished_files_are_classified() {
    // A plain CoW file (block 0 is user data): valid container, not an image.
    let h = FaultHandle::new(&[], 1);
    let (mut c, _) = CowFile::create(st(&h)).expect("create");
    c.commit(&[(0, &[0x11u8; BS][..]), (1, &[0x22u8; BS][..])])
        .expect("commit");
    drop(c);
    h.crash(CrashPolicy::DropPending);
    let r = inspect(&h, InspectMode::Check);
    assert_eq!(
        (r.outcome, r.reason),
        (Outcome::Unsupported, "not_a_layer_file")
    );
    // Empty and garbage files.
    let r = inspect(&FaultHandle::new(&[], 2), InspectMode::Info);
    assert_eq!((r.outcome, r.reason), (Outcome::Corrupt, "no_valid_root"));
    let r = inspect(&FaultHandle::new(&[0xEE; 4096], 3), InspectMode::Info);
    assert_eq!((r.outcome, r.reason), (Outcome::Corrupt, "no_valid_root"));
    // An unpublished compaction target shows its layer but is refused.
    let h = FaultHandle::new(&[], 4);
    let building = Header {
        building: true,
        file_uid: 9,
        disk_blocks: 0,
        next_snapshot_id: 1,
        depth: 0,
        parent: None,
        snapshots: Vec::new(),
    };
    SnapFile::init(st(&h), building, Vec::new()).expect("init");
    h.crash(CrashPolicy::DropPending);
    let r = inspect(&h, InspectMode::Info);
    assert_eq!(
        (r.outcome, r.reason),
        (Outcome::Corrupt, "unpublished_compaction_target")
    );
    assert_eq!(r.layer.as_ref().map(|l| l.building), Some(true));
}

#[test]
fn damaged_roots_are_reported_and_never_repaired() {
    let h = FaultHandle::new(&[], 1);
    make_base(&h, &SplitMix64(9).bytes(BS));
    let gen_slot = {
        let r = inspect(&h, InspectMode::Info);
        slot_offset(r.root_generation.expect("generation")) as usize
    };
    // Torn root slot with an intact root copy: a writable open would roll
    // forward. The inspection says so and writes nothing.
    let mut bytes = h.durable_bytes();
    let before_len = bytes.len();
    bytes[gen_slot..gen_slot + 512].fill(0);
    // The copy of the *newest* root sits in the last sector; the slot we just
    // blanked held the newest root, so recovery sees a valid older root plus a
    // copy of the newer one.
    let torn = FaultHandle::new(&bytes, 2);
    let r = inspect(&torn, InspectMode::Check);
    assert_eq!(r.open_class, Some(OpenClass::RolledForward));
    assert!(r.repair_needed);
    assert_eq!(r.outcome, Outcome::Ok, "{}", r.reason);
    assert_eq!(torn.durable_bytes().len(), before_len);

    // Lose the copy too: a newer acknowledged commit may be gone.
    let mut bytes = h.durable_bytes();
    bytes[gen_slot..gen_slot + 512].fill(0);
    let len = bytes.len();
    bytes[len - 512..].fill(0);
    let lost = FaultHandle::new(&bytes, 3);
    let r = inspect(&lost, InspectMode::Check);
    assert_eq!(
        (r.outcome, r.reason),
        (Outcome::Corrupt, "possible_lost_commit")
    );
    assert_eq!(r.open_class, Some(OpenClass::PossibleLostCommit));
    assert!(!r.repair_needed);
}

/// Backend whose reads fail.
struct BrokenReads;

impl StorageBackend for BrokenReads {
    fn capabilities(&self) -> StorageCapabilities {
        StorageCapabilities::read_only(1 << 20)
    }

    fn size(&self) -> Result<u64, BackendError> {
        Ok(4096)
    }

    fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> Result<usize, BackendError> {
        Err(BackendError::Io {
            kind: std::io::ErrorKind::Other,
            os_error: Some(5),
        })
    }

    fn write_at(&mut self, _offset: u64, _buf: &[u8]) -> Result<usize, BackendError> {
        Err(BackendError::Unsupported)
    }
}

#[test]
fn an_io_failure_is_an_io_outcome() {
    let storage = CheckedStorage::new(BrokenReads).expect("checked");
    let r = inspect_storage(storage, &mut NoParents, InspectMode::Check);
    assert_eq!((r.outcome, r.reason), (Outcome::Io, "io_error"));
}

#[test]
fn reports_are_bounded_and_carry_no_payload() {
    // Guest payload with a recognisable marker is never echoed.
    let marker = b"TOP-SECRET-GUEST-PAYLOAD-0123456789";
    let mut payload = Vec::new();
    while payload.len() < 2 * BS {
        payload.extend_from_slice(marker);
    }
    let h = FaultHandle::new(&[], 1);
    make_base(&h, &payload);
    let text = inspect(&h, InspectMode::Check).render();
    assert!(!text.contains("SECRET") && !text.contains("TOP-"), "{text}");
    assert!(text.is_ascii() && text.len() <= MAX_REPORT_BYTES);
    // The worst case of every bounded list stays far below the limit.
    let big = InspectReport {
        mode: InspectMode::Check,
        outcome: Outcome::Corrupt,
        reason: "snapshot_block_corrupt",
        detail: Some(u64::MAX),
        file_len: Some(u64::MAX),
        block_size: u32::MAX,
        cow_version: u32::MAX,
        layer_version: u32::MAX,
        root_generation: Some(u64::MAX),
        open_class: Some(OpenClass::PossibleLostCommit),
        other_slot: Some("damaged"),
        repair_needed: true,
        layer: Some(LayerInfo {
            kind: LayerKind::Child,
            building: true,
            file_uid: u64::MAX,
            logical_size: u64::MAX,
            disk_blocks: u64::MAX,
            depth: u32::MAX,
            next_snapshot_id: u64::MAX,
            snapshot_ids: vec![u64::MAX; crate::cow_snapshot::MAX_SNAPSHOTS],
            parent: None,
        }),
        chain: vec![
            ChainLink {
                file_uid: u64::MAX,
                snapshot_id: u64::MAX
            };
            crate::cow_snapshot::MAX_CHAIN_DEPTH as usize
        ],
        chain_verified: true,
    };
    let text = big.render();
    assert!(text.len() < MAX_REPORT_BYTES / 2, "{}", text.len());
    // A failed report (missing or busy file) renders without a layer.
    let failed = InspectReport::failed(InspectMode::Info, Outcome::Busy, "busy", None);
    assert!(failed.render().contains("outcome=busy\n"));
}
