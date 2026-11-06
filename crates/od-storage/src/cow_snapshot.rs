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
//! is S9-17 and does not exist here). Reading a snapshot parses the retained
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
//! # Not covered
//!
//! Snapshot deletion and reclaim (S9-17), branch/checkout/rollback (S9-15),
//! host lock lifecycle for the whole chain, real power-loss behaviour, and
//! sparse physical storage.

use crate::cow_parse::{
    BlockDescriptor, Features, ParseKind, RootRecord, StorageSource, parse_map,
};
use crate::cow_proto::{
    BLOCK_SIZE, CowError, CowFile, CowView, MAX_TX_BLOCKS, OpenReport, crc32, slot_offset,
};
use crate::io::{
    BackendError, CheckedStorage, DurableAck, StorageBackend, StorageCapabilities, StorageError,
};
use std::collections::BTreeMap;

/// Logical CoW blocks before disk block 0 (header, presence bitmap).
pub const DATA_BASE: u64 = 2;
/// Most disk blocks one layer can address: one bit per block in one bitmap
/// block. Hypothesis.
pub const MAX_DISK_BLOCKS: u64 = (BLOCK_SIZE as u64) * 8;
/// Most snapshot records one file holds. Hypothesis; there is no deletion yet.
pub const MAX_SNAPSHOTS: usize = 8;
/// Most ancestors a child may have (a base file has depth 0). Exceeding it is a
/// refusal at open and at create.
pub const MAX_CHAIN_DEPTH: u32 = 8;

const HEADER_MAGIC: [u8; 8] = *b"ODSNAP\0\x01";
const HEADER_VERSION: u32 = 1;
const KIND_BASE: u32 = 0;
const KIND_CHILD: u32 = 1;
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

type Block = [u8; BLOCK_SIZE];

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

/// Decoded layer header (block 0).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Header {
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
    };
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
    let kind = rd_u32(b, 12);
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
        file_uid,
        disk_blocks,
        next_snapshot_id,
        depth,
        parent,
        snapshots,
    })
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

fn read_desc<B: StorageBackend>(
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
struct Level {
    header: Header,
    presence: Block,
    map: Vec<BlockDescriptor>,
}

impl Level {
    fn data_desc(&self, b: u64) -> Option<&BlockDescriptor> {
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
fn load_level<B: StorageBackend>(
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
    if view.block_count() < DATA_BASE {
        return Err(SnapError::NotSnapFile("not initialised"));
    }
    let header = decode_header(&block_of(view.read_block(0)?)?)?;
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
fn resolve_chain<R: ParentResolver>(
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

/// An open writable layer file: a base disk or a child over a parent snapshot.
/// One writer; the ancestors are read-only.
pub struct SnapFile<B: StorageBackend> {
    file: CowFile<B>,
    header: Header,
    presence: Block,
    snapshots: BTreeMap<u64, Level>,
    chain: Vec<Ancestor>,
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

    /// Writes an arbitrary header over an empty backend with no verification.
    /// Exists so tests can forge cycles and depth lies that the checked
    /// constructors refuse.
    pub(crate) fn init(
        storage: CheckedStorage<B>,
        header: Header,
        chain: Vec<Ancestor>,
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
            },
            ack,
        ))
    }

    /// Writable open: CoW recovery, then the header, every retained snapshot
    /// and (for a child) the verified parent chain. Any failure refuses.
    pub fn open<R: ParentResolver>(
        storage: CheckedStorage<B>,
        resolver: &mut R,
    ) -> Result<(Self, OpenReport), SnapError> {
        let (file, report) = CowFile::open(storage)?;
        let (header, presence) = read_state(&file)?;
        let mut snapshots = BTreeMap::new();
        for rec in &header.snapshots {
            if rec.generation >= file.generation() || rec.end_offset > file.end_offset() {
                return Err(SnapError::SnapshotCorrupt { id: rec.id });
            }
            let level = load_level(file.storage(), rec)?;
            if level.header.file_uid != header.file_uid || level.header.parent != header.parent {
                return Err(SnapError::SnapshotCorrupt { id: rec.id });
            }
            snapshots.insert(rec.id, level);
        }
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
        Ok((
            Self {
                file,
                header,
                presence,
                snapshots,
                chain,
            },
            report,
        ))
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

    /// Publishes a snapshot of the current published state and returns its id.
    /// One commit; after a crash the snapshot is whole or absent.
    pub fn create_snapshot(&mut self) -> Result<(u64, DurableAck), SnapError> {
        if self.header.snapshots.len() >= MAX_SNAPSHOTS {
            return Err(SnapError::SnapshotLimit);
        }
        let id = self.header.next_snapshot_id;
        let next = id.checked_add(1).ok_or(SnapError::SnapshotIdExhausted)?;
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
        let mut header = self.header.clone();
        header.snapshots.push(rec);
        header.next_snapshot_id = next;
        let ack = self
            .file
            .commit(&[(0, encode_header(&header).as_slice())])?;
        self.header = header;
        self.snapshots.insert(id, level);
        Ok((id, ack))
    }
}
