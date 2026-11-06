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
