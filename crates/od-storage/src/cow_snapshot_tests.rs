//! Tests for [`crate::cow_snapshot`] (S9-14): immutable snapshots, bounded
//! chains, read resolution and snapshot publication under fault injection.
//!
//! Oracles are plain byte models (`Vec<u8>` of the logical disk) that know
//! nothing about headers, bitmaps or maps. Files live on [`FaultHandle`]s; a
//! "reopen" is a crash (`DropPending`: only acknowledged state is kept)
//! followed by a writable open, so every reopen below is also a recovery.

use crate::cow_proto::{BLOCK_SIZE, CowFile};
use crate::cow_proto_tests::{OpKind, faults_for, is_failing, parse_kind, policies};
use crate::cow_snapshot::{
    Header, MAX_CHAIN_DEPTH, MAX_DISK_BLOCKS, MAX_SNAPSHOTS, NoParents, ParentRef, ParentResolver,
    ResolveError, SnapError, SnapFile, SnapshotRecord, decode_header, encode_header,
};
use crate::io::CheckedStorage;
use crate::testing::fault::{CrashPolicy, FaultBackend, FaultHandle};
use crate::testing::rng::SplitMix64;
use std::collections::BTreeMap;

const BS: usize = BLOCK_SIZE;

type File = SnapFile<FaultBackend>;

// ---------------------------------------------------------------- harness

/// Media of every file by uid; also the resolver's catalogue.
#[derive(Default)]
struct Fs {
    files: BTreeMap<u64, FaultHandle>,
}

impl Fs {
    fn handle(&mut self, uid: u64, seed: u64) -> FaultHandle {
        let h = FaultHandle::new(&[], seed);
        self.files.insert(uid, h.clone());
        h
    }
}

struct FsResolver<'a>(&'a Fs);

impl ParentResolver for FsResolver<'_> {
    type Backend = FaultBackend;

    fn open_parent(&mut self, p: &ParentRef) -> Result<CheckedStorage<FaultBackend>, ResolveError> {
        let h = self.0.files.get(&p.file_uid).ok_or(ResolveError::Missing)?;
        CheckedStorage::new(h.open()).map_err(ResolveError::Storage)
    }
}

fn st(h: &FaultHandle) -> CheckedStorage<FaultBackend> {
    CheckedStorage::new(h.open()).expect("capabilities are consistent")
}

fn base(fs: &mut Fs, uid: u64) -> File {
    let h = fs.handle(uid, 0x5000 + uid);
    SnapFile::create_base(st(&h), uid).expect("create base").0
}

fn child(fs: &mut Fs, uid: u64, parent: &ParentRef) -> Result<File, SnapError> {
    let h = FaultHandle::new(&[], 0x6000 + uid);
    let created = SnapFile::create_child(st(&h), &mut FsResolver(fs), parent, uid);
    // Registered only on success: a refused create must not shadow a file.
    if created.is_ok() {
        fs.files.insert(uid, h);
    }
    created.map(|(f, _)| f)
}

/// Crash (only acknowledged state survives) and writable open of file `uid`.
fn reopen(fs: &Fs, uid: u64) -> Result<File, SnapError> {
    let h = fs.files.get(&uid).expect("known file");
    h.crash(CrashPolicy::DropPending);
    SnapFile::open(st(h), &mut FsResolver(fs)).map(|(f, _)| f)
}

fn rnd(seed: u64, len: usize) -> Vec<u8> {
    SplitMix64(seed).bytes(len)
}

/// Byte model of a logical disk; always a whole number of blocks.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Model(Vec<u8>);

impl Model {
    fn apply(&mut self, off: usize, data: &[u8]) {
        let end = off + data.len();
        if self.0.len() < end {
            self.0.resize(end.next_multiple_of(BS), 0);
        }
        self.0[off..end].copy_from_slice(data);
    }
}

fn write(f: &mut File, m: &mut Model, off: usize, data: &[u8]) {
    f.write_at(off as u64, data).expect("write");
    m.apply(off, data);
}

fn image(f: &File) -> Vec<u8> {
    let mut v = vec![0u8; f.disk_size() as usize];
    f.read_at(0, &mut v).expect("read disk");
    v
}

fn snap_image(f: &File, id: u64) -> Vec<u8> {
    let blocks = f.snapshot_disk_blocks(id).expect("snapshot exists");
    let mut v = vec![0u8; blocks as usize * BS];
    f.read_snapshot_at(id, 0, &mut v).expect("read snapshot");
    v
}

/// Takes a snapshot and returns `(id, reference for a child)`.
fn snap(f: &mut File) -> (u64, ParentRef) {
    let (id, _ack) = f.create_snapshot().expect("snapshot");
    (id, f.snapshot_ref(id).expect("ref"))
}

// ----------------------------------------------------------- header codec

fn sample_header() -> Header {
    Header {
        building: false,
        file_uid: 0x1122_3344_5566_7788,
        disk_blocks: 17,
        next_snapshot_id: 4,
        depth: 2,
        parent: Some(ParentRef {
            file_uid: 9,
            snapshot_id: 2,
            generation: 7,
            block_count: 12,
            end_offset: 8192,
            map_crc: 0xDEAD_BEEF,
        }),
        snapshots: vec![
            SnapshotRecord {
                id: 1,
                generation: 3,
                block_count: 4,
                end_offset: 4096,
                map_offset: 3072,
                map_crc: 5,
            },
            SnapshotRecord {
                id: 3,
                generation: 9,
                block_count: 6,
                end_offset: 9216,
                map_offset: 7168,
                map_crc: 6,
            },
        ],
    }
}

fn recrc(b: &mut [u8; BS]) {
    let crc = crate::cow_proto::crc32(&b[..BS - 4]);
    b[BS - 4..].copy_from_slice(&crc.to_le_bytes());
}

#[test]
fn header_round_trips_and_any_single_bit_flip_is_refused() {
    let h = sample_header();
    let enc = encode_header(&h);
    assert_eq!(decode_header(&enc), Ok(h));
    for at in 0..BS {
        for bit in [0u8, 7] {
            let mut b = enc;
            b[at] ^= 1 << bit;
            assert!(
                decode_header(&b).is_err(),
                "flip at {at} bit {bit} accepted"
            );
        }
    }
}

#[test]
fn crc_valid_but_structurally_wrong_headers_are_refused() {
    let enc = encode_header(&sample_header());
    let mutate = |f: &dyn Fn(&mut [u8; BS])| {
        let mut b = enc;
        f(&mut b);
        recrc(&mut b);
        decode_header(&b)
    };
    // version, kind, uid zero, reserved, unused record, id order.
    assert!(matches!(
        mutate(&|b| b[8] = 2),
        Err(SnapError::UnsupportedHeader(_))
    ));
    assert!(matches!(
        mutate(&|b| b[12] = 7),
        Err(SnapError::UnsupportedHeader(_))
    ));
    assert!(matches!(
        mutate(&|b| b[16..24].fill(0)),
        Err(SnapError::HeaderCorrupt(_))
    ));
    assert!(matches!(
        mutate(&|b| b[490] = 1),
        Err(SnapError::HeaderCorrupt(_))
    ));
    assert!(matches!(
        mutate(&|b| b[96 + 5 * 48] = 1),
        Err(SnapError::HeaderCorrupt(_))
    ));
    // Record 1 id <= record 0 id.
    assert!(matches!(
        mutate(&|b| b[96 + 48] = 1),
        Err(SnapError::HeaderCorrupt(_))
    ));
    // Disk size above the bitmap limit.
    assert!(matches!(
        mutate(&|b| b[24..32].copy_from_slice(&(MAX_DISK_BLOCKS + 1).to_le_bytes())),
        Err(SnapError::HeaderCorrupt(_))
    ));
    // Depth above the limit is its own error.
    assert_eq!(
        mutate(&|b| b[44..48].copy_from_slice(&(MAX_CHAIN_DEPTH + 1).to_le_bytes())),
        Err(SnapError::ChainTooDeep {
            depth: MAX_CHAIN_DEPTH + 1
        })
    );
    // A base header must not carry parent data.
    let mut base_h = sample_header();
    base_h.parent = None;
    base_h.depth = 0;
    let mut b = encode_header(&base_h);
    b[48] = 1;
    recrc(&mut b);
    assert!(matches!(
        decode_header(&b),
        Err(SnapError::HeaderCorrupt(_))
    ));
}

// ------------------------------------------------- (a) create and reopen

#[test]
fn snapshot_is_reproduced_after_new_writes_and_reopen() {
    let mut fs = Fs::default();
    let mut f = base(&mut fs, 1);
    let mut m = Model::default();
    write(&mut f, &mut m, 0, &rnd(1, 3 * BS));
    write(&mut f, &mut m, 700, &rnd(2, 40));
    let frozen = m.clone();
    let (id, _) = snap(&mut f);
    assert_eq!(id, 1);
    assert_eq!(snap_image(&f, 1), frozen.0);

    write(&mut f, &mut m, 100, &rnd(3, 2 * BS)); // overwrite across blocks
    write(&mut f, &mut m, 5 * BS + 3, &rnd(4, 9)); // extend with a gap
    assert_ne!(m, frozen);
    assert_eq!(image(&f), m.0);
    assert_eq!(snap_image(&f, 1), frozen.0, "live snapshot read changed");

    drop(f);
    let f = reopen(&fs, 1).expect("reopen");
    assert_eq!(f.snapshot_ids(), vec![1]);
    assert_eq!(image(&f), m.0);
    assert_eq!(snap_image(&f, 1), frozen.0, "snapshot changed after reopen");
}

// --------------------------------------- (b) overwrite does not leak back

#[test]
fn many_overwrites_never_change_a_snapshot() {
    let mut fs = Fs::default();
    let mut f = base(&mut fs, 1);
    let mut m = Model::default();
    write(&mut f, &mut m, 0, &rnd(10, 6 * BS));
    let frozen = m.clone();
    snap(&mut f);
    let mut rng = SplitMix64(0xB0B);
    for round in 0..25u64 {
        let off = rng.below(8 * BS as u64) as usize;
        let len = 1 + rng.below(2 * BS as u64) as usize;
        write(&mut f, &mut m, off, &rnd(100 + round, len));
        assert_eq!(snap_image(&f, 1), frozen.0, "round {round}");
    }
    assert_eq!(image(&f), m.0);
    drop(f);
    let f = reopen(&fs, 1).expect("reopen");
    assert_eq!(image(&f), m.0);
    assert_eq!(snap_image(&f, 1), frozen.0);
}

// ------------------------------------------------ (c) holes and zeros

#[test]
fn child_reads_parent_data_holes_and_zeros() {
    let mut fs = Fs::default();
    let mut p = base(&mut fs, 1);
    let mut pm = Model::default();
    // Block 0 data, block 1 never written (hole), block 2 data, block 3 hole
    // by disk growth is not possible: last block is data.
    write(&mut p, &mut pm, 0, &rnd(1, BS));
    write(&mut p, &mut pm, 2 * BS, &rnd(2, BS));
    assert_eq!(&pm.0[BS..2 * BS], &[0u8; BS][..], "parent hole is zeros");
    let (_, pref) = snap(&mut p);

    let mut c = child(&mut fs, 2, &pref).expect("child");
    let mut cm = pm.clone();
    assert_eq!(c.disk_blocks(), 3);
    assert_eq!(image(&c), cm.0, "child before any write shows the parent");

    // Partial write into a parent hole: read-modify-write from zeros.
    write(&mut c, &mut cm, BS + 10, &[0xAB; 20]);
    assert_eq!(image(&c), cm.0);
    // Partial write into a parent data block keeps the rest of the parent's.
    write(&mut c, &mut cm, 20, &[0xCD; 5]);
    assert_eq!(image(&c), cm.0);
    // Growth past the parent: new blocks, and a gap of untouched zero blocks.
    write(&mut c, &mut cm, 7 * BS + 1, &[0xEE; 3]);
    assert_eq!(c.disk_blocks(), 8);
    assert!(image(&c)[3 * BS..7 * BS].iter().all(|x| *x == 0));
    assert_eq!(image(&c), cm.0);

    drop(c);
    let c = reopen(&fs, 2).expect("reopen child");
    assert_eq!(image(&c), cm.0);
    // Reads outside the disk are refused, not zero-filled.
    let mut b = [0u8; 1];
    assert_eq!(c.read_at(8 * BS as u64, &mut b), Err(SnapError::OutOfRange));
}

// ---------------------------------------------------- (d) several roots

#[test]
fn several_snapshot_roots_read_independently() {
    let mut fs = Fs::default();
    let mut f = base(&mut fs, 1);
    let mut m = Model::default();
    let mut frozen = Vec::new();
    for i in 0..MAX_SNAPSHOTS as u64 {
        write(&mut f, &mut m, (i as usize % 3) * 100, &rnd(i, BS + 50));
        let (id, _) = snap(&mut f);
        assert_eq!(id, i + 1, "ids are consecutive and monotonic");
        frozen.push((id, m.clone()));
    }
    // The table is full: a further snapshot is refused and changes nothing.
    let gen_before = f.generation();
    assert_eq!(f.create_snapshot().err(), Some(SnapError::SnapshotLimit));
    assert_eq!(f.generation(), gen_before);

    for (id, model) in &frozen {
        assert_eq!(snap_image(&f, *id), model.0, "live id {id}");
    }
    drop(f);
    let f = reopen(&fs, 1).expect("reopen");
    assert_eq!(
        f.snapshot_ids(),
        (1..=MAX_SNAPSHOTS as u64).collect::<Vec<_>>()
    );
    for (id, model) in &frozen {
        assert_eq!(snap_image(&f, *id), model.0, "reopened id {id}");
    }
    assert_eq!(
        f.read_snapshot_at(99, 0, &mut [0u8; 1]).err(),
        Some(SnapError::SnapshotMissing { id: 99 })
    );
}

#[test]
fn children_of_different_snapshot_roots_see_their_own_root() {
    let mut fs = Fs::default();
    let mut p = base(&mut fs, 1);
    let mut m = Model::default();
    write(&mut p, &mut m, 0, &rnd(1, 2 * BS));
    let (_, r1) = snap(&mut p);
    let m1 = m.clone();
    write(&mut p, &mut m, 0, &rnd(2, 2 * BS));
    let (_, r2) = snap(&mut p);
    let m2 = m.clone();
    drop(p);
    let p = reopen(&fs, 1).expect("reopen parent");
    drop(p);
    let c1 = child(&mut fs, 2, &r1).expect("child 1");
    let c2 = child(&mut fs, 3, &r2).expect("child 2");
    assert_eq!(image(&c1), m1.0);
    assert_eq!(image(&c2), m2.0);
    assert_ne!(m1, m2);
}

// ------------------------------- (e) missing, mismatch, cycle, depth

/// base(1) with data and one snapshot; returns its reference.
fn parent_with_snapshot(fs: &mut Fs) -> ParentRef {
    let mut p = base(fs, 1);
    let mut m = Model::default();
    write(&mut p, &mut m, 0, &rnd(1, 2 * BS));
    snap(&mut p).1
}

#[test]
fn missing_parent_refuses_create_and_open() {
    let mut fs = Fs::default();
    let pref = parent_with_snapshot(&mut fs);
    let mut c = child(&mut fs, 2, &pref).expect("child");
    let mut m = Model::default();
    write(&mut c, &mut m, 0, &[1u8; 4]);
    drop(c);
    fs.files.remove(&1);
    assert_eq!(reopen(&fs, 2).err(), Some(SnapError::ParentMissing));
    assert_eq!(
        child(&mut fs, 3, &pref).err(),
        Some(SnapError::ParentMissing)
    );
}

#[test]
fn replaced_parent_is_an_identity_mismatch() {
    let mut fs = Fs::default();
    let pref = parent_with_snapshot(&mut fs);
    let c = child(&mut fs, 2, &pref).expect("child");
    drop(c);
    let original = fs.files.get(&1).expect("parent").clone();

    // A different file (other uid) under the name the child knows.
    let mut other = base(&mut fs, 77);
    let mut om = Model::default();
    write(&mut other, &mut om, 0, &rnd(1, 2 * BS));
    snap(&mut other);
    let other_handle = fs.files.get(&77).expect("other").clone();
    fs.files.insert(1, other_handle);
    assert_eq!(
        reopen(&fs, 2).err(),
        Some(SnapError::ParentIdentityMismatch)
    );

    // Same uid, same snapshot id, different content: the retained map's CRC
    // is the content identity, so the uid alone is not trusted.
    let mut twin = base(&mut fs, 1);
    let mut tm = Model::default();
    write(&mut twin, &mut tm, 0, &rnd(999, 2 * BS));
    snap(&mut twin);
    assert_eq!(
        reopen(&fs, 2).err(),
        Some(SnapError::ParentIdentityMismatch)
    );

    // Same uid, the snapshot id does not exist there.
    let mut empty = base(&mut fs, 1);
    let mut em = Model::default();
    write(&mut empty, &mut em, 0, &rnd(1, 2 * BS));
    assert_eq!(
        reopen(&fs, 2).err(),
        Some(SnapError::SnapshotMissing { id: 1 })
    );

    // The real parent restored: opens again.
    fs.files.insert(1, original);
    assert!(reopen(&fs, 2).is_ok());
}

#[test]
fn tampered_parent_snapshot_map_is_refused() {
    let mut fs = Fs::default();
    let pref = parent_with_snapshot(&mut fs);
    // Move the parent forward so the retained map is not the head map.
    let mut p = reopen(&fs, 1).expect("parent");
    let mut m = Model::default();
    write(&mut p, &mut m, 0, &rnd(5, 3 * BS));
    let map_offset = p.snapshot_record(1).expect("record").map_offset as usize;
    drop(p);
    let h = fs.files.get(&1).expect("parent").clone();
    h.crash(CrashPolicy::DropPending);
    let mut bytes = h.durable_bytes();
    bytes[map_offset + 3] ^= 0x40;
    fs.files.insert(1, FaultHandle::new(&bytes, 1));
    assert_eq!(
        child(&mut fs, 2, &pref).err(),
        Some(SnapError::SnapshotCorrupt { id: 1 })
    );
}

#[test]
fn a_chain_that_repeats_a_file_identity_is_a_cycle() {
    let mut fs = Fs::default();
    // Parent naming itself, and a child whose uid equals its parent's.
    let pref = parent_with_snapshot(&mut fs);
    assert_eq!(child(&mut fs, 1, &pref).err(), Some(SnapError::ParentCycle));

    // 1 <- 2, then a file with uid 1 over 2's snapshot: the chain would be
    // 1 -> 2 -> 1.
    let mut c2 = child(&mut fs, 2, &pref).expect("child 2");
    let mut m = Model::default();
    write(&mut c2, &mut m, 0, &[7u8; 8]);
    let (_, r2) = snap(&mut c2);
    assert_eq!(child(&mut fs, 1, &r2).err(), Some(SnapError::ParentCycle));

    // The same chain forged on media (the checked constructor refuses it):
    // opening must refuse too, before reading anything from the parents.
    let h = FaultHandle::new(&[], 0xF0);
    let forged = Header {
        building: false,
        file_uid: 1,
        disk_blocks: 2,
        next_snapshot_id: 1,
        depth: 2,
        parent: Some(r2),
        snapshots: Vec::new(),
    };
    SnapFile::init(st(&h), forged, Vec::new()).expect("forge");
    // uid 1 is resolvable (the real base) but the forged file claims uid 1.
    h.crash(CrashPolicy::DropPending);
    let opened = SnapFile::open(st(&h), &mut FsResolver(&fs));
    assert_eq!(opened.err(), Some(SnapError::ParentCycle));
}

#[test]
fn chain_depth_is_bounded() {
    let mut fs = Fs::default();
    let mut f = base(&mut fs, 1);
    let mut m = Model::default();
    write(&mut f, &mut m, 0, &rnd(1, BS));
    let (_, mut pref) = snap(&mut f);
    drop(f);
    // uid 2..=9: depths 1..=MAX_CHAIN_DEPTH.
    for depth in 1..=MAX_CHAIN_DEPTH {
        let uid = 1 + u64::from(depth);
        let mut c = child(&mut fs, uid, &pref).expect("child within the limit");
        assert_eq!(c.depth(), depth);
        let mut cm = Model::default();
        cm.apply(0, &m.0);
        write(&mut c, &mut cm, 10 * depth as usize, &[depth as u8; 3]);
        m = cm;
        pref = snap(&mut c).1;
    }
    // The deepest child still reads through all eight ancestors.
    let deepest = reopen(&fs, 1 + u64::from(MAX_CHAIN_DEPTH)).expect("deepest opens");
    assert_eq!(deepest.depth(), MAX_CHAIN_DEPTH);
    assert_eq!(image(&deepest), m.0);
    drop(deepest);
    // One more would have nine ancestors: refused, nothing created.
    assert_eq!(
        child(&mut fs, 100, &pref).err(),
        Some(SnapError::ChainTooDeep {
            depth: MAX_CHAIN_DEPTH + 1
        })
    );

    // Forged: header depth over the limit is refused without a walk.
    let h = FaultHandle::new(&[], 0xF1);
    let too_deep = Header {
        building: false,
        file_uid: 200,
        disk_blocks: 1,
        next_snapshot_id: 1,
        depth: MAX_CHAIN_DEPTH + 1,
        parent: Some(pref),
        snapshots: Vec::new(),
    };
    SnapFile::init(st(&h), too_deep, Vec::new()).expect("forge");
    h.crash(CrashPolicy::DropPending);
    assert_eq!(
        SnapFile::open(st(&h), &mut FsResolver(&fs)).err(),
        Some(SnapError::ChainTooDeep {
            depth: MAX_CHAIN_DEPTH + 1
        })
    );

    // Forged: a header that lies about its depth is caught by the walk.
    let h = FaultHandle::new(&[], 0xF2);
    let liar = Header {
        building: false,
        file_uid: 201,
        disk_blocks: 1,
        next_snapshot_id: 1,
        depth: MAX_CHAIN_DEPTH,
        parent: Some(pref),
        snapshots: Vec::new(),
    };
    SnapFile::init(st(&h), liar, Vec::new()).expect("forge");
    h.crash(CrashPolicy::DropPending);
    assert_eq!(
        SnapFile::open(st(&h), &mut FsResolver(&fs)).err(),
        Some(SnapError::ChainTooDeep {
            depth: MAX_CHAIN_DEPTH + 1
        })
    );

    // Forged: depth that is within limits but disagrees with the walk.
    let h = FaultHandle::new(&[], 0xF3);
    let short = Header {
        building: false,
        file_uid: 202,
        disk_blocks: 1,
        next_snapshot_id: 1,
        depth: 2,
        parent: Some(base_ref_of_chain_head(&fs)),
        snapshots: Vec::new(),
    };
    SnapFile::init(st(&h), short, Vec::new()).expect("forge");
    h.crash(CrashPolicy::DropPending);
    assert!(matches!(
        SnapFile::open(st(&h), &mut FsResolver(&fs)).err(),
        Some(SnapError::ChainInconsistent(_))
    ));
}

/// Reference to snapshot 1 of the base file (uid 1) of a chain test.
fn base_ref_of_chain_head(fs: &Fs) -> ParentRef {
    let f = reopen(fs, 1).expect("base");
    f.snapshot_ref(1).expect("snapshot 1")
}

// -------------------------------------- (f) fault sweep on publication

struct Pub {
    handle: FaultHandle,
    file: File,
    /// Image of every acknowledged snapshot before the one being published.
    prior: Vec<(u64, Vec<u8>)>,
    /// Disk image at the moment of publication (equals the head).
    head: Vec<u8>,
}

fn publication_history(seed: u64, with_prior: bool) -> Pub {
    let handle = FaultHandle::new(&[], seed);
    let mut file = SnapFile::create_base(st(&handle), 1).expect("create").0;
    let mut m = Model::default();
    let mut prior = Vec::new();
    write(&mut file, &mut m, 0, &rnd(1, 3 * BS));
    write(&mut file, &mut m, 600, &rnd(2, 30));
    if with_prior {
        let (id, _) = snap(&mut file);
        prior.push((id, m.0.clone()));
        write(&mut file, &mut m, 10, &rnd(3, 2 * BS));
        write(&mut file, &mut m, 4 * BS, &rnd(4, BS));
    }
    Pub {
        handle,
        file,
        prior,
        head: m.0,
    }
}

fn publication_ops(with_prior: bool) -> Vec<OpKind> {
    let mut p = publication_history(0x14, with_prior);
    let start = p.handle.next_ordinal();
    p.file.create_snapshot().expect("clean publication");
    let end = p.handle.next_ordinal();
    p.handle
        .trace()
        .iter()
        .filter_map(|l| parse_kind(l))
        .filter(|(o, _)| *o >= start && *o < end)
        .map(|(_, k)| k)
        .collect()
}

#[test]
fn snapshot_publication_is_whole_or_absent_under_every_fault() {
    let mut cases = 0u64;
    let mut present_unacked = 0u64;
    let mut absent = 0u64;
    for with_prior in [false, true] {
        let ops = publication_ops(with_prior);
        assert!(ops.len() >= 6, "publication makes the commit's calls");
        let new_id = u64::from(with_prior) + 1;
        let mut scenarios: Vec<(usize, Option<crate::testing::fault::Fault>)> =
            vec![(ops.len(), None)]; // power cut after the last call
        for (i, kind) in ops.iter().enumerate() {
            for fault in faults_for(*kind) {
                scenarios.push((i, Some(fault)));
            }
        }
        for (i, fault) in scenarios {
            for policy in policies() {
                cases += 1;
                let Pub {
                    handle,
                    mut file,
                    prior,
                    head,
                } = publication_history(0x14, with_prior);
                if let Some(f) = fault {
                    handle.script(handle.next_ordinal() + i as u64, f);
                }
                let result = file.create_snapshot();
                let label = format!("prior={with_prior} op={i} fault={fault:?} policy={policy:?}");
                if fault.is_some_and(is_failing) {
                    assert!(result.is_err(), "{label}: failing fault was acknowledged");
                }
                let acked = result.is_ok();
                drop(file);
                handle.crash(policy);
                let reopened = SnapFile::open(st(&handle), &mut NoParents);
                let mut f = match reopened {
                    Ok((f, _)) => f,
                    Err(e) => panic!("{}", handle.report(&format!("{label}: refused {e:?}"))),
                };
                let old: Vec<u64> = prior.iter().map(|(id, _)| *id).collect();
                let ids = f.snapshot_ids();
                let present = ids.last() == Some(&new_id);
                let expected: Vec<u64> = if present {
                    old.iter().copied().chain([new_id]).collect()
                } else {
                    old.clone()
                };
                if ids != expected {
                    panic!("{}", handle.report(&format!("{label}: ids {ids:?}")));
                }
                if acked && !present {
                    panic!(
                        "{}",
                        handle.report(&format!("{label}: acked snapshot lost"))
                    );
                }
                if present && !acked {
                    present_unacked += 1;
                } else if !present {
                    absent += 1;
                }
                // The disk never changes, whatever happened to the record.
                assert_eq!(image(&f), head, "{label}: disk changed");
                if present {
                    assert_eq!(snap_image(&f, new_id), head, "{label}: partial snapshot");
                }
                for (id, img) in &prior {
                    assert_eq!(&snap_image(&f, *id), img, "{label}: prior {id} changed");
                }
                // The id is never reused: the next snapshot is strictly later.
                let next_expected = if present { new_id + 1 } else { new_id };
                let (id, _) = f.create_snapshot().expect("snapshot after recovery");
                assert_eq!(id, next_expected, "{label}");
            }
        }
    }
    // The sweep reached both legal outcomes of an unacknowledged publication.
    assert!(
        absent > 0 && present_unacked > 0,
        "{absent} {present_unacked}"
    );
    assert!(cases > 500, "{cases}");
}

#[test]
fn an_acknowledged_publication_makes_no_further_backend_call_and_survives_reopen() {
    let Pub {
        handle,
        mut file,
        head,
        ..
    } = publication_history(0x15, false);
    let ops = publication_ops(false).len() as u64;
    let start = handle.next_ordinal();
    // A fault just past the commit's last call must never be reached: after
    // the commit nothing can fail and turn a published snapshot into `Err`.
    handle.script(
        start + ops,
        crate::testing::fault::Fault::Write(crate::testing::fault::WriteFault::Fail(
            std::io::ErrorKind::Other,
        )),
    );
    let (id, _ack) = file.create_snapshot().expect("published snapshot is Ok");
    assert_eq!(handle.next_ordinal(), start + ops, "extra backend call");
    assert_eq!(snap_image(&file, id), head);
    drop(file);
    let f = reopen_handle(&handle).expect("reopen");
    assert_eq!(f.snapshot_ids(), vec![id]);
    assert_eq!(snap_image(&f, id), head);
}

fn reopen_handle(h: &FaultHandle) -> Result<File, SnapError> {
    h.crash(CrashPolicy::DropPending);
    SnapFile::open(st(h), &mut NoParents).map(|(f, _)| f)
}

// ------------------------- (g) child overwrite does not touch the parent

#[test]
fn child_writes_leave_every_parent_byte_unchanged() {
    let mut fs = Fs::default();
    let mut p = base(&mut fs, 1);
    let mut pm = Model::default();
    write(&mut p, &mut pm, 0, &rnd(1, 4 * BS));
    write(&mut p, &mut pm, 6 * BS, &rnd(2, BS)); // hole at blocks 4..6
    let (_, pref) = snap(&mut p);
    drop(p);
    let parent_handle = fs.files.get(&1).expect("parent").clone();
    parent_handle.crash(CrashPolicy::DropPending);
    let parent_before = parent_handle.volatile_bytes();

    let mut c = child(&mut fs, 2, &pref).expect("child");
    let mut cm = pm.clone();
    let mut rng = SplitMix64(0xCC);
    for round in 0..20u64 {
        let off = rng.below(9 * BS as u64) as usize;
        let len = 1 + rng.below(BS as u64 + 100) as usize;
        write(&mut c, &mut cm, off, &rnd(300 + round, len));
        assert_eq!(image(&c), cm.0, "round {round}");
    }
    c.create_snapshot().expect("child snapshot");
    assert_eq!(
        parent_handle.volatile_bytes(),
        parent_before,
        "parent bytes changed"
    );
    drop(c);
    assert_eq!(parent_handle.durable_bytes(), parent_before);
    // And the parent still reads exactly its frozen bytes.
    let p = reopen(&fs, 1).expect("parent reopen");
    assert_eq!(snap_image(&p, 1), pm.0);
    assert_eq!(image(&p), pm.0);
    let c = reopen(&fs, 2).expect("child reopen");
    assert_eq!(image(&c), cm.0);
}

// ------------------------------------- future parent mutations are hidden

#[test]
fn later_parent_mutations_are_not_visible_to_the_child() {
    let mut fs = Fs::default();
    let mut p = base(&mut fs, 1);
    let mut pm = Model::default();
    write(&mut p, &mut pm, 0, &rnd(1, 2 * BS));
    write(&mut p, &mut pm, 3 * BS, &rnd(2, BS)); // block 2 is a hole
    let (_, pref) = snap(&mut p);
    let frozen = pm.clone();
    drop(p);
    let c = child(&mut fs, 2, &pref).expect("child");
    drop(c);

    // The parent moves on: overwrites, fills its hole, grows, snapshots.
    let mut p = reopen(&fs, 1).expect("parent writer");
    write(&mut p, &mut pm, 0, &rnd(7, 4 * BS));
    write(&mut p, &mut pm, 9 * BS, &rnd(8, BS));
    let (new_id, _) = snap(&mut p);
    assert_eq!(new_id, 2);
    assert_ne!(pm, frozen);
    drop(p);

    // The child (new open and a brand new child) still sees only the frozen
    // state: the filled hole reads as zeros, the growth does not exist.
    let c = reopen(&fs, 2).expect("child reopen");
    assert_eq!(image(&c), frozen.0);
    assert_eq!(c.disk_blocks(), 4);
    let c3 = child(&mut fs, 3, &pref).expect("second child of the old snapshot");
    assert_eq!(image(&c3), frozen.0);
    // Parent head and snapshot 2 are different objects.
    let p = reopen(&fs, 1).expect("parent");
    assert_eq!(image(&p), pm.0);
    assert_eq!(snap_image(&p, 1), frozen.0);
}

// ------------------------------------------------ random chain vs model

#[test]
fn random_chain_matches_a_flat_model_at_every_level() {
    let mut fs = Fs::default();
    let mut rng = SplitMix64(0x5EED);
    let mut f = base(&mut fs, 1);
    let mut m = Model::default();
    let mut frozen: Vec<(u64, u64, Model)> = Vec::new(); // (uid, snapshot id, image)
    let levels = 5u64;
    for level in 1..=levels {
        for _ in 0..8 {
            let off = rng.below(10 * BS as u64) as usize;
            let len = 1 + rng.below(BS as u64 * 2) as usize;
            let data = rng.bytes(len);
            write(&mut f, &mut m, off, &data);
        }
        let (id, r) = snap(&mut f);
        frozen.push((level, id, m.clone()));
        drop(f);
        if level < levels {
            f = child(&mut fs, level + 1, &r).expect("child");
        } else {
            f = reopen(&fs, level).expect("reopen last level");
        }
    }
    assert_eq!(image(&f), m.0);
    drop(f);
    // Every level's snapshot, read through its own chain, equals its model.
    for (uid, id, model) in &frozen {
        let f = reopen(&fs, *uid).expect("level");
        assert_eq!(snap_image(&f, *id), model.0, "level {uid}");
        assert_eq!(f.depth() as u64, uid - 1);
    }
}

// ------------------------------------------------------ write contract

#[test]
fn write_bounds_are_checked_before_any_backend_call() {
    let mut fs = Fs::default();
    let mut f = base(&mut fs, 1);
    let h = fs.files.get(&1).expect("handle").clone();
    let ord = h.next_ordinal();
    assert_eq!(
        f.write_at(0, &[]).err(),
        Some(SnapError::BadWrite("empty write"))
    );
    assert_eq!(
        f.write_at(MAX_DISK_BLOCKS * BS as u64, &[1]).err(),
        Some(SnapError::OutOfRange)
    );
    assert_eq!(
        f.write_at(u64::MAX, &[1, 2]).err(),
        Some(SnapError::OutOfRange)
    );
    // A write far past the end needs a gap of placeholders above the tx limit.
    let far = (crate::cow_proto::MAX_TX_BLOCKS as u64 + 5) * BS as u64;
    assert!(matches!(
        f.write_at(far, &[1]).err(),
        Some(SnapError::TooLarge { .. })
    ));
    assert_eq!(
        h.next_ordinal(),
        ord,
        "a rejected write touched the backend"
    );
    assert!(!f.is_poisoned());
    // A full-size write below the limit works and the file is intact.
    let big = rnd(9, 200 * BS);
    f.write_at(0, &big).expect("big write");
    assert_eq!(image(&f), big);
}

#[test]
fn a_plain_cow_file_or_an_uninitialised_layer_is_not_a_snapshot_file() {
    // S9-11 style file: block 0 is user data.
    let h = FaultHandle::new(&[], 1);
    let (mut cow, _) = CowFile::create(st(&h)).expect("create");
    cow.commit(&[(0, &[0x11u8; BS][..]), (1, &[0x22u8; BS][..])])
        .expect("commit");
    drop(cow);
    h.crash(CrashPolicy::DropPending);
    assert!(matches!(
        SnapFile::open(st(&h), &mut NoParents).err(),
        Some(SnapError::NotSnapFile(_))
    ));
    // Created but never initialised (crash between create and first commit).
    let h = FaultHandle::new(&[], 2);
    drop(CowFile::create(st(&h)).expect("create"));
    h.crash(CrashPolicy::DropPending);
    assert!(matches!(
        SnapFile::open(st(&h), &mut NoParents).err(),
        Some(SnapError::NotSnapFile(_))
    ));
    // Uninitialised, but CoW-valid and non-empty with a stray block.
    let h = FaultHandle::new(&[], 3);
    let (mut cow, _) = CowFile::create(st(&h)).expect("create");
    cow.commit(&[(0, &[0u8; BS][..])]).expect("commit");
    drop(cow);
    h.crash(CrashPolicy::DropPending);
    assert!(matches!(
        SnapFile::open(st(&h), &mut NoParents).err(),
        Some(SnapError::NotSnapFile(_))
    ));
}

#[test]
fn a_child_file_cannot_open_without_its_parent_resolver() {
    let mut fs = Fs::default();
    let pref = parent_with_snapshot(&mut fs);
    let c = child(&mut fs, 2, &pref).expect("child");
    drop(c);
    let h = fs.files.get(&2).expect("child").clone();
    h.crash(CrashPolicy::DropPending);
    assert_eq!(
        SnapFile::open(st(&h), &mut NoParents).err(),
        Some(SnapError::ParentMissing)
    );
}

// ------------------------------------------------ S9-15: branches
//
// Everything here is the logical disk layer. Guest RAM, vTPM and UEFI state
// are not modelled and cannot be touched by it: a branch is never a full-VM
// restore.

/// base(1) with three blocks and a snapshot; child(2) over it with snapshots 1
/// and 2 and one more (unpublished) write.
struct Branches {
    fs: Fs,
    source: File,
    /// `(snapshot id, logical bytes)` of the source's snapshots.
    images: Vec<(u64, Vec<u8>)>,
    /// Logical bytes of the source's current (unsnapshotted) state.
    head: Vec<u8>,
}

fn branch_world() -> Branches {
    let mut fs = Fs::default();
    let mut b = base(&mut fs, 1);
    let mut m = Model::default();
    write(&mut b, &mut m, 0, &rnd(1, 3 * BS));
    let (_, pref) = snap(&mut b);
    drop(b);
    let mut c = child(&mut fs, 2, &pref).expect("source child");
    write(&mut c, &mut m, 600, &rnd(2, 30));
    let mut images = vec![(snap(&mut c).0, m.0.clone())];
    write(&mut c, &mut m, BS + 5, &rnd(3, 2 * BS));
    images.push((snap(&mut c).0, m.0.clone()));
    write(&mut c, &mut m, 0, &rnd(4, 10));
    assert_ne!(images[1].1, m.0);
    Branches {
        fs,
        source: c,
        images,
        head: m.0,
    }
}

/// Branches from `id` of `source` into a fresh file registered as `uid`.
fn branch(fs: &mut Fs, source: &File, id: u64, uid: u64) -> Result<File, SnapError> {
    let h = FaultHandle::new(&[], 0x7000 + uid);
    let made = SnapFile::branch_from(st(&h), &mut FsResolver(fs), source, id, uid);
    if made.is_ok() {
        fs.files.insert(uid, h);
    }
    made.map(|(f, _)| f)
}

fn media(fs: &Fs, uid: u64) -> (Vec<u8>, Vec<u8>) {
    let h = fs.files.get(&uid).expect("known file");
    (h.volatile_bytes(), h.durable_bytes())
}

#[test]
fn a_branch_reproduces_the_snapshot_and_leaves_source_and_siblings_unchanged() {
    let Branches {
        mut fs,
        source,
        images,
        head,
    } = branch_world();
    let before: Vec<_> = [1, 2].iter().map(|u| media(&fs, *u)).collect();

    let (id1, img1) = &images[0];
    let (id2, img2) = &images[1];
    let mut b3 = branch(&mut fs, &source, *id1, 3).expect("branch from snapshot 1");
    let b4 = branch(&mut fs, &source, *id2, 4).expect("branch from snapshot 2");
    // (a) the logical bytes of the chosen snapshot, not of the live state.
    assert_eq!(&image(&b3), img1);
    assert_eq!(&image(&b4), img2);
    assert_ne!(image(&b3), head);
    assert_eq!(b3.parent_ref(), source.snapshot_ref(*id1));
    assert_eq!(b3.depth(), 2);
    assert!(
        b3.snapshot_ids().is_empty(),
        "a branch starts with no snapshot"
    );

    // (b) creating branches wrote nothing to the source chain.
    for (i, uid) in [1u64, 2].iter().enumerate() {
        assert_eq!(media(&fs, *uid), before[i], "file {uid} changed");
    }
    assert_eq!(image(&source), head);
    assert_eq!(&snap_image(&source, *id1), img1);

    // Writes into one branch change neither the source nor the other branch.
    let mut m3 = Model(img1.clone());
    write(&mut b3, &mut m3, 40, &rnd(9, 3 * BS));
    assert_eq!(image(&b3), m3.0);
    assert_eq!(&image(&b4), img2);
    assert_eq!(image(&source), head);
    for (i, uid) in [1u64, 2].iter().enumerate() {
        assert_eq!(media(&fs, *uid), before[i], "file {uid} changed by a write");
    }
    let b4_media = media(&fs, 4);
    write(&mut b3, &mut m3, 0, &rnd(10, 5));
    assert_eq!(media(&fs, 4), b4_media, "a sibling's bytes changed");

    // After a crash and recovery every file reads as before.
    drop((b3, b4, source));
    assert_eq!(image(&reopen(&fs, 3).expect("b3")), m3.0);
    assert_eq!(&image(&reopen(&fs, 4).expect("b4")), img2);
    let s = reopen(&fs, 2).expect("source");
    assert_eq!(image(&s), head);
    assert_eq!(s.snapshot_ids(), vec![*id1, *id2]);
}

#[test]
fn rollback_is_a_new_branch_and_the_current_file_stays() {
    let Branches {
        mut fs,
        source,
        images,
        head,
    } = branch_world();
    let (id1, img1) = images[0].clone();
    let before = media(&fs, 2);
    let back = branch(&mut fs, &source, id1, 3).expect("rollback branch");
    assert_eq!(image(&back), img1);
    assert_ne!(image(&back), head, "rolled back to older bytes");
    assert_eq!(back.parent_ref().map(|p| p.snapshot_id), Some(id1));
    assert_eq!(back.parent_ref().map(|p| p.file_uid), Some(2));
    // The file that was rolled back from is still there, whole and writable.
    assert_eq!(media(&fs, 2), before);
    drop((back, source));
    let mut cur = reopen(&fs, 2).expect("current file");
    assert_eq!(image(&cur), head);
    let mut m = Model(head);
    write(&mut cur, &mut m, 3, &[1, 2, 3]);
    assert_eq!(image(&cur), m.0);
    // A branch of a branch: a rollback of the rollback.
    let mut back = reopen(&fs, 3).expect("branch reopens");
    let (rid, rref) = snap(&mut back);
    let again = branch(&mut fs, &back, rid, 4).expect("branch of a branch");
    assert_eq!(again.depth(), 3);
    assert_eq!(again.parent_ref(), Some(rref));
    assert_eq!(image(&again), img1);
}

#[test]
fn branch_refusals_write_nothing() {
    let Branches {
        mut fs,
        mut source,
        images,
        ..
    } = branch_world();
    let id1 = images[0].0;
    let fresh = |uid: u64| FaultHandle::new(&[], 0x7500 + uid);

    // Unknown snapshot.
    let h = fresh(3);
    let r = SnapFile::branch_from(st(&h), &mut FsResolver(&fs), &source, 99, 3);
    assert_eq!(r.err(), Some(SnapError::SnapshotMissing { id: 99 }));
    assert_eq!(h.next_ordinal(), 0);
    // The new file would repeat an identity of the chain.
    for uid in [1, 2] {
        let h = fresh(uid);
        let r = SnapFile::branch_from(st(&h), &mut FsResolver(&fs), &source, id1, uid);
        assert_eq!(r.err(), Some(SnapError::ParentCycle), "uid {uid}");
        assert_eq!(h.next_ordinal(), 0);
    }
    // Uid zero is never an identity.
    let h = fresh(0);
    let r = SnapFile::branch_from(st(&h), &mut FsResolver(&fs), &source, id1, 0);
    assert!(matches!(r.err(), Some(SnapError::BadWrite(_))));
    assert_eq!(h.next_ordinal(), 0);
    // The source's parent has gone: the whole chain must verify.
    let gone = fs.files.remove(&1);
    let h = fresh(3);
    let r = SnapFile::branch_from(st(&h), &mut FsResolver(&fs), &source, id1, 3);
    assert_eq!(r.err(), Some(SnapError::ParentMissing));
    assert_eq!(h.next_ordinal(), 0);
    fs.files.extend(gone.map(|g| (1, g)));
    // A poisoned source (failed commit, unknown durable state) is refused.
    let handle = fs.files.get(&2).expect("source media").clone();
    handle.script_next(crate::testing::fault::Fault::Resize(
        std::io::ErrorKind::Other,
    ));
    assert!(source.write_at(0, &[1]).is_err());
    assert!(source.is_poisoned());
    let h = fresh(3);
    let r = SnapFile::branch_from(st(&h), &mut FsResolver(&fs), &source, id1, 3);
    assert!(matches!(r.err(), Some(SnapError::BadWrite(_))));
    assert_eq!(h.next_ordinal(), 0);
}

/// Resolver standing for "a live writer holds this file".
struct BusyResolver;

impl ParentResolver for BusyResolver {
    type Backend = FaultBackend;

    fn open_parent(&mut self, _: &ParentRef) -> Result<CheckedStorage<FaultBackend>, ResolveError> {
        Err(ResolveError::Busy)
    }
}

#[test]
fn live_writer_refusal_for_branch_and_child_open() {
    let Branches {
        fs, source, images, ..
    } = branch_world();
    let id1 = images[0].0;

    // Branch from a source a writer holds: refused, nothing written.
    let h = FaultHandle::new(&[], 0x7600);
    let r = SnapFile::branch_from(st(&h), &mut BusyResolver, &source, id1, 3);
    assert_eq!(r.err(), Some(SnapError::ParentBusy));
    assert_eq!(h.next_ordinal(), 0);

    // Writable open of a child whose parent a writer holds: refused before
    // any write, the child's media is as it was.
    drop(source);
    let ch = fs.files.get(&2).expect("child").clone();
    ch.crash(CrashPolicy::DropPending);
    let bytes = (ch.volatile_bytes(), ch.durable_bytes());
    let ops = ch.next_ordinal();
    assert_eq!(
        SnapFile::open(st(&ch), &mut BusyResolver).err(),
        Some(SnapError::ParentBusy)
    );
    assert_eq!(bytes, (ch.volatile_bytes(), ch.durable_bytes()));
    assert_eq!(ch.next_ordinal(), ops, "a mutating call was made");
}

#[test]
fn branch_creation_is_whole_or_absent_under_every_fault() {
    let ops: Vec<OpKind> = {
        let w = branch_world();
        let h = FaultHandle::new(&[], 0x7100);
        SnapFile::branch_from(st(&h), &mut FsResolver(&w.fs), &w.source, w.images[0].0, 3)
            .expect("clean branch");
        h.trace()
            .iter()
            .filter_map(|l| parse_kind(l))
            .map(|(_, k)| k)
            .collect()
    };
    assert!(ops.len() >= 6, "creation makes the commit's calls");
    let mut scenarios: Vec<(usize, Option<crate::testing::fault::Fault>)> = vec![(ops.len(), None)];
    for (i, kind) in ops.iter().enumerate() {
        for fault in faults_for(*kind) {
            scenarios.push((i, Some(fault)));
        }
    }
    let (mut present, mut absent, mut cases) = (0u64, 0u64, 0u64);
    for (i, fault) in scenarios {
        for policy in policies() {
            cases += 1;
            let w = branch_world();
            let id = w.images[0].0;
            let want = w.images[0].1.clone();
            let src_before = [media(&w.fs, 1), media(&w.fs, 2)];
            let h = FaultHandle::new(&[], 0x7100);
            if let Some(f) = fault {
                h.script(h.next_ordinal() + i as u64, f);
            }
            let made = SnapFile::branch_from(st(&h), &mut FsResolver(&w.fs), &w.source, id, 3);
            let label = format!("op={i} fault={fault:?} policy={policy:?}");
            if fault.is_some_and(is_failing) {
                assert!(made.is_err(), "{label}: failing fault was acknowledged");
            }
            let acked = made.is_ok();
            drop(made);
            h.crash(policy);
            match SnapFile::open(st(&h), &mut FsResolver(&w.fs)) {
                Ok((f, _)) => {
                    // The whole branch: the snapshot's bytes under the right
                    // identity, nothing of its own yet.
                    present += 1;
                    assert_eq!(image(&f), want, "{label}: partial branch");
                    assert_eq!(f.file_uid(), 3, "{label}");
                    assert_eq!(f.parent_ref(), w.source.snapshot_ref(id), "{label}");
                    assert!(f.snapshot_ids().is_empty(), "{label}");
                }
                Err(e) => {
                    // Absent: the file is not a valid layer file at all.
                    absent += 1;
                    assert!(!acked, "{label}: acknowledged branch lost: {e:?}");
                    assert!(
                        matches!(
                            e,
                            SnapError::NotSnapFile(_)
                                | SnapError::Cow(crate::cow_proto::CowError::NoValidRoot { .. })
                                | SnapError::Cow(
                                    crate::cow_proto::CowError::IncompleteCreate { .. }
                                )
                        ),
                        "{label}: {e:?}"
                    );
                }
            }
            // The source child is not damaged whatever happened to the branch.
            assert_eq!(src_before[0], media(&w.fs, 1), "{label}: base changed");
            assert_eq!(src_before[1], media(&w.fs, 2), "{label}: source changed");
            assert_eq!(image(&w.source), w.head, "{label}");
            for (sid, img) in &w.images {
                assert_eq!(&snap_image(&w.source, *sid), img, "{label}");
            }
        }
    }
    assert!(present > 0 && absent > 0, "{present} {absent}");
    assert!(cases > 500, "{cases}");
}

#[test]
fn a_branch_can_be_cut_from_a_closed_file_through_its_snapshot_reference() {
    let Branches {
        mut fs,
        source,
        images,
        ..
    } = branch_world();
    let (id2, img2) = images[1].clone();
    let want = source.snapshot_ref(id2).expect("open ref");
    drop(source);
    let h = fs.files.get(&2).expect("source").clone();
    h.crash(CrashPolicy::DropPending);
    let before = media(&fs, 2);
    let got = SnapFile::inspect_snapshot_ref(st(&h), id2).expect("closed ref");
    assert_eq!(got, want);
    assert_eq!(media(&fs, 2), before, "inspection must not write");
    assert_eq!(
        SnapFile::inspect_snapshot_ref(st(&h), 99).err(),
        Some(SnapError::SnapshotMissing { id: 99 })
    );
    let b = child(&mut fs, 3, &got).expect("branch from the closed file");
    assert_eq!(image(&b), img2);
}

// ------------------------------ S9-16: chain validation before writing

/// Header of a forged layer over `parent` (checked constructors refuse these).
fn forged_layer(fs: &mut Fs, uid: u64, parent: ParentRef, depth: u32) -> (File, ParentRef) {
    let h = FaultHandle::new(&[], 0x8000 + uid);
    fs.files.insert(uid, h.clone());
    let header = Header {
        building: false,
        file_uid: uid,
        disk_blocks: 2,
        next_snapshot_id: 1,
        depth,
        parent: Some(parent),
        snapshots: Vec::new(),
    };
    let mut f = SnapFile::init(st(&h), header, Vec::new()).expect("forge").0;
    let mut m = Model(vec![0; 2 * BS]);
    write(&mut f, &mut m, 7, &[0xEE; 5]);
    let r = snap(&mut f).1;
    (f, r)
}

#[test]
fn a_wrong_map_crc_of_an_intermediate_ancestor_refuses_before_any_write() {
    let mut fs = Fs::default();
    let pref = parent_with_snapshot(&mut fs);

    // Control: an intermediate layer 2 with an honest link to the base works.
    let (honest, r_ok) = forged_layer(&mut fs, 2, pref, 1);
    drop(honest);
    assert!(child(&mut fs, 3, &r_ok).is_ok());

    // The same layer whose link to the base carries a wrong map CRC. Its own
    // snapshot reference is perfectly valid, so only a check of the
    // grandparent's identity can refuse the chain.
    let bad_link = ParentRef {
        map_crc: pref.map_crc ^ 1,
        ..pref
    };
    let (liar, r_bad) = forged_layer(&mut fs, 4, bad_link, 1);
    drop(liar);
    let h = FaultHandle::new(&[], 0x8100);
    let made = SnapFile::create_child(st(&h), &mut FsResolver(&fs), &r_bad, 5);
    assert_eq!(made.err(), Some(SnapError::ParentIdentityMismatch));
    assert!(
        h.volatile_bytes().is_empty() && h.next_ordinal() == 0,
        "wrote before refusing"
    );

    // The same on open: a child file recorded over the lying layer.
    let h = FaultHandle::new(&[], 0x8101);
    let top = Header {
        building: false,
        file_uid: 5,
        disk_blocks: 2,
        next_snapshot_id: 1,
        depth: 2,
        parent: Some(r_bad),
        snapshots: Vec::new(),
    };
    SnapFile::init(st(&h), top, Vec::new()).expect("forge top");
    h.crash(CrashPolicy::DropPending);
    let bytes = h.durable_bytes();
    assert_eq!(
        SnapFile::open(st(&h), &mut FsResolver(&fs)).err(),
        Some(SnapError::ParentIdentityMismatch)
    );
    assert_eq!(h.durable_bytes(), bytes);

    // Damage deeper than the nearest parent: the base's retained map.
    let mut c3 = reopen(&fs, 3).expect("honest chain opens");
    let (_, r3) = snap(&mut c3);
    drop(c3);
    let c6 = child(&mut fs, 6, &r3).expect("depth 3");
    assert_eq!(c6.depth(), 3);
    drop(c6);
    let map_offset = {
        let b = reopen(&fs, 1).expect("base");
        b.snapshot_record(1).expect("record").map_offset as usize
    };
    let base_h = fs.files.get(&1).expect("base").clone();
    base_h.crash(CrashPolicy::DropPending);
    let mut tampered = base_h.durable_bytes();
    tampered[map_offset + 3] ^= 0x40;
    fs.files.insert(1, FaultHandle::new(&tampered, 1));
    let six = fs.files.get(&6).expect("child 6").clone();
    six.crash(CrashPolicy::DropPending);
    let bytes = six.durable_bytes();
    assert_eq!(
        reopen(&fs, 6).err(),
        Some(SnapError::SnapshotCorrupt { id: 1 })
    );
    assert_eq!(six.durable_bytes(), bytes);
}

#[test]
fn a_refused_open_does_not_repair_the_childs_damaged_root_slot() {
    use crate::cow_proto::{SLOT_SIZE, slot_offset};
    let mut fs = Fs::default();
    let pref = parent_with_snapshot(&mut fs);
    let c = child(&mut fs, 2, &pref).expect("child");
    drop(c);
    // The child's last acknowledged commit (generation 3) loses its root slot;
    // the trailing root copy is intact, so a writable open would roll forward.
    let h = fs.files.get(&2).expect("child").clone();
    h.crash(CrashPolicy::DropPending);
    let mut damaged = h.durable_bytes();
    let at = slot_offset(3) as usize;
    damaged[at..at + SLOT_SIZE].fill(0);

    let open = |fs: &Fs| {
        let dh = FaultHandle::new(&damaged, 0x77);
        let r = SnapFile::open(st(&dh), &mut FsResolver(fs));
        (r, dh)
    };
    // Control: with a sound chain the open repairs the slot (one write).
    let (ok, dh) = open(&fs);
    let (_, report) = ok.expect("opens");
    assert!(report.rolled_forward);
    assert_ne!(dh.durable_bytes(), damaged, "repair wrote the slot");

    // Broken chain (parent gone, or a different file): refused with the
    // evidence untouched.
    let mut gone = Fs::default();
    gone.files.extend(
        fs.files
            .iter()
            .filter(|(u, _)| **u != 1)
            .map(|(u, h)| (*u, h.clone())),
    );
    let (r, dh) = open(&gone);
    assert_eq!(r.err(), Some(SnapError::ParentMissing));
    assert_eq!(dh.durable_bytes(), damaged);
    assert_eq!(dh.volatile_bytes(), damaged);
    let mut other = base(&mut gone, 1);
    let mut om = Model::default();
    write(&mut other, &mut om, 0, &rnd(999, 2 * BS));
    snap(&mut other);
    let (r, dh) = open(&gone);
    assert_eq!(r.err(), Some(SnapError::ParentIdentityMismatch));
    assert_eq!(dh.durable_bytes(), damaged);
}

#[test]
fn every_wrong_field_of_a_parent_reference_refuses_create_and_open() {
    let mut fs = Fs::default();
    let pref = parent_with_snapshot(&mut fs);
    let cases: Vec<(&str, ParentRef, SnapError)> = vec![
        (
            "generation",
            ParentRef {
                generation: pref.generation + 2,
                ..pref
            },
            SnapError::ParentIdentityMismatch,
        ),
        (
            "block count",
            ParentRef {
                block_count: pref.block_count + 1,
                ..pref
            },
            SnapError::ParentIdentityMismatch,
        ),
        (
            "end offset",
            ParentRef {
                end_offset: pref.end_offset + 512,
                ..pref
            },
            SnapError::ParentIdentityMismatch,
        ),
        (
            "map crc",
            ParentRef {
                map_crc: pref.map_crc ^ 0x8000_0000,
                ..pref
            },
            SnapError::ParentIdentityMismatch,
        ),
        (
            "snapshot id",
            ParentRef {
                snapshot_id: 9,
                ..pref
            },
            SnapError::SnapshotMissing { id: 9 },
        ),
        (
            "file uid",
            ParentRef {
                file_uid: 42,
                ..pref
            },
            SnapError::ParentMissing,
        ),
    ];
    for (name, bad, want) in cases {
        let h = FaultHandle::new(&[], 0x8200);
        let made = SnapFile::create_child(st(&h), &mut FsResolver(&fs), &bad, 2);
        assert_eq!(made.err().as_ref(), Some(&want), "create: {name}");
        assert_eq!(h.next_ordinal(), 0, "create wrote: {name}");

        let h = FaultHandle::new(&[], 0x8201);
        let header = Header {
            building: false,
            file_uid: 2,
            disk_blocks: 2,
            next_snapshot_id: 1,
            depth: 1,
            parent: Some(bad),
            snapshots: Vec::new(),
        };
        SnapFile::init(st(&h), header, Vec::new()).expect("forge");
        h.crash(CrashPolicy::DropPending);
        let bytes = h.durable_bytes();
        let opened = SnapFile::open(st(&h), &mut FsResolver(&fs));
        assert_eq!(opened.err().as_ref(), Some(&want), "open: {name}");
        assert_eq!(h.durable_bytes(), bytes, "open wrote: {name}");
    }
}

// ======================================================== S9-17: delete,
// rootset and copy-compaction

fn is_absent(e: &SnapError) -> bool {
    matches!(
        e,
        SnapError::NotSnapFile(_)
            | SnapError::BuildInProgress
            | SnapError::Cow(crate::cow_proto::CowError::NoValidRoot { .. })
            | SnapError::Cow(crate::cow_proto::CowError::IncompleteCreate { .. })
    )
}

/// Base file with snapshots 1 (image A), 2 (A+B) and a head with more writes.
struct Hist {
    file: File,
    /// (id, image) of every named snapshot.
    snaps: Vec<(u64, Vec<u8>)>,
    head: Vec<u8>,
}

fn history(h: &FaultHandle) -> Hist {
    let mut file = SnapFile::create_base(st(h), 1).expect("create").0;
    let mut m = Model::default();
    let mut snaps = Vec::new();
    write(&mut file, &mut m, 0, &rnd(1, 3 * BS));
    snaps.push((snap(&mut file).0, m.0.clone()));
    write(&mut file, &mut m, 2 * BS + 10, &rnd(2, 600));
    write(&mut file, &mut m, 0, &rnd(3, BS)); // overwrite a snapshotted block
    snaps.push((snap(&mut file).0, m.0.clone()));
    write(&mut file, &mut m, 5 * BS + 4, &rnd(4, 50)); // gap: blocks 4 is a hole
    Hist {
        file,
        snaps,
        head: m.0,
    }
}

// ------------------------------------------------------------ (a) delete

#[test]
fn deleting_a_snapshot_removes_its_name_and_frees_no_bytes() {
    let mut fs = Fs::default();
    let h = fs.handle(1, 0x1700);
    let Hist {
        mut file,
        snaps,
        head,
    } = history(&h);
    let (_, third) = {
        let mut m = Model(head.clone());
        write(&mut file, &mut m, 0, &rnd(9, 2 * BS));
        let id = snap(&mut file);
        (id.0, m.0)
    };
    assert_eq!(file.snapshot_ids(), vec![1, 2, 3]);
    let before = h.durable_bytes();

    file.delete_snapshot(2).expect("delete");
    assert_eq!(file.snapshot_ids(), vec![1, 3]);
    assert_eq!(
        file.read_snapshot_at(2, 0, &mut [0u8; 1]).err(),
        Some(SnapError::SnapshotMissing { id: 2 })
    );
    assert_eq!(
        file.delete_snapshot(2).err(),
        Some(SnapError::SnapshotMissing { id: 2 })
    );
    assert_eq!(
        file.delete_snapshot(99).err(),
        Some(SnapError::SnapshotMissing { id: 99 })
    );
    // Nothing was freed or rewritten: every byte past the two root slots that
    // existed before the delete is unchanged (the file only grew).
    let after = h.durable_bytes();
    assert!(after.len() > before.len());
    assert_eq!(&after[1024..before.len()], &before[1024..]);

    drop(file);
    let mut file = reopen(&fs, 1).expect("reopen");
    assert_eq!(file.snapshot_ids(), vec![1, 3]);
    assert_eq!(snap_image(&file, 1), snaps[0].1);
    assert_eq!(snap_image(&file, 3), third);
    assert_ne!(snaps[1].1, third);
    // Head after snapshot 3 included the extra overwrite.
    assert_eq!(image(&file)[..2 * BS], third[..2 * BS]);
    // Ids are never reused, also after deleting the newest one.
    file.delete_snapshot(3).expect("delete newest");
    let (id, _) = file.create_snapshot().expect("snapshot");
    assert_eq!(id, 4);
    drop(file);
    let file = reopen(&fs, 1).expect("reopen");
    assert_eq!(file.snapshot_ids(), vec![1, 4]);
    assert_eq!(snap_image(&file, 1), snaps[0].1);
}

// -------------------------------------------- (b) delete fault sweep

fn delete_ops() -> Vec<OpKind> {
    let h = FaultHandle::new(&[], 0x1701);
    let mut hist = history(&h);
    let start = h.next_ordinal();
    hist.file.delete_snapshot(1).expect("clean delete");
    let end = h.next_ordinal();
    h.trace()
        .iter()
        .filter_map(|l| parse_kind(l))
        .filter(|(o, _)| *o >= start && *o < end)
        .map(|(_, k)| k)
        .collect()
}

#[test]
fn snapshot_delete_is_whole_or_absent_and_never_damages_data_under_every_fault() {
    let ops = delete_ops();
    assert!(ops.len() >= 6);
    let mut scenarios: Vec<(usize, Option<crate::testing::fault::Fault>)> = vec![(ops.len(), None)];
    for (i, kind) in ops.iter().enumerate() {
        for fault in faults_for(*kind) {
            scenarios.push((i, Some(fault)));
        }
    }
    let (mut gone_unacked, mut kept, mut cases) = (0u64, 0u64, 0u64);
    for (i, fault) in scenarios {
        for policy in policies() {
            cases += 1;
            let h = FaultHandle::new(&[], 0x1701);
            let Hist {
                mut file,
                snaps,
                head,
            } = history(&h);
            if let Some(f) = fault {
                h.script(h.next_ordinal() + i as u64, f);
            }
            let result = file.delete_snapshot(1);
            let label = format!("op={i} fault={fault:?} policy={policy:?}");
            if fault.is_some_and(is_failing) {
                assert!(result.is_err(), "{label}: failing fault was acknowledged");
            }
            let acked = result.is_ok();
            drop(file);
            h.crash(policy);
            let mut f = match SnapFile::open(st(&h), &mut NoParents) {
                Ok((f, _)) => f,
                Err(e) => panic!("{}", h.report(&format!("{label}: refused {e:?}"))),
            };
            let ids = f.snapshot_ids();
            let deleted = ids == vec![2];
            if ids != vec![2] && ids != vec![1, 2] {
                panic!("{}", h.report(&format!("{label}: ids {ids:?}")));
            }
            if acked && !deleted {
                panic!("{}", h.report(&format!("{label}: acked delete undone")));
            }
            if deleted && !acked {
                gone_unacked += 1;
            }
            if !deleted {
                kept += 1;
                // Still whole: the snapshot reads its exact bytes.
                assert_eq!(snap_image(&f, 1), snaps[0].1, "{label}: partial snapshot");
            }
            assert_eq!(snap_image(&f, 2), snaps[1].1, "{label}: other snapshot");
            assert_eq!(image(&f), head, "{label}: head damaged");
            // Ids are not reused whatever happened.
            let (id, _) = f.create_snapshot().expect("snapshot after recovery");
            assert_eq!(id, 3, "{label}");
        }
    }
    assert!(gone_unacked > 0 && kept > 0, "{gone_unacked} {kept}");
    assert!(cases > 400, "{cases}");
}

// ------------------------------------------------------ (d) rootset

fn overwritten_world(h: &FaultHandle) -> (File, Vec<u8>) {
    let mut f = SnapFile::create_base(st(h), 1).expect("create").0;
    let mut m = Model::default();
    write(&mut f, &mut m, 0, &rnd(1, 3 * BS)); // A
    let a = m.0.clone();
    snap(&mut f);
    write(&mut f, &mut m, 0, &rnd(2, 3 * BS)); // B: every block of A replaced
    (f, a)
}

#[test]
fn a_deleted_snapshot_stays_in_the_rootset_while_the_previous_root_names_it() {
    let h = FaultHandle::new(&[], 0x1702);
    let (mut f, _a) = overwritten_world(&h);
    let a_blocks = f.snapshot_data_blocks(1).expect("snapshot blocks");
    assert_eq!(a_blocks.len(), 3);
    let before = f.rootset().expect("rootset");
    assert!(a_blocks.is_subset(&before.data_blocks));
    assert_eq!(
        before.snapshot_ids.iter().copied().collect::<Vec<_>>(),
        vec![1]
    );

    f.delete_snapshot(1).expect("delete");
    let after = f.rootset().expect("rootset");
    // The current root no longer names the snapshot, the previous one does:
    // its bytes are still reachable and so must not be reclaimed.
    assert!(f.snapshot_ids().is_empty());
    assert!(
        a_blocks.is_subset(&after.data_blocks),
        "previous root lost blocks"
    );
    assert_eq!(
        after.snapshot_ids.iter().copied().collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(after.generations, [f.generation(), f.generation() - 1]);
    // The rootset never shrinks by a delete alone.
    assert!(before.data_blocks.is_subset(&after.data_blocks));

    // One more commit: now neither recoverable root names it.
    f.write_at(0, &[7u8; 10]).expect("write");
    let later = f.rootset().expect("rootset");
    assert!(later.snapshot_ids.is_empty());
    assert!(
        a_blocks.is_disjoint(&later.data_blocks),
        "provably unreachable blocks are still held"
    );
    assert!(later.live_bytes() < after.live_bytes());
}

#[test]
fn rootset_covers_head_snapshots_both_roots_and_clone_parents() {
    let mut fs = Fs::default();
    let h = fs.handle(1, 0x1703);
    let Hist {
        mut file, snaps, ..
    } = history(&h);
    let rs = file.rootset().expect("rootset");
    assert_eq!(
        rs.snapshot_ids.iter().copied().collect::<Vec<_>>(),
        vec![1, 2]
    );
    for (id, _) in &snaps {
        let blocks = file.snapshot_data_blocks(*id).expect("blocks");
        assert!(blocks.is_subset(&rs.data_blocks), "snapshot {id}");
    }
    assert!(rs.parents.is_empty());
    let (_, pref) = snap(&mut file);
    drop(file);
    let mut c = child(&mut fs, 2, &pref).expect("child");
    c.write_at(0, &[1u8; 4]).expect("child write");
    let crs = c.rootset().expect("child rootset");
    assert_eq!(crs.parents, vec![pref], "the parent snapshot is pinned");
}

#[test]
fn the_previous_recoverable_root_still_reads_a_snapshot_deleted_in_the_current_one() {
    let h = FaultHandle::new(&[], 0x1704);
    let (mut f, a) = overwritten_world(&h);
    f.delete_snapshot(1).expect("delete");
    let current = f.generation();
    drop(f);
    // Lose the newest root (as a torn root write plus a lost root copy would):
    // the open falls back to the previous generation, which names snapshot 1.
    let mut bytes = h.durable_bytes();
    let slot = crate::cow_proto::slot_offset(current) as usize;
    bytes[slot..slot + 512].fill(0);
    let len = bytes.len();
    bytes[len - 512..].fill(0);
    let damaged = FaultHandle::new(&bytes, 1);
    let (view, report) = crate::cow_proto::CowView::salvage(st(&damaged)).expect("salvage");
    assert!(report.possible_lost_commit);
    assert_eq!(view.generation(), current - 1);
    assert_eq!(
        crate::cow_snapshot::retained_image(&view, 1).expect("retained snapshot"),
        a,
        "bytes of the deleted snapshot were lost"
    );
}

// --------------------------------------------------- (c) compaction

fn assert_content(f: &File, snaps: &[(u64, Vec<u8>)], head: &[u8], label: &str) {
    assert_eq!(
        f.snapshot_ids(),
        snaps.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        "{label}"
    );
    for (id, img) in snaps {
        assert_eq!(&snap_image(f, *id), img, "{label}: snapshot {id}");
    }
    assert_eq!(image(f), head, "{label}: head");
}

#[test]
fn compaction_reproduces_head_and_live_snapshots_in_a_new_smaller_file() {
    let mut fs = Fs::default();
    let h = fs.handle(1, 0x1705);
    let Hist {
        mut file,
        mut snaps,
        head,
    } = history(&h);
    // A third snapshot, then delete the middle one: its bytes become garbage.
    let mut m = Model(head);
    write(&mut file, &mut m, BS, &rnd(8, 2 * BS));
    snaps.push((snap(&mut file).0, m.0.clone()));
    write(&mut file, &mut m, 0, &rnd(9, 3 * BS));
    file.delete_snapshot(2).expect("delete");
    snaps.remove(1);
    let head = m.0;
    let source_before = media(&fs, 1);
    let rs = file.rootset().expect("rootset");

    let th = FaultHandle::new(&[], 0x1706);
    let (target, _ack) = file.compact_to(st(&th), 50).expect("compact");
    assert_eq!(target.file_uid(), 50);
    assert_content(&target, &snaps, &head, "returned target");
    drop(target);
    // The source is byte-for-byte unchanged and still reads the same.
    assert_eq!(media(&fs, 1), source_before, "source changed");
    assert_content(&file, &snaps, &head, "source after compaction");

    // The published target reopens with the same logical content.
    th.crash(CrashPolicy::DropPending);
    let (mut t, _) = SnapFile::open(st(&th), &mut NoParents).expect("open target");
    assert_content(&t, &snaps, &head, "reopened target");
    assert_ne!(t.file_uid(), file.file_uid());
    // Old garbage is not copied: the target is smaller than the source and
    // than the source's rootset footprint of the whole file.
    let target_len = th.durable_bytes().len();
    assert!(target_len < source_before.1.len(), "{target_len}");
    assert!(rs.live_bytes() <= source_before.1.len() as u64);
    // Ids continue from the source's counter (the deleted id 2 stays unused).
    let (id, _) = t.create_snapshot().expect("snapshot");
    assert_eq!(id, 4);
}

#[test]
fn compaction_chunks_large_states_and_keeps_holes() {
    let mut fs = Fs::default();
    let h = fs.handle(1, 0x1707);
    let mut f = SnapFile::create_base(st(&h), 1).expect("create").0;
    let mut m = Model::default();
    let mut snaps = Vec::new();
    write(&mut f, &mut m, 0, &rnd(1, 200 * BS));
    snaps.push((snap(&mut f).0, m.0.clone()));
    write(&mut f, &mut m, 300 * BS, &rnd(2, 100 * BS)); // 100-block hole
    write(&mut f, &mut m, 10 * BS, &rnd(3, 50 * BS));
    snaps.push((snap(&mut f).0, m.0.clone()));
    write(&mut f, &mut m, 5, &rnd(4, 7));
    let th = FaultHandle::new(&[], 0x1708);
    let (t, _) = f.compact_to(st(&th), 51).expect("compact");
    assert_content(&t, &snaps, &m.0, "chunked target");
    drop(t);
    th.crash(CrashPolicy::DropPending);
    let (t, _) = SnapFile::open(st(&th), &mut NoParents).expect("open");
    assert_content(&t, &snaps, &m.0, "reopened chunked target");
    assert_eq!(t.rootset().expect("rootset").snapshot_ids.len(), 2);
}

#[test]
fn a_compacted_child_keeps_its_parent_and_reads_through_it() {
    let mut fs = Fs::default();
    let pref = parent_with_snapshot(&mut fs);
    let mut c = child(&mut fs, 2, &pref).expect("child");
    let mut m = Model(image(&c));
    write(&mut c, &mut m, 100, &rnd(5, 40));
    let s1 = (snap(&mut c).0, m.0.clone());
    write(&mut c, &mut m, 3 * BS, &rnd(6, 20)); // grows past the parent
    let th = FaultHandle::new(&[], 0x1709);
    let (t, _) = c.compact_to(st(&th), 52).expect("compact child");
    assert_eq!(t.parent_ref(), Some(pref), "same parent identity");
    assert_eq!(t.depth(), 1);
    drop(t);
    drop(c);
    fs.files.insert(52, th);
    let t = reopen(&fs, 52).expect("target opens against the real parent");
    assert_content(&t, &[s1], &m.0, "compacted child");
}

#[test]
fn a_child_of_the_old_file_is_not_reconnected_to_the_compacted_one() {
    let mut fs = Fs::default();
    let pref = parent_with_snapshot(&mut fs);
    let c = child(&mut fs, 2, &pref).expect("child");
    drop(c);
    let p = reopen(&fs, 1).expect("parent");
    let th = FaultHandle::new(&[], 0x170A);
    let (t, _) = p.compact_to(st(&th), 60).expect("compact");
    drop(t);
    drop(p);
    let original = fs.files.get(&1).expect("parent").clone();
    // The compacted file takes the parent's place in the catalogue.
    th.crash(CrashPolicy::DropPending);
    fs.files.insert(1, th);
    assert_eq!(
        reopen(&fs, 2).err(),
        Some(SnapError::ParentIdentityMismatch)
    );
    // Nothing was rewritten to make it work, and the original still serves it.
    fs.files.insert(1, original);
    assert!(reopen(&fs, 2).is_ok());
}

#[test]
fn compaction_refuses_bad_targets_without_writing_the_source() {
    let mut fs = Fs::default();
    let h = fs.handle(1, 0x170B);
    let Hist { file, .. } = history(&h);
    let before = media(&fs, 1);
    for uid in [0u64, 1] {
        let th = FaultHandle::new(&[], 1);
        assert!(matches!(
            file.compact_to(st(&th), uid).err(),
            Some(SnapError::BadWrite(_))
        ));
        assert_eq!(th.next_ordinal(), 0, "target touched for uid {uid}");
    }
    assert_eq!(media(&fs, 1), before);
}

#[test]
fn an_unpublished_compaction_target_does_not_open() {
    let h = FaultHandle::new(&[], 0x170C);
    let building = Header {
        building: true,
        file_uid: 9,
        disk_blocks: 0,
        next_snapshot_id: 1,
        depth: 0,
        parent: None,
        snapshots: Vec::new(),
    };
    assert_eq!(
        decode_header(&encode_header(&building)),
        Ok(building.clone())
    );
    SnapFile::init(st(&h), building, Vec::new()).expect("init building");
    h.crash(CrashPolicy::DropPending);
    assert_eq!(
        SnapFile::open(st(&h), &mut NoParents).err(),
        Some(SnapError::BuildInProgress)
    );
}

// -------------------------------------- (g) compaction fault sweep

fn compaction_ops() -> Vec<OpKind> {
    let src = FaultHandle::new(&[], 0x1710);
    let hist = history(&src);
    let th = FaultHandle::new(&[], 0x1711);
    hist.file.compact_to(st(&th), 50).expect("clean compaction");
    th.trace()
        .iter()
        .filter_map(|l| parse_kind(l))
        .map(|(_, k)| k)
        .collect()
}

#[test]
fn compaction_publishes_a_complete_verified_target_or_nothing_under_every_fault() {
    let ops = compaction_ops();
    assert!(ops.len() > 40, "{}", ops.len());
    let mut scenarios: Vec<(usize, Option<crate::testing::fault::Fault>)> = vec![(ops.len(), None)];
    for (i, kind) in ops.iter().enumerate() {
        for fault in faults_for(*kind) {
            scenarios.push((i, Some(fault)));
        }
    }
    let (mut complete, mut complete_unacked, mut absent, mut cases) = (0u64, 0u64, 0u64, 0u64);
    for (i, fault) in scenarios {
        for policy in policies() {
            cases += 1;
            let src = FaultHandle::new(&[], 0x1710);
            let Hist { file, snaps, head } = history(&src);
            let src_before = (src.volatile_bytes(), src.durable_bytes());
            let th = FaultHandle::new(&[], 0x1711);
            if let Some(f) = fault {
                th.script(i as u64, f);
            }
            let result = file.compact_to(st(&th), 50);
            let label = format!("op={i} fault={fault:?} policy={policy:?}");
            if fault.is_some_and(is_failing) {
                assert!(result.is_err(), "{label}: failing fault was acknowledged");
            }
            let acked = result.is_ok();
            drop(result);
            th.crash(policy);
            match SnapFile::open(st(&th), &mut NoParents) {
                Ok((t, _)) => {
                    // Present means complete and equal to the source: every
                    // live snapshot and the head, byte for byte.
                    complete += 1;
                    if !acked {
                        complete_unacked += 1;
                    }
                    assert_content(&t, &snaps, &head, &label);
                    assert_eq!(t.file_uid(), 50, "{label}");
                }
                Err(e) => {
                    absent += 1;
                    assert!(!acked, "{label}: acknowledged target lost: {e:?}");
                    assert!(is_absent(&e), "{label}: {e:?}");
                }
            }
            // The source is never touched by a failed or crashed compaction.
            assert_eq!(
                (src.volatile_bytes(), src.durable_bytes()),
                src_before,
                "{label}: source changed"
            );
            assert_content(&file, &snaps, &head, &label);
        }
    }
    assert!(complete > 0 && absent > 0, "{complete} {absent}");
    assert!(
        complete_unacked > 0,
        "never reached an unacked-but-complete target"
    );
    assert!(cases > 1000, "{cases}");
}

// ------------- publication verifies the target as reopened from media

fn flip_byte(h: &FaultHandle, offset: u64) {
    let mut raw = st(h);
    let mut b = [0u8; 1];
    raw.read_exact_at(offset, &mut b).expect("read");
    raw.write_all_at(offset, &[b[0] ^ 0x40]).expect("flip");
}

#[test]
fn damage_to_a_built_target_is_found_by_the_media_reopen_and_blocks_publication() {
    // Control: an undamaged built target publishes and equals the source.
    let src = FaultHandle::new(&[], 0x1720);
    let Hist { file, snaps, head } = history(&src);
    let th = FaultHandle::new(&[], 0x1721);
    let out = file.compact_build(st(&th), 50).expect("build");
    let (t, _) = file.compact_publish(out).expect("publish");
    assert_content(&t, &snaps, &head, "published");

    // Damage after the last commit and before the reopen: a block only a
    // snapshot reaches, the newest head block, a retained snapshot map.
    type Pick = fn(&File) -> u64;
    let cases: [(&str, Pick); 3] = [
        ("snapshot data block", |t| {
            *t.snapshot_data_blocks(1)
                .expect("blocks")
                .first()
                .expect("a block")
        }),
        ("newest head block", |t| {
            *t.rootset()
                .expect("rootset")
                .data_blocks
                .last()
                .expect("a block")
        }),
        ("snapshot map", |t| {
            t.snapshot_record(1).expect("record").map_offset
        }),
    ];
    for (name, pick) in cases {
        let src = FaultHandle::new(&[], 0x1720);
        let hist = history(&src);
        let th = FaultHandle::new(&[], 0x1721);
        let out = hist.file.compact_build(st(&th), 50).expect("build");
        flip_byte(&th, pick(&out) + 3);
        let result = hist.file.compact_publish(out);
        assert!(result.is_err(), "{name}: damaged target was published");
        // The flag was never cleared: the file does not open as a target.
        th.crash(CrashPolicy::DropPending);
        assert!(
            SnapFile::open(st(&th), &mut NoParents).is_err(),
            "{name}: damaged target opens"
        );
        // The source is untouched.
        assert_content(&hist.file, &hist.snaps, &hist.head, name);
    }
}
