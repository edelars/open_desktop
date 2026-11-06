//! PROTOTYPE of immutable snapshots, bounded parent chains and writable
//! children (S9-14) on top of [`CowFile`](crate::cow_proto::CowFile).
//!
//! **Not a normative format.** Every constant is a hypothesis. The root slot
//! and block map of [`crate::cow_proto`] are unchanged; this layer lives
//! entirely in ordinary logical blocks of a CoW file, so S9-08..S9-12 files and
//! tests are unaffected.
//!
//! # Layer layout (logical blocks of the underlying CoW file)
//!
//! ```text
//! block 0           layer header  (magic, kind, file uid, disk size, parent
//!                                  reference, up to MAX_SNAPSHOTS records, CRC)
//! block 1           presence bitmap: bit b set = disk block b is stored here
//! block 2 + b       disk block b   (DATA_BASE = 2)
//! ```
//!
//! The *disk* is `disk_blocks` blocks of [`BLOCK_SIZE`] bytes. A disk block
//! whose presence bit is clear is a **hole**: a base file reads it as zeros, a
//! child reads it from its parent snapshot (and then from that snapshot's own
//! parent, and so on, ending in zeros). The CoW map is dense, so a hole below
//! the highest written block still occupies a zero-filled physical placeholder
//! block; presence, not the placeholder, is the truth. Disk size is bounded by
//! [`MAX_DISK_BLOCKS`] (one bitmap block).
//!
//! # Snapshots
//!
//! A snapshot is a **retained root**: the published generation `k` at the
//! moment of [`SnapFile::create_snapshot`]. Its record stores `{id,
//! generation, block_count, end_offset, map_offset, map_crc}`. The CoW writer
//! appends and never overwrites published bytes and this slice has no garbage
//! collection, so the map and blocks of generation `k` stay intact (retain
//! policy: generation `k` is never reused while its snapshot exists; deletion
//! of the name is S9-17 and frees nothing). Reading a snapshot parses the retained
//! map (CRC-checked against `map_crc`), takes the header and bitmap of that
//! generation from it, and reads blocks through it with per-block CRC checks.
//! What the snapshot shows therefore cannot depend on later writes.
//!
//! Publication is one ordinary commit (two barriers) that rewrites block 0 with
//! the new record and the next id. Ids are `u64`, start at 1, grow strictly
//! and are never reused. After a crash the snapshot is either in the recovered
//! header (whole) or absent; there is no partial record.
//!
//! # Children and identity of the parent
//!
//! A child is a **separate** CoW file whose header stores a [`ParentRef`]:
//! the parent file's uid, the snapshot id, and the snapshot's content identity
//! (generation, block count, end offset and the CRC of its block map). No
//! path is stored and no path would be trusted: a [`ParentResolver`] (the host
//! supplies one over `ParentLease`, which re-checks the file identity on every
//! use) hands over a storage for the reference, and this module accepts it only
//! if the file uid, the snapshot record and the retained map's CRC all match.
//! Anything else is [`SnapError::ParentIdentityMismatch`] (or
//! [`SnapError::ParentMissing`]) and nothing is read from it.
//!
//! The chain is walked at open: at most [`MAX_CHAIN_DEPTH`] ancestors, no file
//! uid may repeat (including the child's own), and every level's recorded
//! depth must agree with the walk. Violations refuse the open. Ancestors are
//! opened read-only ([`CowView::salvage`] never writes) and only the
//! referenced snapshot of each is read, so later mutation of a parent is
//! invisible to the child.
//!
//! # Writes
//!
//! [`SnapFile::write_at`] is one atomic commit: touched blocks (partial blocks
//! are read-modify-write through the full resolution, including the parent),
//! the presence bitmap, zero placeholders for a gap, and the header when the
//! disk grows. It is bounded by [`MAX_TX_BLOCKS`]; a larger write is
//! [`SnapError::TooLarge`] before any backend call.
//!
//! # Validation before the writable open (S9-16)
//!
//! [`SnapFile::open`] and [`SnapFile::create_child`] verify the **whole**
//! chain before any byte is written: the open runs through
//! [`CowFile::open_gated`](crate::cow_proto::CowFile), whose gate sees the
//! recovered read-only state before the one write a writable open can make
//! (the roll-forward repair of a damaged root slot). Per ancestor the checks
//! are: file uid, snapshot id, generation, block count, end offset and the CRC
//! of the retained block map (so a wrong or replaced intermediate ancestor is
//! caught at its own level, not only the nearest parent), the ancestor's own
//! parent link, no uid cycle, depth at most [`MAX_CHAIN_DEPTH`] and recorded
//! depths that agree with the walk. Failures are typed
//! ([`SnapError::ParentMissing`], [`SnapError::ParentIdentityMismatch`],
//! [`SnapError::SnapshotMissing`], [`SnapError::ParentCycle`],
//! [`SnapError::ChainTooDeep`], [`SnapError::ChainInconsistent`], ...). The
//! storages the resolver returned stay inside the open file, so a leasing
//! resolver keeps every ancestor leased (and unwritable) while the child is
//! open; the host re-checks file identity on each read, which turns a swap
//! after the check into a failed read (`od-host` tests).
//!
//! # Branch, checkout and rollback (S9-15)
//!
//! [`SnapFile::branch_from`] creates a new child from a chosen snapshot of an
//! open file. Checkout is [`SnapFile::open`] of that file; rollback is a new
//! branch from an older snapshot, never a rewrite of the current file. All of
//! this is the logical disk only: it does not touch guest RAM, vTPM or UEFI
//! state and is not a full-VM restore. Switching which file is the *active*
//! disk is a namespace operation that is not implemented (no durable rename,
//! ISSUE-055).
//!
//! # Limits of this slice
//!
//! No production [`ParentResolver`] exists: the only implementations are
//! [`NoParents`] and the test resolvers (the `od-host` test `PathResolver`).
//! Without one a child file cannot be opened in the product; chains are proven
//! only in tests. The identity of a parent is checked at open (file uid,
//! snapshot record, retained map CRC) and, when the resolver hands out
//! `od-host`'s `ParentLeaseStorage`, additionally on every read through the
//! lease by `(st_dev, st_ino)`. A resolver that does not re-check the host
//! file identity gives only the open-time content check.
//!
//! # Deletion, rootset and reclamation (S9-17)
//!
//! Deleting a snapshot ([`SnapFile::delete_snapshot`]) removes its **name**
//! only: one commit, no block freed. What is reachable is defined by
//! [`SnapFile::rootset`]: the head, every named snapshot, **both** recoverable
//! root generations with the snapshots each names, their maps, and the clone
//! parents the chain pins. A snapshot deleted in the current root stays in
//! the set while the previous root still names it; only a later commit lets it
//! drop out. The first reclamation is [`SnapFile::compact_to`]: offline,
//! exclusive-writer, copy into a **new** file (new uid, same snapshot ids),
//! full read-back comparison, then a single publishing commit. The source is
//! kept untouched until the caller confirms the target. Losing free space is
//! allowed, losing reachable bytes is not: compaction copies logical content,
//! never frees in place, and an unpublished target does not open.
//!
//! Limits: there is no in-place garbage collection. The file cannot see closed
//! children that name its snapshots (a catalogue is needed; deleting such a
//! snapshot makes the child refuse to open) and children are **not**
//! reconnected to a compacted file (the new file has another uid; they refuse
//! with [`SnapError::ParentIdentityMismatch`]). Reader and export leases are
//! `flock`s of the host: a held lease excludes the exclusive writer that both
//! operations need. Replacing the source by the target in the namespace needs
//! an atomic durable rename (ISSUE-055) and does not exist. Power loss is
//! modelled only by `FaultBackend`.
//!
//! # Not covered
//!
//! Atomic switch of an active disk
//! pointer, a production resolver and the host lock lifecycle for a whole VM
//! run, real power-loss behaviour, and sparse physical storage.

use crate::cow_parse::{
    Access, BlockDescriptor, Capabilities, Features, ParseError, ParseKind, RootRecord, Slot,
    StorageSource, parse_map, parse_slot_at,
};
use crate::cow_proto::{
    BLOCK_SIZE, CowError, CowFile, CowView, MAX_TX_BLOCKS, OpenReport, crc32, slot_offset,
};
use crate::io::{
    BackendError, CheckedStorage, DurableAck, StorageBackend, StorageCapabilities, StorageError,
};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

/// Logical CoW blocks before disk block 0 (header, presence bitmap).
pub const DATA_BASE: u64 = 2;
/// Most disk blocks one layer can address: one bit per block in one bitmap
/// block. Hypothesis.
pub const MAX_DISK_BLOCKS: u64 = (BLOCK_SIZE as u64) * 8;
/// Most snapshot records one file holds. Hypothesis.
pub const MAX_SNAPSHOTS: usize = 8;
/// Most ancestors a child may have (a base file has depth 0). Exceeding it is a
/// refusal at open and at create.
pub const MAX_CHAIN_DEPTH: u32 = 8;

const HEADER_MAGIC: [u8; 8] = *b"ODSNAP\0\x01";
pub(crate) const HEADER_VERSION: u32 = 1;
const KIND_BASE: u32 = 0;
const KIND_CHILD: u32 = 1;
const KIND_BUILDING: u32 = 2;
const UID_AT: usize = 16;
const DISK_BLOCKS_AT: usize = 24;
const NEXT_ID_AT: usize = 32;
const COUNT_AT: usize = 40;
const DEPTH_AT: usize = 44;
const PARENT_AT: usize = 48;
const PARENT_LEN: usize = 48;
const RECORDS_AT: usize = PARENT_AT + PARENT_LEN;
const RECORD_LEN: usize = 48;
const RESERVED_AT: usize = RECORDS_AT + MAX_SNAPSHOTS * RECORD_LEN;
const CRC_AT: usize = BLOCK_SIZE - 4;

pub(crate) type Block = [u8; BLOCK_SIZE];

// ------------------------------------------------------------------ types

/// Reference from a child to one snapshot of its parent file. It is identity,
/// not a locator: it contains no path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParentRef {
    /// Uid stored in the parent file's own header.
    pub file_uid: u64,
    pub snapshot_id: u64,
    /// Generation of the retained root.
    pub generation: u64,
    /// CoW block count of the retained root.
    pub block_count: u64,
    pub end_offset: u64,
    /// CRC of the retained block map: identifies every block's place and CRC.
    pub map_crc: u32,
}

/// One snapshot of a file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapshotRecord {
    pub id: u64,
    pub generation: u64,
    pub block_count: u64,
    pub end_offset: u64,
    pub map_offset: u64,
    pub map_crc: u32,
}

/// Byte quota and reserve of one file (S9-18). A **policy** the layer applies
/// itself; it is independent of what the host file system does. Every commit's
/// final file end is computed exactly ([`CowView::projected_end`]) and checked
/// **before** any backend call.
///
/// Ordinary data commits ([`SnapFile::write_at`], [`SnapFile::zero_range`])
/// may not take the file beyond `max_file_bytes - reserve_bytes`. The reserve
/// is held back for metadata-only commits ([`SnapFile::create_snapshot`],
/// [`SnapFile::delete_snapshot`], [`SnapFile::discard_range`]), which may use
/// it up to `max_file_bytes`, so a nearly full file can still publish or drop
/// a snapshot name. Compaction writes a different file and is not charged
/// here.
///
/// This is not a promise about the volume: a real `ENOSPC` can still arrive
/// at any mutating step and is handled by the commit protocol (the previous
/// durable state stays; an `Err` means old or new, never a torn state). An
/// acknowledged commit is never undone by a later refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapacityPolicy {
    pub max_file_bytes: u64,
    pub reserve_bytes: u64,
}

impl Default for CapacityPolicy {
    /// No quota.
    fn default() -> Self {
        Self {
            max_file_bytes: u64::MAX,
            reserve_bytes: 0,
        }
    }
}

/// What the layer offers for zero and discard (S9-18).
///
/// * `zero`: [`SnapFile::zero_range`] stores explicit zero blocks. They read
///   as zeros whatever a parent holds.
/// * `discard_inherit`: [`SnapFile::discard_range`] unmaps whole blocks. The
///   block goes back to "not stored here": a base file reads zeros, a child
///   reads its **parent's** bytes again. This is a layer operation, not a
///   guest-visible zeroing.
/// * `discard_zeroes_data`: after a discard the range reads as zeros. True
///   only without a parent; with one the parent data shows through.
/// * `physical_discard`: the backend can release physical space
///   ([`StorageCapabilities::discard`]); `false` for every backend today.
///
/// A guest feature may advertise discard only when
/// [`guest_discard_advertisable`](Self::guest_discard_advertisable) holds. A
/// child must serve a guest's discard (or write-zeroes) with
/// [`SnapFile::zero_range`] if it promises zeros.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LayerCapabilities {
    pub zero: bool,
    pub discard_inherit: bool,
    pub discard_zeroes_data: bool,
    pub physical_discard: bool,
}

impl LayerCapabilities {
    pub fn guest_discard_advertisable(&self) -> bool {
        self.discard_inherit && self.discard_zeroes_data
    }
}

/// Decoded layer header (block 0).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Header {
    /// Compaction target under construction: never a usable file head (kind
    /// bit 1). Retained states inside a finished target keep the flag.
    pub(crate) building: bool,
    pub(crate) file_uid: u64,
    pub(crate) disk_blocks: u64,
    pub(crate) next_snapshot_id: u64,
    pub(crate) depth: u32,
    pub(crate) parent: Option<ParentRef>,
    pub(crate) snapshots: Vec<SnapshotRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SnapError {
    Cow(CowError),
    /// Block 0 is not a layer header (or the file was never initialised).
    NotSnapFile(&'static str),
    /// CRC-valid or structurally wrong layer metadata.
    HeaderCorrupt(&'static str),
    UnsupportedHeader(&'static str),
    /// A retained snapshot's map, header or bitmap does not verify.
    SnapshotCorrupt {
        id: u64,
    },
    SnapshotMissing {
        id: u64,
    },
    SnapshotLimit,
    SnapshotIdExhausted,
    BlockCorrupt,
    /// The resolver has no parent for the reference.
    ParentMissing,
    /// The parent is not the object the child recorded (other file uid, other
    /// snapshot content, or a host identity check failed).
    ParentIdentityMismatch,
    ParentBusy,
    ParentStorage(StorageError),
    ParentIo(std::io::ErrorKind),
    /// The parent file does not open as a coherent CoW file.
    ParentCorrupt(CowError),
    /// The parent file may have lost an acknowledged commit.
    ParentDamaged,
    /// A file uid repeats along the chain.
    ParentCycle,
    ChainTooDeep {
        depth: u32,
    },
    ChainInconsistent(&'static str),
    OutOfRange,
    TooLarge {
        needed: u64,
        limit: usize,
    },
    BadWrite(&'static str),
    /// The file is a compaction target that was never published.
    BuildInProgress,
    /// The capacity policy refuses the commit; decided before any backend
    /// call, nothing was written. `end_after` is the file end the commit would
    /// publish, `limit` the byte limit that applies to this kind of commit.
    QuotaExceeded {
        end_after: u64,
        limit: u64,
    },
    /// Compaction read-back disagrees with the source (`None` is the head).
    CompactionMismatch {
        snapshot: Option<u64>,
    },
}

impl From<CowError> for SnapError {
    fn from(e: CowError) -> Self {
        Self::Cow(e)
    }
}

impl std::fmt::Display for SnapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cow snapshot error: {self:?}")
    }
}

impl std::error::Error for SnapError {}

/// Why a [`ParentResolver`] produced no storage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolveError {
    Missing,
    /// The host's own file identity check failed.
    IdentityMismatch,
    /// A writer holds the parent.
    Busy,
    Storage(StorageError),
    /// Any other host I/O failure while locating or opening the parent.
    Io(std::io::ErrorKind),
}

/// Supplies read-only storage for a parent reference. The implementation maps
/// the reference to a host object however it likes (a catalog, a path hint);
/// it is never trusted: the caller verifies uid and snapshot content.
pub trait ParentResolver {
    type Backend: StorageBackend + 'static;

    fn open_parent(
        &mut self,
        parent: &ParentRef,
    ) -> Result<CheckedStorage<Self::Backend>, ResolveError>;
}

/// Backend with no values: the backend type of [`NoParents`].
pub enum NoBackend {}

impl StorageBackend for NoBackend {
    fn capabilities(&self) -> StorageCapabilities {
        match *self {}
    }

    fn size(&self) -> Result<u64, BackendError> {
        match *self {}
    }

    fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> Result<usize, BackendError> {
        match *self {}
    }

    fn write_at(&mut self, _offset: u64, _buf: &[u8]) -> Result<usize, BackendError> {
        match *self {}
    }
}

/// Resolver for base files: every reference is missing.
pub struct NoParents;

impl ParentResolver for NoParents {
    type Backend = NoBackend;

    fn open_parent(
        &mut self,
        _parent: &ParentRef,
    ) -> Result<CheckedStorage<NoBackend>, ResolveError> {
        Err(ResolveError::Missing)
    }
}

// --------------------------------------------------------- header encoding

fn put(b: &mut Block, at: usize, bytes: &[u8]) {
    if let Some(dst) = b.get_mut(at..at + bytes.len()) {
        dst.copy_from_slice(bytes);
    }
}

fn rd_u64(b: &[u8], at: usize) -> u64 {
    let mut a = [0u8; 8];
    if let Some(s) = b.get(at..at + 8) {
        a.copy_from_slice(s);
    }
    u64::from_le_bytes(a)
}

fn rd_u32(b: &[u8], at: usize) -> u32 {
    let mut a = [0u8; 4];
    if let Some(s) = b.get(at..at + 4) {
        a.copy_from_slice(s);
    }
    u32::from_le_bytes(a)
}

fn all_zero(b: &[u8], from: usize, to: usize) -> bool {
    b.get(from..to).is_some_and(|s| s.iter().all(|x| *x == 0))
}

pub(crate) fn encode_header(h: &Header) -> Block {
    let mut b = [0u8; BLOCK_SIZE];
    put(&mut b, 0, &HEADER_MAGIC);
    put(&mut b, 8, &HEADER_VERSION.to_le_bytes());
    let kind = if h.parent.is_some() {
        KIND_CHILD
    } else {
        KIND_BASE
    } | if h.building { KIND_BUILDING } else { 0 };
    put(&mut b, 12, &kind.to_le_bytes());
    put(&mut b, UID_AT, &h.file_uid.to_le_bytes());
    put(&mut b, DISK_BLOCKS_AT, &h.disk_blocks.to_le_bytes());
    put(&mut b, NEXT_ID_AT, &h.next_snapshot_id.to_le_bytes());
    put(&mut b, COUNT_AT, &(h.snapshots.len() as u32).to_le_bytes());
    put(&mut b, DEPTH_AT, &h.depth.to_le_bytes());
    if let Some(p) = &h.parent {
        put(&mut b, PARENT_AT, &p.file_uid.to_le_bytes());
        put(&mut b, PARENT_AT + 8, &p.snapshot_id.to_le_bytes());
        put(&mut b, PARENT_AT + 16, &p.generation.to_le_bytes());
        put(&mut b, PARENT_AT + 24, &p.block_count.to_le_bytes());
        put(&mut b, PARENT_AT + 32, &p.end_offset.to_le_bytes());
        put(&mut b, PARENT_AT + 40, &p.map_crc.to_le_bytes());
    }
    for (i, r) in h.snapshots.iter().take(MAX_SNAPSHOTS).enumerate() {
        let at = RECORDS_AT + i * RECORD_LEN;
        put(&mut b, at, &r.id.to_le_bytes());
        put(&mut b, at + 8, &r.generation.to_le_bytes());
        put(&mut b, at + 16, &r.block_count.to_le_bytes());
        put(&mut b, at + 24, &r.end_offset.to_le_bytes());
        put(&mut b, at + 32, &r.map_offset.to_le_bytes());
        put(&mut b, at + 40, &r.map_crc.to_le_bytes());
    }
    let crc = crc32(b.get(..CRC_AT).unwrap_or(&[]));
    put(&mut b, CRC_AT, &crc.to_le_bytes());
    b
}

/// Strict decoder: anything not exactly understood is a refusal.
pub(crate) fn decode_header(b: &Block) -> Result<Header, SnapError> {
    if b.get(..8) != Some(&HEADER_MAGIC[..]) {
        return Err(SnapError::NotSnapFile("header magic"));
    }
    let stored = rd_u32(b, CRC_AT);
    if crc32(b.get(..CRC_AT).unwrap_or(&[])) != stored {
        return Err(SnapError::HeaderCorrupt("header crc"));
    }
    if rd_u32(b, 8) != HEADER_VERSION {
        return Err(SnapError::UnsupportedHeader("version"));
    }
    let raw_kind = rd_u32(b, 12);
    let building = raw_kind & KIND_BUILDING != 0;
    let kind = raw_kind & !KIND_BUILDING;
    let file_uid = rd_u64(b, UID_AT);
    let disk_blocks = rd_u64(b, DISK_BLOCKS_AT);
    let next_snapshot_id = rd_u64(b, NEXT_ID_AT);
    let count = rd_u32(b, COUNT_AT) as usize;
    let depth = rd_u32(b, DEPTH_AT);
    if file_uid == 0 {
        return Err(SnapError::HeaderCorrupt("file uid zero"));
    }
    if disk_blocks > MAX_DISK_BLOCKS {
        return Err(SnapError::HeaderCorrupt("disk size above limit"));
    }
    if next_snapshot_id == 0 {
        return Err(SnapError::HeaderCorrupt("next snapshot id zero"));
    }
    if count > MAX_SNAPSHOTS {
        return Err(SnapError::HeaderCorrupt("snapshot count above limit"));
    }
    if !all_zero(b, RESERVED_AT, CRC_AT) {
        return Err(SnapError::HeaderCorrupt("reserved bytes not zero"));
    }
    let parent = match kind {
        KIND_BASE => {
            if depth != 0 || !all_zero(b, PARENT_AT, RECORDS_AT) {
                return Err(SnapError::HeaderCorrupt("base file with parent data"));
            }
            None
        }
        KIND_CHILD => {
            if depth > MAX_CHAIN_DEPTH {
                return Err(SnapError::ChainTooDeep { depth });
            }
            if depth == 0 {
                return Err(SnapError::HeaderCorrupt("child depth zero"));
            }
            if !all_zero(b, PARENT_AT + 44, RECORDS_AT) {
                return Err(SnapError::HeaderCorrupt("parent padding not zero"));
            }
            let p = ParentRef {
                file_uid: rd_u64(b, PARENT_AT),
                snapshot_id: rd_u64(b, PARENT_AT + 8),
                generation: rd_u64(b, PARENT_AT + 16),
                block_count: rd_u64(b, PARENT_AT + 24),
                end_offset: rd_u64(b, PARENT_AT + 32),
                map_crc: rd_u32(b, PARENT_AT + 40),
            };
            if p.file_uid == 0 || p.snapshot_id == 0 {
                return Err(SnapError::HeaderCorrupt("parent reference zero"));
            }
            Some(p)
        }
        _ => return Err(SnapError::UnsupportedHeader("kind")),
    };
    let mut snapshots: Vec<SnapshotRecord> = Vec::with_capacity(count);
    for i in 0..MAX_SNAPSHOTS {
        let at = RECORDS_AT + i * RECORD_LEN;
        if i >= count {
            if !all_zero(b, at, at + RECORD_LEN) {
                return Err(SnapError::HeaderCorrupt("unused record not zero"));
            }
            continue;
        }
        if !all_zero(b, at + 44, at + RECORD_LEN) {
            return Err(SnapError::HeaderCorrupt("record padding not zero"));
        }
        let r = SnapshotRecord {
            id: rd_u64(b, at),
            generation: rd_u64(b, at + 8),
            block_count: rd_u64(b, at + 16),
            end_offset: rd_u64(b, at + 24),
            map_offset: rd_u64(b, at + 32),
            map_crc: rd_u32(b, at + 40),
        };
        if r.id == 0 || r.id >= next_snapshot_id {
            return Err(SnapError::HeaderCorrupt("snapshot id out of order"));
        }
        if r.block_count < DATA_BASE || r.block_count > MAX_DISK_BLOCKS + DATA_BASE {
            return Err(SnapError::HeaderCorrupt("snapshot block count"));
        }
        if let Some(prev) = snapshots.last()
            && (r.id <= prev.id || r.generation <= prev.generation)
        {
            return Err(SnapError::HeaderCorrupt("snapshot records not increasing"));
        }
        snapshots.push(r);
    }
    Ok(Header {
        building,
        file_uid,
        disk_blocks,
        next_snapshot_id,
        depth,
        parent,
        snapshots,
    })
}

fn clear_bit(bitmap: &mut Block, b: u64) {
    if let Some(byte) = usize::try_from(b / 8).ok().and_then(|i| bitmap.get_mut(i)) {
        *byte &= !(1 << (b % 8));
    }
}

fn bit(bitmap: &Block, b: u64) -> bool {
    usize::try_from(b / 8)
        .ok()
        .and_then(|i| bitmap.get(i))
        .is_some_and(|byte| byte & (1 << (b % 8)) != 0)
}

fn set_bit(bitmap: &mut Block, b: u64) {
    if let Some(byte) = usize::try_from(b / 8).ok().and_then(|i| bitmap.get_mut(i)) {
        *byte |= 1 << (b % 8);
    }
}

/// Bits at or above the disk size must be clear.
fn bitmap_within(bitmap: &Block, disk_blocks: u64) -> bool {
    (disk_blocks..MAX_DISK_BLOCKS).all(|b| !bit(bitmap, b))
}

fn block_of(v: Vec<u8>) -> Result<Block, SnapError> {
    Block::try_from(v).map_err(|_| SnapError::BlockCorrupt)
}

pub(crate) fn read_desc<B: StorageBackend>(
    storage: &CheckedStorage<B>,
    d: &BlockDescriptor,
) -> Result<Block, SnapError> {
    let mut buf = [0u8; BLOCK_SIZE];
    storage
        .read_exact_at(d.phys, &mut buf)
        .map_err(|e| SnapError::Cow(CowError::Storage(e)))?;
    if crc32(&buf) != d.crc {
        return Err(SnapError::BlockCorrupt);
    }
    Ok(buf)
}

// ------------------------------------------------------------------ levels

/// An immutable state (a retained root) reduced to what reading needs.
pub(crate) struct Level {
    pub(crate) header: Header,
    pub(crate) presence: Block,
    pub(crate) map: Vec<BlockDescriptor>,
}

impl Level {
    pub(crate) fn data_desc(&self, b: u64) -> Option<&BlockDescriptor> {
        if bit(&self.presence, b) {
            self.map
                .get(usize::try_from(b.checked_add(DATA_BASE)?).ok()?)
        } else {
            None
        }
    }
}

/// Parses the retained map of `rec` out of `storage`, then its header and
/// bitmap. Any inconsistency is `SnapshotCorrupt`.
pub(crate) fn load_level<B: StorageBackend>(
    storage: &CheckedStorage<B>,
    rec: &SnapshotRecord,
) -> Result<Level, SnapError> {
    let corrupt = SnapError::SnapshotCorrupt { id: rec.id };
    let root = RootRecord {
        slot_offset: slot_offset(rec.generation),
        generation: rec.generation,
        block_count: rec.block_count,
        map_offset: rec.map_offset,
        map_crc: rec.map_crc,
        end_offset: rec.end_offset,
        features: Features::default(),
    };
    let map = parse_map(&StorageSource(storage), &root).map_err(|e| match e.kind {
        ParseKind::Io(s) => SnapError::Cow(CowError::Storage(s)),
        _ => corrupt.clone(),
    })?;
    let (Some(h), Some(p)) = (map.first(), map.get(1)) else {
        return Err(corrupt);
    };
    let header = decode_header(&read_desc(storage, h).map_err(|_| corrupt.clone())?)
        .map_err(|_| corrupt.clone())?;
    let presence = read_desc(storage, p).map_err(|_| corrupt.clone())?;
    if !bitmap_within(&presence, header.disk_blocks) {
        return Err(corrupt);
    }
    // Every present block must exist in this root's dense map.
    let materialized = (map.len() as u64).saturating_sub(DATA_BASE);
    if (materialized..MAX_DISK_BLOCKS).any(|b| bit(&presence, b)) {
        return Err(corrupt);
    }
    Ok(Level {
        header,
        presence,
        map,
    })
}

/// Loads and verifies every retained snapshot a head header names.
pub(crate) fn load_own_snapshots<B: StorageBackend>(
    view: &CowView<B>,
    header: &Header,
) -> Result<BTreeMap<u64, Level>, SnapError> {
    let mut snapshots = BTreeMap::new();
    for rec in &header.snapshots {
        if rec.generation >= view.generation() || rec.end_offset > view.end_offset() {
            return Err(SnapError::SnapshotCorrupt { id: rec.id });
        }
        let level = load_level(view.storage(), rec)?;
        if level.header.file_uid != header.file_uid || level.header.parent != header.parent {
            return Err(SnapError::SnapshotCorrupt { id: rec.id });
        }
        snapshots.insert(rec.id, level);
    }
    Ok(snapshots)
}

trait AncestorStore {
    fn read_block(&self, d: &BlockDescriptor) -> Result<Block, SnapError>;
}

impl<B: StorageBackend> AncestorStore for CowView<B> {
    fn read_block(&self, d: &BlockDescriptor) -> Result<Block, SnapError> {
        read_desc(self.storage(), d)
    }
}

pub(crate) struct Ancestor {
    store: Box<dyn AncestorStore>,
    level: Level,
}

/// The references a verified chain was resolved through, nearest first: `first`
/// and then the parent link of each ancestor's retained state.
pub(crate) fn chain_links(first: &ParentRef, chain: &[Ancestor]) -> Vec<ParentRef> {
    let mut links = vec![*first];
    for a in chain {
        if let Some(p) = a.level.header.parent {
            links.push(p);
        }
    }
    links
}

/// Block `b` as the chain shows it, starting below a level whose own blocks
/// did not hold it. Holes at every level end in zeros.
fn chain_read(chain: &[Ancestor], b: u64) -> Result<Block, SnapError> {
    for a in chain {
        if b >= a.level.header.disk_blocks {
            break;
        }
        if let Some(d) = a.level.data_desc(b) {
            return a.store.read_block(d);
        }
    }
    Ok([0u8; BLOCK_SIZE])
}

fn from_resolve(e: ResolveError) -> SnapError {
    match e {
        ResolveError::Missing => SnapError::ParentMissing,
        ResolveError::IdentityMismatch => SnapError::ParentIdentityMismatch,
        ResolveError::Busy => SnapError::ParentBusy,
        ResolveError::Storage(s) => SnapError::ParentStorage(s),
        ResolveError::Io(kind) => SnapError::ParentIo(kind),
    }
}

fn read_state<B: StorageBackend>(view: &CowView<B>) -> Result<(Header, Block), SnapError> {
    read_state_with(view, false)
}

/// [`read_state`]; `allow_building` admits a compaction target that has not
/// been published (only the compaction's own verification asks for it).
pub(crate) fn read_state_with<B: StorageBackend>(
    view: &CowView<B>,
    allow_building: bool,
) -> Result<(Header, Block), SnapError> {
    if view.block_count() < DATA_BASE {
        return Err(SnapError::NotSnapFile("not initialised"));
    }
    let header = decode_header(&block_of(view.read_block(0)?)?)?;
    if header.building && !allow_building {
        return Err(SnapError::BuildInProgress);
    }
    let presence = block_of(view.read_block(1)?)?;
    if !bitmap_within(&presence, header.disk_blocks) {
        return Err(SnapError::HeaderCorrupt("presence beyond disk size"));
    }
    let materialized = view.block_count().saturating_sub(DATA_BASE);
    if (materialized..MAX_DISK_BLOCKS).any(|b| bit(&presence, b)) {
        return Err(SnapError::HeaderCorrupt("presence beyond stored blocks"));
    }
    Ok((header, presence))
}

/// Opens and verifies the whole chain starting at `first`. `own_uid` seeds the
/// cycle check. Checks happen before anything is read from a candidate.
pub(crate) fn resolve_chain<R: ParentResolver>(
    resolver: &mut R,
    first: &ParentRef,
    own_uid: u64,
) -> Result<Vec<Ancestor>, SnapError> {
    let mut seen = vec![own_uid];
    let mut chain: Vec<Ancestor> = Vec::new();
    let mut want = *first;
    loop {
        if chain.len() >= MAX_CHAIN_DEPTH as usize {
            return Err(SnapError::ChainTooDeep {
                depth: chain.len() as u32 + 1,
            });
        }
        if seen.contains(&want.file_uid) {
            return Err(SnapError::ParentCycle);
        }
        let storage = resolver.open_parent(&want).map_err(from_resolve)?;
        let (view, report) = CowView::salvage(storage).map_err(SnapError::ParentCorrupt)?;
        if report.possible_lost_commit {
            return Err(SnapError::ParentDamaged);
        }
        let (head, _) = read_state(&view)?;
        if head.file_uid != want.file_uid {
            return Err(SnapError::ParentIdentityMismatch);
        }
        let rec = head
            .snapshots
            .iter()
            .find(|r| r.id == want.snapshot_id)
            .ok_or(SnapError::SnapshotMissing {
                id: want.snapshot_id,
            })?;
        if rec.generation != want.generation
            || rec.block_count != want.block_count
            || rec.end_offset != want.end_offset
            || rec.map_crc != want.map_crc
        {
            return Err(SnapError::ParentIdentityMismatch);
        }
        if rec.generation >= view.generation() || rec.end_offset > view.end_offset() {
            return Err(SnapError::SnapshotCorrupt { id: rec.id });
        }
        let level = load_level(view.storage(), rec)?;
        if level.header.file_uid != want.file_uid {
            return Err(SnapError::ParentIdentityMismatch);
        }
        if level.header.parent != head.parent {
            return Err(SnapError::ChainInconsistent("parent link differs per root"));
        }
        seen.push(want.file_uid);
        let next = level.header.parent;
        chain.push(Ancestor {
            store: Box::new(view),
            level,
        });
        match next {
            Some(p) => want = p,
            None => break,
        }
    }
    let total = chain.len();
    for (i, a) in chain.iter().enumerate() {
        if a.level.header.depth as usize != total.saturating_sub(1 + i) {
            return Err(SnapError::ChainInconsistent("depth disagrees with chain"));
        }
    }
    Ok(chain)
}

fn read_range(
    disk_blocks: u64,
    offset: u64,
    buf: &mut [u8],
    mut block: impl FnMut(u64) -> Result<Block, SnapError>,
) -> Result<(), SnapError> {
    if buf.is_empty() {
        return Ok(());
    }
    let bs = BLOCK_SIZE as u64;
    let end = offset
        .checked_add(buf.len() as u64)
        .ok_or(SnapError::OutOfRange)?;
    if end > disk_blocks * bs {
        return Err(SnapError::OutOfRange);
    }
    let mut pos = offset;
    while pos < end {
        let b = pos / bs;
        let data = block(b)?;
        let in_block = usize::try_from(pos % bs).map_err(|_| SnapError::OutOfRange)?;
        let take =
            usize::try_from((bs - pos % bs).min(end - pos)).map_err(|_| SnapError::OutOfRange)?;
        let done = usize::try_from(pos - offset).map_err(|_| SnapError::OutOfRange)?;
        let (src, dst) = (
            data.get(in_block..in_block + take),
            buf.get_mut(done..done + take),
        );
        match (src, dst) {
            (Some(s), Some(d)) => d.copy_from_slice(s),
            _ => return Err(SnapError::OutOfRange),
        }
        pos += take as u64;
    }
    Ok(())
}

// ------------------------------------------------------------------ file

/// State proven by [`SnapFile::open`] before the CoW file may write.
struct Verified {
    header: Header,
    presence: Block,
    snapshots: BTreeMap<u64, Level>,
    chain: Vec<Ancestor>,
}

/// An open writable layer file: a base disk or a child over a parent snapshot.
/// One writer; the ancestors are read-only.
pub struct SnapFile<B: StorageBackend> {
    file: CowFile<B>,
    header: Header,
    presence: Block,
    snapshots: BTreeMap<u64, Level>,
    chain: Rc<Vec<Ancestor>>,
    policy: CapacityPolicy,
}

impl<B: StorageBackend> SnapFile<B> {
    /// Creates an empty base disk (no parent). `file_uid` is the file's
    /// identity as children will record it; it must be non-zero and should be
    /// unique (random in a real host).
    pub fn create_base(
        storage: CheckedStorage<B>,
        file_uid: u64,
    ) -> Result<(Self, DurableAck), SnapError> {
        if file_uid == 0 {
            return Err(SnapError::BadWrite("file uid zero"));
        }
        Self::init(
            storage,
            Header {
                building: false,
                file_uid,
                disk_blocks: 0,
                next_snapshot_id: 1,
                depth: 0,
                parent: None,
                snapshots: Vec::new(),
            },
            Vec::new(),
        )
    }

    /// Creates a writable child of `parent` (see [`SnapFile::snapshot_ref`]).
    /// The whole chain is resolved and verified first; on any failure nothing
    /// is written. The child starts with the parent snapshot's disk size and
    /// no blocks of its own.
    pub fn create_child<R: ParentResolver>(
        storage: CheckedStorage<B>,
        resolver: &mut R,
        parent: &ParentRef,
        file_uid: u64,
    ) -> Result<(Self, DurableAck), SnapError> {
        if file_uid == 0 {
            return Err(SnapError::BadWrite("file uid zero"));
        }
        let chain = resolve_chain(resolver, parent, file_uid)?;
        let disk_blocks = chain
            .first()
            .map(|a| a.level.header.disk_blocks)
            .ok_or(SnapError::ChainInconsistent("empty chain"))?;
        let depth =
            u32::try_from(chain.len()).map_err(|_| SnapError::ChainInconsistent("depth"))?;
        Self::init(
            storage,
            Header {
                building: false,
                file_uid,
                disk_blocks,
                next_snapshot_id: 1,
                depth,
                parent: Some(*parent),
                snapshots: Vec::new(),
            },
            chain,
        )
    }

    /// Creates a new writable child (a **branch**) that starts from snapshot
    /// `snapshot_id` of `source` (S9-15). It is [`create_child`](Self::create_child)
    /// over `source.snapshot_ref(snapshot_id)`: the chosen snapshot, the source
    /// file's live state and every other branch are only read and never
    /// written, and the new file shows exactly the logical bytes of the
    /// snapshot until it is written.
    ///
    /// `resolver` must hand out read-only storage for the **source file**
    /// (and its ancestors): the branch reads the snapshot from there for as
    /// long as it is open. With the host's lease resolver a source that a
    /// writer holds (an open `source` is one) is [`SnapError::ParentBusy`], so
    /// a branch cannot be cut under a live writer. To branch from a file nobody holds, take the
    /// reference with [`inspect_snapshot_ref`](Self::inspect_snapshot_ref)
    /// and call [`create_child`](Self::create_child). A poisoned `source` is
    /// refused, a missing id is [`SnapError::SnapshotMissing`], and a `new_uid`
    /// equal to one on the chain is [`SnapError::ParentCycle`]. On any error
    /// nothing is written to `storage`.
    ///
    /// This is a logical-disk operation only. It says nothing about guest RAM,
    /// vTPM or UEFI state, and a branch is not a full-VM restore.
    ///
    /// # Checkout and rollback
    ///
    /// *Checkout* of a branch is [`SnapFile::open`] of its file. *Rollback* of
    /// a disk to an older snapshot is a new branch from that snapshot: the
    /// current file is never rewritten and stays available. Making the branch
    /// the **active** one is a change of which file the VM's disk name refers
    /// to; that needs an atomic, durable namespace operation (rename plus
    /// directory flush), which does not exist yet (ISSUE-055), so this slice
    /// does not switch an active pointer and does not imitate one.
    pub fn branch_from<S: StorageBackend, R: ParentResolver>(
        storage: CheckedStorage<B>,
        resolver: &mut R,
        source: &SnapFile<S>,
        snapshot_id: u64,
        new_uid: u64,
    ) -> Result<(Self, DurableAck), SnapError> {
        if source.is_poisoned() {
            return Err(SnapError::BadWrite("branch source is poisoned"));
        }
        let parent = source
            .snapshot_ref(snapshot_id)
            .ok_or(SnapError::SnapshotMissing { id: snapshot_id })?;
        Self::create_child(storage, resolver, &parent, new_uid)
    }

    /// Reads the [`ParentRef`] of snapshot `snapshot_id` out of a **closed**
    /// file through read-only storage (it never writes). This is how a branch
    /// is cut from a file no writer holds: lease the source for reading, take
    /// the reference here, then [`create_child`](Self::create_child) with it.
    /// The record is verified like any retained snapshot (its map CRC, header
    /// and bitmap); a source that may have lost an acknowledged commit is
    /// [`SnapError::ParentDamaged`]. The source's own ancestors are not
    /// checked here: `create_child` verifies the whole chain.
    pub fn inspect_snapshot_ref(
        storage: CheckedStorage<B>,
        snapshot_id: u64,
    ) -> Result<ParentRef, SnapError> {
        let (view, report) = CowView::salvage(storage)?;
        if report.possible_lost_commit {
            return Err(SnapError::ParentDamaged);
        }
        let (head, _) = read_state(&view)?;
        let rec = head
            .snapshots
            .iter()
            .find(|r| r.id == snapshot_id)
            .ok_or(SnapError::SnapshotMissing { id: snapshot_id })?;
        if rec.generation >= view.generation() || rec.end_offset > view.end_offset() {
            return Err(SnapError::SnapshotCorrupt { id: rec.id });
        }
        let level = load_level(view.storage(), rec)?;
        if level.header.file_uid != head.file_uid || level.header.parent != head.parent {
            return Err(SnapError::SnapshotCorrupt { id: rec.id });
        }
        Ok(ParentRef {
            file_uid: head.file_uid,
            snapshot_id: rec.id,
            generation: rec.generation,
            block_count: rec.block_count,
            end_offset: rec.end_offset,
            map_crc: rec.map_crc,
        })
    }

    /// Writes an arbitrary header over an empty backend with no verification.
    /// Exists so tests can forge cycles and depth lies that the checked
    /// constructors refuse.
    pub(crate) fn init(
        storage: CheckedStorage<B>,
        header: Header,
        chain: Vec<Ancestor>,
    ) -> Result<(Self, DurableAck), SnapError> {
        Self::init_shared(storage, header, Rc::new(chain))
    }

    fn init_shared(
        storage: CheckedStorage<B>,
        header: Header,
        chain: Rc<Vec<Ancestor>>,
    ) -> Result<(Self, DurableAck), SnapError> {
        let (mut file, _created) = CowFile::create(storage)?;
        let hdr = encode_header(&header);
        let presence = [0u8; BLOCK_SIZE];
        let ack = file.commit(&[(0, hdr.as_slice()), (1, presence.as_slice())])?;
        Ok((
            Self {
                file,
                header,
                presence,
                snapshots: BTreeMap::new(),
                chain,
                policy: CapacityPolicy::default(),
            },
            ack,
        ))
    }

    /// Writable open: CoW recovery, then the header, every retained snapshot
    /// and (for a child) the verified parent chain. Any failure refuses.
    ///
    /// All of that verification (S9-16) runs against the recovered read-only
    /// state **before** the only write a writable open can make (repairing a
    /// damaged root slot), so a refused open leaves every byte of the child
    /// and of its ancestors untouched. For a child the whole chain is walked:
    /// per ancestor the resolver's storage must hold the recorded file uid, the
    /// recorded snapshot id with generation, block count, end offset and block
    /// map CRC, and its own parent link must continue the chain; uid cycles and
    /// more than [`MAX_CHAIN_DEPTH`] ancestors are refused, and the recorded
    /// depths must agree with the walk. The ancestors' storages stay in the
    /// returned file, so with a leasing resolver the leases are held until it
    /// is dropped.
    pub fn open<R: ParentResolver>(
        storage: CheckedStorage<B>,
        resolver: &mut R,
    ) -> Result<(Self, OpenReport), SnapError> {
        let mut verified: Option<Verified> = None;
        let (file, report) = CowFile::open_gated(storage, &Capabilities::CURRENT, |view| {
            verified = Some(Self::verify_open(view, resolver)?);
            Ok::<(), SnapError>(())
        })?;
        let Verified {
            header,
            presence,
            snapshots,
            chain,
        } = verified.ok_or(SnapError::ChainInconsistent("open was not verified"))?;
        Ok((
            Self {
                file,
                header,
                presence,
                snapshots,
                chain: Rc::new(chain),
                policy: CapacityPolicy::default(),
            },
            report,
        ))
    }

    /// Everything `open` checks, from read-only state (no write).
    fn verify_open<R: ParentResolver>(
        view: &CowView<B>,
        resolver: &mut R,
    ) -> Result<Verified, SnapError> {
        let (header, presence) = read_state(view)?;
        let snapshots = load_own_snapshots(view, &header)?;
        let chain = match &header.parent {
            None => Vec::new(),
            Some(p) => {
                let chain = resolve_chain(resolver, p, header.file_uid)?;
                if chain.len() != header.depth as usize {
                    return Err(SnapError::ChainInconsistent(
                        "header depth differs from chain",
                    ));
                }
                chain
            }
        };
        Ok(Verified {
            header,
            presence,
            snapshots,
            chain,
        })
    }

    pub fn file_uid(&self) -> u64 {
        self.header.file_uid
    }

    pub fn generation(&self) -> u64 {
        self.file.generation()
    }

    /// Number of ancestors (0 for a base file).
    pub fn depth(&self) -> u32 {
        self.header.depth
    }

    pub fn parent_ref(&self) -> Option<ParentRef> {
        self.header.parent
    }

    pub fn disk_blocks(&self) -> u64 {
        self.header.disk_blocks
    }

    pub fn disk_size(&self) -> u64 {
        self.header.disk_blocks * BLOCK_SIZE as u64
    }

    pub fn is_poisoned(&self) -> bool {
        self.file.is_poisoned()
    }

    pub fn snapshot_ids(&self) -> Vec<u64> {
        self.header.snapshots.iter().map(|r| r.id).collect()
    }

    pub fn snapshot_record(&self, id: u64) -> Option<SnapshotRecord> {
        self.header.snapshots.iter().copied().find(|r| r.id == id)
    }

    /// Disk size in blocks as the snapshot saw it.
    pub fn snapshot_disk_blocks(&self, id: u64) -> Option<u64> {
        self.snapshots.get(&id).map(|l| l.header.disk_blocks)
    }

    /// The reference a child records to use snapshot `id` of this file.
    pub fn snapshot_ref(&self, id: u64) -> Option<ParentRef> {
        let r = self.snapshot_record(id)?;
        Some(ParentRef {
            file_uid: self.header.file_uid,
            snapshot_id: r.id,
            generation: r.generation,
            block_count: r.block_count,
            end_offset: r.end_offset,
            map_crc: r.map_crc,
        })
    }

    fn head_block(&self, b: u64) -> Result<Block, SnapError> {
        if b >= self.header.disk_blocks {
            return Err(SnapError::OutOfRange);
        }
        if bit(&self.presence, b) {
            let index = b.checked_add(DATA_BASE).ok_or(SnapError::OutOfRange)?;
            block_of(self.file.read_block(index)?)
        } else {
            chain_read(&self.chain, b)
        }
    }

    /// Reads the current disk. Holes read as parent data or zeros; every
    /// block read is CRC-checked.
    pub fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), SnapError> {
        read_range(self.header.disk_blocks, offset, buf, |b| self.head_block(b))
    }

    /// Reads snapshot `id` as it was when it was published.
    pub fn read_snapshot_at(&self, id: u64, offset: u64, buf: &mut [u8]) -> Result<(), SnapError> {
        let level = self
            .snapshots
            .get(&id)
            .ok_or(SnapError::SnapshotMissing { id })?;
        read_range(level.header.disk_blocks, offset, buf, |b| {
            match level.data_desc(b) {
                Some(d) => read_desc(self.file.storage(), d),
                None => chain_read(&self.chain, b),
            }
        })
    }

    /// Writes `data` at byte `offset` of the disk as one atomic commit; the
    /// disk grows if the range ends past it (zero gap). See the module docs.
    pub fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<DurableAck, SnapError> {
        if data.is_empty() {
            return Err(SnapError::BadWrite("empty write"));
        }
        let bs = BLOCK_SIZE as u64;
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or(SnapError::OutOfRange)?;
        let first = offset / bs;
        let last = (end - 1) / bs;
        if last >= MAX_DISK_BLOCKS {
            return Err(SnapError::OutOfRange);
        }
        let have = self.file.block_count();
        let gap_end = first + DATA_BASE;
        let gap = gap_end.saturating_sub(have);
        let new_disk = self.header.disk_blocks.max(last + 1);
        let grows = new_disk != self.header.disk_blocks;
        let needed = (last - first + 1) + gap + 1 + u64::from(grows);
        if needed > MAX_TX_BLOCKS as u64 {
            return Err(SnapError::TooLarge {
                needed,
                limit: MAX_TX_BLOCKS,
            });
        }

        self.check_capacity(
            usize::try_from(needed).map_err(|_| SnapError::OutOfRange)?,
            have.max(last + 1 + DATA_BASE),
            false,
        )?;
        let mut presence = self.presence;
        let mut header = self.header.clone();
        header.disk_blocks = new_disk;
        let mut writes: Vec<(u64, Vec<u8>)> = Vec::new();
        if grows {
            writes.push((0, encode_header(&header).to_vec()));
        }
        for index in have..gap_end {
            writes.push((index, vec![0u8; BLOCK_SIZE]));
        }
        for b in first..=last {
            let start = b * bs;
            let from = offset.max(start);
            let to = end.min(start + bs);
            let full = from == start && to == start + bs;
            let mut block = if full || b >= self.header.disk_blocks {
                [0u8; BLOCK_SIZE]
            } else {
                self.head_block(b)?
            };
            let src = usize::try_from(from - offset)
                .ok()
                .zip(usize::try_from(to - offset).ok())
                .and_then(|(a, z)| data.get(a..z));
            let dst = usize::try_from(from - start)
                .ok()
                .zip(usize::try_from(to - start).ok())
                .and_then(|(a, z)| block.get_mut(a..z));
            match (src, dst) {
                (Some(s), Some(d)) => d.copy_from_slice(s),
                _ => return Err(SnapError::OutOfRange),
            }
            set_bit(&mut presence, b);
            writes.push((b + DATA_BASE, block.to_vec()));
        }
        writes.push((1, presence.to_vec()));
        let refs: Vec<(u64, &[u8])> = writes.iter().map(|(i, d)| (*i, d.as_slice())).collect();
        let ack = self.file.commit(&refs)?;
        self.header = header;
        self.presence = presence;
        Ok(ack)
    }

    /// Test hooks: published end, logical CoW block count and the projected
    /// end of a commit, as the capacity policy sees them.
    #[cfg(test)]
    pub(crate) fn end_for_tests(&self) -> u64 {
        self.file.end_offset()
    }

    #[cfg(test)]
    pub(crate) fn file_block_count_for_tests(&self) -> u64 {
        self.file.block_count()
    }

    #[cfg(test)]
    pub(crate) fn projected_end_for_tests(&self, n: usize, count: u64) -> Option<u64> {
        self.file.projected_end(n, count)
    }

    /// Replaces the capacity policy (not persisted). `reserve_bytes` must not
    /// exceed `max_file_bytes`.
    pub fn set_capacity_policy(&mut self, policy: CapacityPolicy) -> Result<(), SnapError> {
        if policy.reserve_bytes > policy.max_file_bytes {
            return Err(SnapError::BadWrite("reserve above the file limit"));
        }
        self.policy = policy;
        Ok(())
    }

    pub fn capacity_policy(&self) -> CapacityPolicy {
        self.policy
    }

    /// Checks a commit of `n_writes` blocks that leaves `new_block_count`
    /// logical CoW blocks against the policy. No backend call.
    fn check_capacity(
        &self,
        n_writes: usize,
        new_block_count: u64,
        metadata_only: bool,
    ) -> Result<(), SnapError> {
        let end_after = self
            .file
            .projected_end(n_writes, new_block_count)
            .ok_or(SnapError::Cow(CowError::LayoutOverflow))?;
        let limit = if metadata_only {
            self.policy.max_file_bytes
        } else {
            self.policy
                .max_file_bytes
                .saturating_sub(self.policy.reserve_bytes)
        };
        if end_after > limit {
            return Err(SnapError::QuotaExceeded { end_after, limit });
        }
        Ok(())
    }

    /// Zero and discard support of this file (S9-18); see
    /// [`LayerCapabilities`].
    pub fn layer_capabilities(&self) -> LayerCapabilities {
        LayerCapabilities {
            zero: true,
            discard_inherit: true,
            discard_zeroes_data: self.chain.is_empty(),
            physical_discard: self.file.storage().capabilities().discard,
        }
    }

    /// Writes explicit zero blocks over `[offset, offset + len)` as one commit
    /// (S9-18). Fully covered blocks become stored zero blocks, so in a child
    /// they **mask** the parent; a partially covered block is a
    /// read-modify-write that zeroes only the range and keeps every other byte
    /// (parent bytes included). The range must lie inside the disk and span at
    /// most the transaction limit; both are checked before any backend call.
    /// Zero blocks are physical blocks: nothing is released (no sparse
    /// extents, no host hole punching).
    pub fn zero_range(&mut self, offset: u64, len: u64) -> Result<DurableAck, SnapError> {
        if len == 0 {
            return Err(SnapError::BadWrite("empty zero range"));
        }
        let end = offset.checked_add(len).ok_or(SnapError::OutOfRange)?;
        if end > self.disk_size() {
            return Err(SnapError::OutOfRange);
        }
        let limit_bytes = (MAX_TX_BLOCKS as u64) * BLOCK_SIZE as u64;
        if len > limit_bytes {
            return Err(SnapError::TooLarge {
                needed: len.div_ceil(BLOCK_SIZE as u64),
                limit: MAX_TX_BLOCKS,
            });
        }
        let zeros = vec![0u8; usize::try_from(len).map_err(|_| SnapError::OutOfRange)?];
        self.write_at(offset, &zeros)
    }

    /// Unmaps every block that `[offset, offset + len)` covers **completely**
    /// (S9-18): its presence bit is cleared in one metadata commit, so it
    /// reads as "not stored here" again (zeros in a base file, the parent's
    /// bytes in a child). Partially covered edge blocks are left as they are:
    /// discard is a hint. Earlier snapshots keep what they saw. Physical
    /// blocks stay in the file until a compaction (no hole punching).
    /// Returns `None` when nothing changed (no commit). The range must lie
    /// inside the disk.
    ///
    /// This is **not** a zeroing operation; see [`LayerCapabilities`].
    pub fn discard_range(
        &mut self,
        offset: u64,
        len: u64,
    ) -> Result<Option<DurableAck>, SnapError> {
        if len == 0 {
            return Err(SnapError::BadWrite("empty discard range"));
        }
        let end = offset.checked_add(len).ok_or(SnapError::OutOfRange)?;
        if end > self.disk_size() {
            return Err(SnapError::OutOfRange);
        }
        let bs = BLOCK_SIZE as u64;
        let (first, last_excl) = (offset.div_ceil(bs), end / bs);
        let mut presence = self.presence;
        let mut changed = false;
        for b in first..last_excl {
            if bit(&presence, b) {
                clear_bit(&mut presence, b);
                changed = true;
            }
        }
        if !changed {
            return Ok(None);
        }
        self.check_capacity(1, self.file.block_count(), true)?;
        let ack = self.file.commit(&[(1, presence.as_slice())])?;
        self.presence = presence;
        Ok(Some(ack))
    }

    /// Publishes a snapshot of the current published state and returns its id.
    /// One commit; after a crash the snapshot is whole or absent.
    pub fn create_snapshot(&mut self) -> Result<(u64, DurableAck), SnapError> {
        let id = self.header.next_snapshot_id;
        let next = id.checked_add(1).ok_or(SnapError::SnapshotIdExhausted)?;
        let ack = self.publish_snapshot(id, next)?;
        Ok((id, ack))
    }

    /// Publishes snapshot `id` (not below `next_snapshot_id`) and sets the next
    /// id to `next`. [`create_snapshot`](Self::create_snapshot) passes the next
    /// free id; compaction passes the id the source used.
    fn publish_snapshot(&mut self, id: u64, next: u64) -> Result<DurableAck, SnapError> {
        if self.header.snapshots.len() >= MAX_SNAPSHOTS {
            return Err(SnapError::SnapshotLimit);
        }
        if id < self.header.next_snapshot_id || next <= id {
            return Err(SnapError::BadWrite("snapshot id not increasing"));
        }
        let rec = SnapshotRecord {
            id,
            generation: self.file.generation(),
            block_count: self.file.block_count(),
            end_offset: self.file.end_offset(),
            map_offset: self.file.map_offset(),
            map_crc: self.file.map_crc(),
        };
        // The retained root is already published, so its level can be loaded
        // and verified *before* the commit. Everything after the commit below
        // is infallible in-memory bookkeeping: once the commit has returned,
        // no error may report an unpublished snapshot. A record that does not
        // verify is refused here and never published.
        let level = load_level(self.file.storage(), &rec)?;
        self.check_capacity(1, self.file.block_count(), true)?;
        let mut header = self.header.clone();
        header.snapshots.push(rec);
        header.next_snapshot_id = next;
        let ack = self
            .file
            .commit(&[(0, encode_header(&header).as_slice())])?;
        self.header = header;
        self.snapshots.insert(id, level);
        Ok(ack)
    }

    /// Deletes the **name** of snapshot `id` (S9-17): one commit (two
    /// barriers) that rewrites the header without its record. No block is
    /// freed or touched: the data stays in the file, still referenced by the
    /// previous recoverable root (which still names the snapshot) and by
    /// whatever else is reachable; only [`compact_to`](Self::compact_to)
    /// later drops what no root reaches. The id is never reused
    /// (`next_snapshot_id` is monotonic). After a crash the snapshot is either
    /// still named (whole) or gone.
    ///
    /// The file cannot know which closed child files record this snapshot as
    /// their parent. Deleting it makes such a child refuse to open
    /// ([`SnapError::SnapshotMissing`]); a host must check its catalogue of
    /// children first (S9-16 catalogue; not implemented here) and must hold the
    /// exclusive writer lock, which an open child's parent lease prevents.
    pub fn delete_snapshot(&mut self, id: u64) -> Result<DurableAck, SnapError> {
        let pos = self
            .header
            .snapshots
            .iter()
            .position(|r| r.id == id)
            .ok_or(SnapError::SnapshotMissing { id })?;
        self.check_capacity(1, self.file.block_count(), true)?;
        let mut header = self.header.clone();
        header.snapshots.remove(pos);
        let ack = self
            .file
            .commit(&[(0, encode_header(&header).as_slice())])?;
        self.header = header;
        self.snapshots.remove(&id);
        Ok(ack)
    }

    /// Physical offsets of the data blocks snapshot `id` stores itself (holes
    /// are not listed), for tests and the rootset.
    pub fn snapshot_data_blocks(&self, id: u64) -> Option<BTreeSet<u64>> {
        let level = self.snapshots.get(&id)?;
        Some(
            (0..level.header.disk_blocks)
                .filter_map(|b| level.data_desc(b).map(|d| d.phys))
                .collect(),
        )
    }

    /// The previous recoverable root (generation - 1), read from its slot.
    fn previous_root(&self) -> Result<RootRecord, SnapError> {
        let unavailable =
            || SnapError::Cow(CowError::RootInconsistent("previous root unavailable"));
        let prev = self
            .file
            .generation()
            .checked_sub(1)
            .ok_or_else(unavailable)?;
        let src = StorageSource(self.file.storage());
        match parse_slot_at(
            &src,
            slot_offset(prev),
            (prev & 1) as usize,
            &Capabilities::CURRENT,
            Access::ReadOnly,
        ) {
            Ok(Slot::Root(root)) if root.generation == prev => Ok(root),
            _ => Err(unavailable()),
        }
    }

    /// The set of everything reachable from a root this file can recover to
    /// (S9-17): the writable head, every named snapshot, **both** recoverable
    /// root generations (current and previous, with the snapshots each of them
    /// names), and the maps of all of these. A snapshot deleted in the current
    /// root is still in the previous root's table, so its blocks stay in the
    /// set until a later commit makes the previous root stop naming it.
    ///
    /// Outside this set (the bytes of the file not covered by `data_blocks` and
    /// `extents`) is *provably unreachable from this file's roots*; it is only
    /// a candidate for reclamation. What the set cannot know and a host must
    /// add: closed child files that name a snapshot of this file (they pin it
    /// by [`ParentRef`]; a catalogue is S9-16 follow-up work), and reader or
    /// export leases (flock; they exclude the exclusive writer that compaction
    /// needs, so a held lease means no compaction at all). `parents` lists the
    /// ancestor snapshots this file pins in *other* files (clone parents).
    pub fn rootset(&self) -> Result<Rootset, SnapError> {
        let storage = self.file.storage();
        let generation = self.file.generation();
        let mut rs = Rootset {
            generations: [generation, generation.saturating_sub(1)],
            ..Rootset::default()
        };
        // Head.
        for i in 0..self.file.block_count() {
            if let Some(phys) = self.file.block_offset(i) {
                rs.data_blocks.insert(phys);
            }
        }
        rs.add_extents(
            self.file.map_offset(),
            self.file.block_count(),
            self.file.end_offset(),
        )?;
        for (id, level) in &self.snapshots {
            let Some(rec) = self.snapshot_record(*id) else {
                return Err(SnapError::SnapshotCorrupt { id: *id });
            };
            rs.add_level(&rec, level)?;
        }
        // Previous recoverable root and the snapshots it names.
        let prev = self.previous_root()?;
        let map = parse_map(&StorageSource(storage), &prev).map_err(parse_failure)?;
        rs.data_blocks.extend(map.iter().map(|d| d.phys));
        rs.add_extents(prev.map_offset, prev.block_count, prev.end_offset)?;
        if let Some(h) = map.first() {
            let header = decode_header(&read_desc(storage, h)?)?;
            for rec in &header.snapshots {
                let level = load_level(storage, rec)?;
                rs.add_level(rec, &level)?;
            }
        }
        // Clone parents: what this file's own chain pins elsewhere.
        if let Some(p) = self.header.parent {
            rs.parents.push(p);
        }
        for a in self.chain.iter() {
            if let Some(p) = a.level.header.parent
                && !rs.parents.contains(&p)
            {
                rs.parents.push(p);
            }
        }
        Ok(rs)
    }

    /// Copy-compaction into a **new** file (S9-17). Offline: `self` is the
    /// exclusive writer, nothing else may hold the source.
    ///
    /// The target receives a new `new_uid` and the logical content (own
    /// blocks, holes, disk size, parent reference) of the head and of every
    /// named snapshot **with the same snapshot ids**, rebuilt in the order of
    /// the source's generations so that each snapshot is a real retained root
    /// of the target. Unreachable bytes (blocks of deleted snapshots and of
    /// overwritten data, old maps) are simply not copied.
    ///
    /// Publication: the target is built with a *building* header that no open
    /// accepts ([`SnapError::BuildInProgress`]). When everything is written,
    /// the target storage is released and **reopened from the backend**
    /// (full CoW recovery with CRC verification of every reachable block, the
    /// header, bitmap and every retained snapshot parsed again from media,
    /// the reopened header required to equal the one that was written), and
    /// every snapshot and the head of that reopened file are compared byte for
    /// byte with the source (through the same parent chain). Only then does
    /// one final commit, on the reopened file, clear the flag and set the
    /// source's `next_snapshot_id`; a mismatch or any corruption found on the
    /// way is an error and the flag stays set. A crash earlier
    /// leaves a target that does not open (absent); after the returned `Ok`
    /// the target is complete. The source is only read: its bytes do not
    /// change, and it must be kept until the caller has confirmed the target.
    ///
    /// **Children are not reconnected.** The target has a different file uid
    /// (and different map CRCs), so a child that recorded the source as its
    /// parent refuses to open against the target
    /// ([`SnapError::ParentIdentityMismatch`]); rewriting children is a
    /// separate, explicit operation that does not exist. For a child source the
    /// target keeps the same [`ParentRef`] and shares the verified chain.
    ///
    /// Not covered: replacing the source by the target in the namespace (needs
    /// an atomic durable rename, ISSUE-055), freeing the source, power loss.
    pub fn compact_to<T: StorageBackend>(
        &self,
        target: CheckedStorage<T>,
        new_uid: u64,
    ) -> Result<(SnapFile<T>, DurableAck), SnapError> {
        let out = self.compact_build(target, new_uid)?;
        self.compact_publish(out)
    }

    /// First half of [`compact_to`](Self::compact_to): writes the target up
    /// to and including the head state; the target is still *building*.
    pub(crate) fn compact_build<T: StorageBackend>(
        &self,
        target: CheckedStorage<T>,
        new_uid: u64,
    ) -> Result<SnapFile<T>, SnapError> {
        if self.is_poisoned() {
            return Err(SnapError::BadWrite("compaction source is poisoned"));
        }
        if new_uid == 0 || new_uid == self.header.file_uid {
            return Err(SnapError::BadWrite("compaction needs a new file uid"));
        }
        if self
            .chain
            .iter()
            .any(|a| a.level.header.file_uid == new_uid)
        {
            return Err(SnapError::ParentCycle);
        }
        let (mut out, _ack) = SnapFile::init_shared(
            target,
            Header {
                building: true,
                file_uid: new_uid,
                disk_blocks: 0,
                next_snapshot_id: 1,
                depth: self.header.depth,
                parent: self.header.parent,
                snapshots: Vec::new(),
            },
            Rc::clone(&self.chain),
        )?;
        // Snapshots in generation (= id) order, then the head.
        for (id, level) in &self.snapshots {
            out.apply_state(
                level.header.disk_blocks,
                &level.presence,
                &mut |b| match level.data_desc(b) {
                    Some(d) => read_desc(self.file.storage(), d),
                    None => Err(SnapError::SnapshotCorrupt { id: *id }),
                },
            )?;
            let next = id.checked_add(1).ok_or(SnapError::SnapshotIdExhausted)?;
            out.publish_snapshot(*id, next)?;
        }
        out.apply_state(self.header.disk_blocks, &self.presence, &mut |b| {
            let index = b.checked_add(DATA_BASE).ok_or(SnapError::OutOfRange)?;
            block_of(self.file.read_block(index)?)
        })?;
        Ok(out)
    }

    /// Second half: verifies a built target from the media and publishes it.
    pub(crate) fn compact_publish<T: StorageBackend>(
        &self,
        out: SnapFile<T>,
    ) -> Result<(SnapFile<T>, DurableAck), SnapError> {
        let mut out = out.reopen_from_media()?;
        self.verify_same_content(&out)?;
        let mut header = out.header.clone();
        header.building = false;
        header.next_snapshot_id = self.header.next_snapshot_id;
        let ack = out.file.commit(&[(0, encode_header(&header).as_slice())])?;
        out.header = header;
        Ok((out, ack))
    }

    /// Releases the storage and opens the same media again (writable CoW
    /// recovery, which verifies every reachable block's CRC), re-reads the
    /// header, bitmap and retained snapshots, and requires the header and
    /// bitmap to equal what this object believed it had written. Accepts a
    /// *building* header; nothing is written unless recovery has to repair.
    fn reopen_from_media(self) -> Result<Self, SnapError> {
        let Self {
            file,
            header,
            presence,
            chain,
            policy,
            ..
        } = self;
        if file.is_poisoned() {
            return Err(SnapError::Cow(CowError::Poisoned));
        }
        let backend = file
            .into_storage()
            .into_backend()
            .map_err(|_| SnapError::BadWrite("target storage needs reconcile"))?;
        let storage = CheckedStorage::new(backend).map_err(|e| SnapError::Cow(e.into()))?;
        let (file, _report) = CowFile::open(storage)?;
        let (media_header, media_presence) = read_state_with(&file, true)?;
        if media_header != header || media_presence != presence {
            return Err(SnapError::CompactionMismatch { snapshot: None });
        }
        let snapshots = load_own_snapshots(&file, &media_header)?;
        Ok(Self {
            file,
            header: media_header,
            presence: media_presence,
            snapshots,
            chain,
            policy,
        })
    }

    /// Compares every named snapshot and the head of `self` with `other`
    /// through each side's own reads.
    fn verify_same_content<T: StorageBackend>(&self, other: &SnapFile<T>) -> Result<(), SnapError> {
        let mismatch = |snapshot| SnapError::CompactionMismatch { snapshot };
        if self.snapshot_ids() != other.snapshot_ids() {
            return Err(mismatch(None));
        }
        for id in self.snapshot_ids() {
            let blocks = self.snapshot_disk_blocks(id);
            if blocks != other.snapshot_disk_blocks(id) {
                return Err(mismatch(Some(id)));
            }
            let len = image_len(blocks.unwrap_or(0))?;
            let (mut a, mut b) = (vec![0u8; len], vec![0u8; len]);
            self.read_snapshot_at(id, 0, &mut a)?;
            other.read_snapshot_at(id, 0, &mut b)?;
            if a != b {
                return Err(mismatch(Some(id)));
            }
        }
        if self.header.disk_blocks != other.header.disk_blocks {
            return Err(mismatch(None));
        }
        let len = image_len(self.header.disk_blocks)?;
        let (mut a, mut b) = (vec![0u8; len], vec![0u8; len]);
        self.read_at(0, &mut a)?;
        other.read_at(0, &mut b)?;
        if a != b {
            return Err(mismatch(None));
        }
        Ok(())
    }

    /// Moves the head of this (building) file to the given state with as few
    /// copied blocks as possible and in commits of at most `MAX_TX_BLOCKS - 2`
    /// data blocks. The disk size may only grow; presence may shrink (a discard
    /// clears bits; the block keeps its placeholder). Bitmap and header go
    /// with the last commit, so every
    /// intermediate commit keeps presence within the stored blocks.
    fn apply_state(
        &mut self,
        disk_blocks: u64,
        presence: &Block,
        get: &mut dyn FnMut(u64) -> Result<Block, SnapError>,
    ) -> Result<(), SnapError> {
        let not_successor = SnapError::BadWrite("state is not a successor of the head");
        // Presence may shrink between states (a discard clears bits): a cleared
        // block keeps its physical placeholder, only the bitmap changes.
        if disk_blocks < self.header.disk_blocks {
            return Err(not_successor);
        }
        let have = self.file.block_count();
        let top = (0..disk_blocks).rev().find(|b| bit(presence, *b));
        let required = top.map_or(have, |t| (t + 1 + DATA_BASE).max(have));
        let mut writes: BTreeMap<u64, Block> = BTreeMap::new();
        for b in 0..disk_blocks {
            if !bit(presence, b) {
                continue;
            }
            let want = get(b)?;
            let index = b + DATA_BASE;
            let same = bit(&self.presence, b)
                && block_of(self.file.read_block(index)?).is_ok_and(|cur| cur == want);
            if !same {
                writes.insert(index, want);
            }
        }
        for index in have..required {
            writes.entry(index).or_insert([0u8; BLOCK_SIZE]);
        }
        let header_changed = disk_blocks != self.header.disk_blocks;
        let presence_changed = *presence != self.presence;
        if writes.is_empty() && !header_changed && !presence_changed {
            return Ok(());
        }
        let mut header = self.header.clone();
        header.disk_blocks = disk_blocks;
        let entries: Vec<(u64, Block)> = writes.into_iter().collect();
        let chunks: Vec<&[(u64, Block)]> = if entries.is_empty() {
            vec![&[]]
        } else {
            entries.chunks(MAX_TX_BLOCKS - 2).collect()
        };
        let last = chunks.len().saturating_sub(1);
        for (i, chunk) in chunks.iter().enumerate() {
            let mut refs: Vec<(u64, &[u8])> =
                chunk.iter().map(|(x, d)| (*x, d.as_slice())).collect();
            let hdr = encode_header(&header);
            if i == last {
                if header_changed {
                    refs.push((0, hdr.as_slice()));
                }
                if presence_changed {
                    refs.push((1, presence.as_slice()));
                }
            }
            if refs.is_empty() {
                continue;
            }
            self.file.commit(&refs)?;
        }
        self.header = header;
        self.presence = *presence;
        Ok(())
    }
}

/// Reachable objects of a file (see [`SnapFile::rootset`]).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Rootset {
    /// Physical offsets of every reachable data block (and of the header and
    /// bitmap blocks of every reachable root, which are ordinary blocks).
    pub data_blocks: BTreeSet<u64>,
    /// `(offset, len)` of every reachable block map and root copy sector.
    pub extents: BTreeSet<(u64, u64)>,
    /// The two recoverable root generations: `[current, previous]`.
    pub generations: [u64; 2],
    /// Ids of snapshots named by any recoverable root.
    pub snapshot_ids: BTreeSet<u64>,
    /// Snapshots of other files pinned by this file's chain.
    pub parents: Vec<ParentRef>,
}

impl Rootset {
    fn add_extents(
        &mut self,
        map_offset: u64,
        block_count: u64,
        end_offset: u64,
    ) -> Result<(), SnapError> {
        if block_count > 0 {
            let len = block_count
                .checked_mul(crate::cow_proto::ENTRY_LEN as u64)
                .ok_or(SnapError::Cow(CowError::LayoutOverflow))?;
            self.extents.insert((map_offset, len));
        }
        if let Some(copy) = end_offset.checked_sub(crate::cow_proto::SLOT_SIZE as u64) {
            self.extents
                .insert((copy, crate::cow_proto::SLOT_SIZE as u64));
        }
        Ok(())
    }

    fn add_level(&mut self, rec: &SnapshotRecord, level: &Level) -> Result<(), SnapError> {
        self.snapshot_ids.insert(rec.id);
        self.data_blocks.extend(level.map.iter().map(|d| d.phys));
        self.add_extents(rec.map_offset, rec.block_count, rec.end_offset)
    }

    /// Bytes the set covers.
    pub fn live_bytes(&self) -> u64 {
        self.data_blocks.len() as u64 * BLOCK_SIZE as u64
            + self.extents.iter().map(|(_, len)| *len).sum::<u64>()
    }
}

/// Byte length of an image of `blocks` blocks, without overflow.
fn image_len(blocks: u64) -> Result<usize, SnapError> {
    usize::try_from(blocks)
        .ok()
        .and_then(|b| b.checked_mul(BLOCK_SIZE))
        .ok_or(SnapError::Cow(CowError::LayoutOverflow))
}

fn parse_failure(e: ParseError) -> SnapError {
    match e.kind {
        ParseKind::Io(s) => SnapError::Cow(CowError::Storage(s)),
        _ => SnapError::Cow(CowError::RootInconsistent("retained root does not parse")),
    }
}

/// Logical image of snapshot `id` as the root `view` recovered to names it,
/// for a **base** file (no parent chain). Used by tests to read what a
/// previous recoverable root still retains.
#[cfg(test)]
pub(crate) fn retained_image<B: StorageBackend>(
    view: &CowView<B>,
    id: u64,
) -> Result<Vec<u8>, SnapError> {
    let header = decode_header(&block_of(view.read_block(0)?)?)?;
    let rec = header
        .snapshots
        .iter()
        .find(|r| r.id == id)
        .ok_or(SnapError::SnapshotMissing { id })?;
    let level = load_level(view.storage(), rec)?;
    let mut out = Vec::new();
    for b in 0..level.header.disk_blocks {
        match level.data_desc(b) {
            Some(d) => out.extend_from_slice(&read_desc(view.storage(), d)?),
            None => out.extend_from_slice(&[0u8; BLOCK_SIZE]),
        }
    }
    Ok(out)
}
