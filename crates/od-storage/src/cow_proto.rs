//! PROTOTYPE of a minimal copy-on-write commit protocol (S9-08).
//!
//! This is **not** a normative disk format. Block size, root layout, the
//! block-map representation and every limit below are hypotheses that the
//! format work (S9-40) may replace. The module exists to prove one thing on
//! the [`FaultBackend`](crate::testing::fault::FaultBackend) failure model:
//! the *commit protocol* never exposes a half-applied transaction and never
//! acknowledges a transaction that a crash can still undo.
//!
//! # Layout (one file)
//!
//! ```text
//! [0 .. 512)      root slot 0   (holds generations 0, 2, 4, ...)
//! [512 .. 1024)   root slot 1   (holds generations 1, 3, 5, ...)
//! [1024 .. E)     append-only area; E is the `end_offset` of the newest root
//! ```
//!
//! A root slot is one 512-byte record (little endian): magic, version, block
//! size, `generation`, logical `block_count`, `map_offset`, CRC-32 of the map,
//! `end_offset`, zero padding, and a CRC-32 over the first 508 bytes of the
//! slot. The block map is a flat array of `{physical_offset: u64, block_crc:
//! u32}` entries, one per logical block, written as a whole on every commit (a
//! "direct entries" map; no tree). Generation `g` always lives in slot `g & 1`,
//! so a commit never touches the slot of the generation it replaces.
//!
//! # Commit protocol
//!
//! A candidate is `[new blocks][block map][root copy]`, appended at `E` of the
//! current root (not at the file size), so published bytes are never
//! overwritten; the file is resized to the candidate's end, which is the new
//! `end_offset`. The *root copy* is the byte-for-byte record that will become
//! the root, stored in the **last sector of the file**.
//!
//! 1. Resize, write blocks, map and root copy.
//! 2. `flush_file_durable` (barrier 1): the candidate is on media.
//! 3. Write the root (generation + 1, CRC) into the inactive slot.
//! 4. `flush_file_durable` (barrier 2). Only its [`DurableAck`] is returned to
//!    the caller; the in-memory state advances only after it.
//!
//! Any error poisons the session: the caller must drop it and reopen, because
//! after a failed flush the durable state is unknown (see [`CheckedStorage`]'s
//! sticky reconcile rule).
//!
//! A commit writes at most [`MAX_TX_BLOCKS`] blocks (checked before any backend
//! call). The caller may cancel at the [`CommitStep`]s before barrier 1 (old
//! state, session usable); after the last step the commit cannot be cancelled.
//! Partial-block writes and multi-range transactions are built on top of this
//! in [`crate::cow_tx`].
//!
//! # Create
//!
//! `create` needs an empty backend. It sizes the file to the header, writes an
//! empty generation-1 root into slot 1 and an empty generation-2 root into
//! slot 0, and issues one durable barrier; only its [`DurableAck`] is
//! returned. The first user commit is generation 3. Before the ack a crash
//! leaves, per slot, blank / torn / written: both roots valid is the clean
//! generation-2 state; exactly one valid empty root in a header-sized file is
//! an unfinished create ([`CowError::IncompleteCreate`] for a writable open,
//! [`CowError::NoValidRoot`] read-only); no valid root is `NoValidRoot`.
//! Because both slots are populated from the start, a blank slot is never
//! normal afterwards.
//!
//! # Recovery and root selection
//!
//! Every byte of metadata is read through the strict parser in
//! [`crate::cow_parse`]; this module owns no second decoder. Open reads both
//! slots and classifies each: blank, structurally valid root, or damaged
//! (wrong magic or CRC: what a torn or rotted root write looks like). A slot
//! that is CRC-valid but cannot be honoured (unknown version, geometry or
//! mandatory feature; impossible fields; generation zero, `u64::MAX` or not
//! belonging to its slot) is a refusal, never "damaged": a torn write cannot
//! produce it. Two CRC-valid slots claiming the same generation are
//! [`CowError::GenerationConflict`]; no tie-break is attempted.
//!
//! The valid root with the highest generation `N` is selected. The other slot
//! must hold generation `N - 1`; anything else valid is
//! [`CowError::RootInconsistent`]. If the other slot is blank or damaged
//! (except the unfinished create above), recovery looks at the last sector of
//! the file: a root there that parses, has generation `N + 1` and
//! `end_offset == file size` is the copy stored before barrier 1, so the
//! candidate was complete on media and the damaged slot is its torn root write
//! (or bit-rot of it). Recovery **rolls forward** to it; a writable open first
//! rewrites the copy into the damaged slot and flushes. Without such a copy
//! the damage is unexplained. The map and **every** referenced block are
//! verified against their CRCs afterwards (the whole reachable graph of this
//! format); a failure is a refusal, never a fallback to the other slot and
//! never zero-fill. No valid root is a refusal.
//!
//! A rolled-forward candidate is verified completely like any root: if its
//! map or blocks are damaged the open is refused, not shown as `N` with a
//! flag, because the copy proves the candidate was complete on media.
//!
//! Only a crash before barrier 1 or before the root write leaves the other
//! slot valid (generation `N - 1`): the candidate is simply an unreferenced
//! tail ([`OpenReport::tail_len`]) and `possible_lost_commit` is `false`.
//! [`OpenReport::class`] names the four non-refusal outcomes; corruption and
//! ambiguity are errors.
//!
//! # Writable vs salvage
//!
//! [`CowFile::open`] (parser access `Writable`: every mandatory feature bit must
//! be writable by the caller) returns a writable file only for a state that is
//! explained by the model (clean, or rolled forward and repaired). Unexplained
//! damage is a refusal ([`CowError::DamagedOtherSlot`]); no path commits on top
//! of a damaged slot. [`CowView::salvage`] opens the same media read-only (parser
//! access `ReadOnly`: mandatory bits only need to be understood; it
//! has no `commit` and never writes): it rolls forward in memory when possible,
//! otherwise it shows generation `N` and sets
//! [`OpenReport::possible_lost_commit`] because a newer acknowledged commit may
//! have been lost.
//!
//! # Not covered
//!
//! No garbage collection (old blocks and maps stay in the file), no refcounts,
//! no snapshots, no encryption, and open verifies all blocks eagerly (O(file)). The
//! copy only helps when the file is not shortened or extended after barrier 1
//! by something other than this protocol.

use crate::cow_parse::{
    Access, BLOCK_COUNT_AT, BLOCK_SIZE_AT, BlockDescriptor, Capabilities, END_OFFSET_AT, Features,
    GENERATION_AT, MAGIC_AT, MANDATORY_AT, MAP_CRC_AT, MAP_OFFSET_AT, OPTIONAL_AT, ParseError,
    ParseKind, ReadError, RootRecord, SLOT_CRC_AT, Section, Slot, Source, StorageSource,
    VERSION_AT, parse_map, parse_slot_at, verify_block_data,
};
use crate::io::{CheckedStorage, DurableAck, StorageBackend, StorageError};
use std::ops::Deref;

/// Hypothesis: 512-byte blocks (equal to the fault model's sector so a block is
/// one atomic piece). S9-40 decides the real size.
pub const BLOCK_SIZE: usize = 512;
/// Size of one root slot; also one sector.
pub const SLOT_SIZE: usize = 512;
/// Bytes before the append-only area.
pub const HEADER_LEN: u64 = 2 * SLOT_SIZE as u64;
/// Generation of the empty state `create` leaves behind; generation 1 is its
/// companion in the other slot.
pub const CREATE_GENERATION: u64 = 2;
/// Prototype limit on logical blocks; bounds allocations driven by a root.
pub const MAX_BLOCKS: u64 = 1 << 16;
/// Prototype limit on the blocks one transaction may write (dirty state). A
/// larger `commit` is rejected before any backend call. Hypothesis, like every
/// other limit here.
pub const MAX_TX_BLOCKS: usize = 256;

pub(crate) const MAGIC: [u8; 8] = *b"ODCOWP\0\x01";
pub(crate) const VERSION: u32 = 1;
pub(crate) const ENTRY_LEN: usize = 12;

// ------------------------------------------------------------------ CRC-32

/// CRC-32 as in IEEE 802.3 / ISO 3309 / zlib: reflected polynomial
/// `0xEDB88320`, initial value and final xor `0xFFFFFFFF`. The table is
/// computed here from that polynomial; the check value for the ASCII string
/// `"123456789"` is `0xCBF43926`.
const CRC_TABLE: [u32; 256] = make_crc_table();

const fn make_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c = CRC_TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

// ------------------------------------------------------------------ types

/// State of one root slot as seen by recovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlotState {
    /// Every byte is zero.
    Blank,
    /// Structurally valid root of this generation.
    Valid { generation: u64 },
    /// Not blank, wrong magic or CRC: what a torn or rotted root write looks
    /// like. CRC-valid slots that cannot be honoured are errors, not this.
    Damaged,
}

/// Which step order the test-only `CowFile::commit_with_order` uses. Everything except
/// [`Correct`](Self::Correct) is a deliberately broken protocol that exists
/// only so tests can prove the oracle notices it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitOrder {
    Correct,
    /// Returns the barrier-1 ack and never flushes the root (ack before
    /// barrier 2).
    AckBeforeBarrier2,
    /// Skips barrier 1: candidate and root share one barrier, so a crash may
    /// keep the root and lose the data it points to.
    NoDataBarrier,
}

/// A point inside [`CowFile::commit_cancellable`] where the caller may still
/// cancel. Every point precedes barrier 1; the last one precedes the write of
/// the root copy, so a cancelled commit never leaves a complete, adoptable
/// candidate behind. After `RootCopy` the commit runs to its end and can no
/// longer be cancelled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitStep {
    /// About to resize the file to the candidate length.
    Resize,
    /// About to write the i-th data block of the candidate.
    Block(usize),
    /// About to write the block map.
    Map,
    /// About to write the root copy (last cancel point).
    RootCopy,
}

/// Why `execute` stopped.
enum Abort {
    Failed(CowError),
    /// Cancelled at a [`CommitStep`]; `mutated` tells whether the backend was
    /// already changed (a resize or write happened).
    Cancelled {
        mutated: bool,
    },
}

impl From<StorageError> for Abort {
    fn from(e: StorageError) -> Self {
        Self::Failed(CowError::Storage(e))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenReport {
    /// Generation of the state that was opened.
    pub generation: u64,
    /// Slot that holds (or, after repair, now holds) that generation.
    pub chosen_slot: usize,
    /// The slot that did not hold the opened root, as read from media.
    pub other_slot: SlotState,
    /// The opened state is the root copy found at the end of the file because
    /// the slot of generation `N + 1` was damaged.
    pub rolled_forward: bool,
    /// The other slot is damaged without a valid root copy explaining it: a
    /// newer acknowledged commit may be lost. Only a salvage open reports this
    /// (a writable open refuses instead).
    pub possible_lost_commit: bool,
    /// Bytes between the published end of the opened state and the end of the
    /// file. Non-zero after a crash before the root write (an unfinished
    /// candidate) and after any abandoned attempt; always zero after a
    /// roll-forward.
    pub tail_len: u64,
}

/// How an open that succeeded relates to the media. Corruption and ambiguity
/// never reach this type: they are [`CowError`]s.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenClass {
    /// Coherent committed state, nothing after it.
    Committed,
    /// Coherent committed state; the file holds an unreferenced tail that is
    /// an unfinished (or abandoned) candidate and is ignored.
    UnfinishedCandidate,
    /// The root copy of generation `N + 1` was adopted: the candidate was
    /// complete and only its root write was torn.
    RolledForward,
    /// Read-only salvage of an older root; a newer acknowledged commit may be
    /// lost.
    PossibleLostCommit,
}

impl OpenReport {
    pub fn class(&self) -> OpenClass {
        if self.possible_lost_commit {
            OpenClass::PossibleLostCommit
        } else if self.rolled_forward {
            OpenClass::RolledForward
        } else if self.tail_len > 0 {
            OpenClass::UnfinishedCandidate
        } else {
            OpenClass::Committed
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CowError {
    Storage(StorageError),
    /// A create that was never acknowledged: exactly one slot holds an empty
    /// root, the file is header-sized, and nothing else exists. Only a
    /// writable open reports this; a read-only open sees `NoValidRoot`.
    IncompleteCreate {
        slots: [SlotState; 2],
    },
    /// No slot holds a valid root (also: file shorter than the header).
    NoValidRoot {
        slots: [SlotState; 2],
    },
    /// Writable open refused: the other slot is damaged and no valid root copy
    /// explains it. Use [`CowView::salvage`] for read-only access.
    DamagedOtherSlot {
        chosen_generation: u64,
        other: SlotState,
    },
    /// CRC-valid root with an unknown version or block size; the parser error
    /// names the field.
    UnsupportedRoot(ParseError),
    /// A mandatory feature bit (lowest offending index) this open cannot
    /// honour; writable opens are stricter than read-only ones.
    UnsupportedMandatoryFeature(u8),
    /// Two CRC-valid slots claim the same generation. Neither is preferred.
    GenerationConflict {
        generation: u64,
    },
    /// Root fields are impossible (limits, ranges, generations).
    RootInconsistent(&'static str),
    MapCorrupt,
    MapEntryOutOfRange {
        index: u64,
    },
    /// Two map entries name overlapping or identical physical blocks.
    MapEntryCollision {
        index: u64,
    },
    BlockCorrupt {
        index: u64,
    },
    /// Offset arithmetic of a new candidate overflowed.
    LayoutOverflow,
    /// A transaction wrote more than [`MAX_TX_BLOCKS`] blocks. Rejected before
    /// any backend call; the session is still usable.
    TxTooLarge {
        blocks: usize,
        limit: usize,
    },
    /// The caller cancelled inside `commit` before barrier 1. The published
    /// state is unchanged and the session is still usable.
    Cancelled,
    /// Rejected `commit`/`create` input; the session is still usable.
    BadWrite(&'static str),
    /// An earlier commit failed; reopen the file.
    Poisoned,
}

impl From<StorageError> for CowError {
    fn from(e: StorageError) -> Self {
        Self::Storage(e)
    }
}

impl std::fmt::Display for CowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cow prototype error: {self:?}")
    }
}

impl std::error::Error for CowError {}

/// Byte offset of the slot that holds `generation`.
pub fn slot_offset(generation: u64) -> u64 {
    (generation & 1) * SLOT_SIZE as u64
}

/// A root as the writer builds it: no features, located in its own slot.
fn new_root(
    generation: u64,
    block_count: u64,
    map_offset: u64,
    map_crc: u32,
    end_offset: u64,
) -> RootRecord {
    RootRecord {
        slot_offset: slot_offset(generation),
        generation,
        block_count,
        map_offset,
        map_crc,
        end_offset,
        features: Features::default(),
    }
}

/// Builds a CRC-valid root sector from raw field values. Exported only so
/// parser tests can craft structurally invalid but CRC-valid roots.
pub fn forge_root_sector(
    generation: u64,
    block_count: u64,
    map_offset: u64,
    map_crc: u32,
    end_offset: u64,
) -> [u8; SLOT_SIZE] {
    encode_root(&new_root(
        generation,
        block_count,
        map_offset,
        map_crc,
        end_offset,
    ))
}

fn encode_root(root: &RootRecord) -> [u8; SLOT_SIZE] {
    fn put(s: &mut [u8; SLOT_SIZE], at: usize, bytes: &[u8]) {
        if let Some(dst) = s.get_mut(at..at + bytes.len()) {
            dst.copy_from_slice(bytes);
        }
    }
    let mut s = [0u8; SLOT_SIZE];
    put(&mut s, MAGIC_AT, &MAGIC);
    put(&mut s, VERSION_AT, &VERSION.to_le_bytes());
    put(&mut s, BLOCK_SIZE_AT, &(BLOCK_SIZE as u32).to_le_bytes());
    put(&mut s, GENERATION_AT, &root.generation.to_le_bytes());
    put(&mut s, BLOCK_COUNT_AT, &root.block_count.to_le_bytes());
    put(&mut s, MAP_OFFSET_AT, &root.map_offset.to_le_bytes());
    put(&mut s, MAP_CRC_AT, &root.map_crc.to_le_bytes());
    put(&mut s, END_OFFSET_AT, &root.end_offset.to_le_bytes());
    put(&mut s, MANDATORY_AT, &root.features.mandatory.to_le_bytes());
    put(&mut s, OPTIONAL_AT, &root.features.optional.to_le_bytes());
    let crc = crc32(s.get(..SLOT_CRC_AT).unwrap_or(&[]));
    put(&mut s, SLOT_CRC_AT, &crc.to_le_bytes());
    s
}

fn encode_map(map: &[BlockDescriptor]) -> Vec<u8> {
    let mut out = Vec::with_capacity(map.len() * ENTRY_LEN);
    for e in map {
        out.extend_from_slice(&e.phys.to_le_bytes());
        out.extend_from_slice(&e.crc.to_le_bytes());
    }
    out
}

fn read_vec<B: StorageBackend>(
    storage: &CheckedStorage<B>,
    offset: u64,
    len: usize,
) -> Result<Vec<u8>, CowError> {
    let mut v = vec![0u8; len];
    storage.read_exact_at(offset, &mut v)?;
    Ok(v)
}

fn align_up(value: u64) -> Result<u64, CowError> {
    value
        .checked_next_multiple_of(BLOCK_SIZE as u64)
        .ok_or(CowError::LayoutOverflow)
}

// ------------------------------------------------------------ root reading

/// Result of reading one slot through the strict parser.
enum SlotRead {
    Blank,
    Valid(RootRecord),
    /// Wrong magic or CRC: a torn or rotted write.
    Damaged,
    /// CRC-valid root whose generation belongs to the other slot.
    Misplaced(u64),
}

fn read_error(e: ReadError) -> CowError {
    match e {
        ReadError::Storage(s) => CowError::Storage(s),
        ReadError::OutOfBounds => CowError::RootInconsistent("slot outside file"),
    }
}

/// Maps a parser failure of a CRC-valid root (or of one of its root-declared
/// fields) to a refusal. `base` is the file offset the root was read from.
fn root_error(e: ParseError, base: u64) -> CowError {
    match e.kind {
        ParseKind::Io(s) => CowError::Storage(s),
        ParseKind::Version { .. } | ParseKind::Identity => CowError::UnsupportedRoot(e),
        ParseKind::UnsupportedMandatoryFeature(bit) => CowError::UnsupportedMandatoryFeature(bit),
        ParseKind::Reserved => CowError::RootInconsistent("reserved bytes not zero"),
        ParseKind::Generation => CowError::RootInconsistent("generation not valid for its slot"),
        _ => CowError::RootInconsistent(
            match usize::try_from(e.offset.saturating_sub(base)).unwrap_or(usize::MAX) {
                BLOCK_COUNT_AT => "block_count invalid",
                MAP_OFFSET_AT => "map_offset invalid",
                END_OFFSET_AT => "end_offset invalid",
                _ => "root field invalid",
            },
        ),
    }
}

/// Maps a failure of [`parse_map`] (root fields, map CRC, descriptors).
fn map_error(e: ParseError, root: &RootRecord) -> CowError {
    let entry = || e.offset.saturating_sub(root.map_offset) / ENTRY_LEN as u64;
    match (e.section, e.kind) {
        (_, ParseKind::Io(s)) => CowError::Storage(s),
        (Section::Slot, _) => root_error(e, root.slot_offset),
        (Section::Map, _) => CowError::MapCorrupt,
        (_, ParseKind::Overlap { .. } | ParseKind::Duplicate { .. }) => {
            CowError::MapEntryCollision { index: entry() }
        }
        _ => CowError::MapEntryOutOfRange { index: entry() },
    }
}

fn read_slot<S: Source + ?Sized>(
    src: &S,
    slot: usize,
    caps: &Capabilities,
    access: Access,
) -> Result<SlotRead, CowError> {
    let offset = (slot * SLOT_SIZE) as u64;
    match parse_slot_at(src, offset, slot, caps, access) {
        Ok(Slot::Blank) => Ok(SlotRead::Blank),
        Ok(Slot::Root(root)) => Ok(SlotRead::Valid(root)),
        Err(e) => match e.kind {
            ParseKind::Magic { .. } | ParseKind::Crc { .. } => Ok(SlotRead::Damaged),
            ParseKind::Generation => {
                let mut raw = [0u8; 8];
                src.read_at(offset + GENERATION_AT as u64, &mut raw)
                    .map_err(read_error)?;
                match u64::from_le_bytes(raw) {
                    0 => Err(CowError::RootInconsistent("generation zero")),
                    u64::MAX => Err(CowError::RootInconsistent("generation at maximum")),
                    g => Ok(SlotRead::Misplaced(g)),
                }
            }
            _ => Err(root_error(e, offset)),
        },
    }
}

/// The root a recovery decided on, before its map is read.
struct Selection {
    chosen: RootRecord,
    /// State of the slot that does not hold `chosen`.
    other_slot: SlotState,
    /// The other slot is damaged or blank.
    damaged: bool,
    /// Root copy of `chosen.generation + 1` that explains the damage.
    forward: Option<RootRecord>,
}

fn select_root<S: Source + ?Sized>(
    src: &S,
    size: u64,
    caps: &Capabilities,
    access: Access,
) -> Result<Selection, CowError> {
    let reads = [
        read_slot(src, 0, caps, access)?,
        read_slot(src, 1, caps, access)?,
    ];
    for (slot, read) in reads.iter().enumerate() {
        if let SlotRead::Misplaced(generation) = read {
            return Err(match &reads[1 - slot] {
                SlotRead::Valid(other) if other.generation == *generation => {
                    CowError::GenerationConflict {
                        generation: *generation,
                    }
                }
                _ => CowError::RootInconsistent("generation does not belong to its slot"),
            });
        }
    }
    let state = |read: &SlotRead| match read {
        SlotRead::Blank => SlotState::Blank,
        SlotRead::Valid(r) => SlotState::Valid {
            generation: r.generation,
        },
        SlotRead::Damaged | SlotRead::Misplaced(_) => SlotState::Damaged,
    };
    let states = [state(&reads[0]), state(&reads[1])];
    let [r0, r1] = reads;
    let (chosen_slot, chosen) = match (r0, r1) {
        (SlotRead::Valid(a), SlotRead::Valid(b)) if a.generation > b.generation => (0, a),
        (SlotRead::Valid(_), SlotRead::Valid(b)) => (1, b),
        (SlotRead::Valid(a), _) => (0, a),
        (_, SlotRead::Valid(b)) => (1, b),
        _ => return Err(CowError::NoValidRoot { slots: states }),
    };
    let other_index = 1 - chosen_slot;
    let other_slot = states[other_index];
    // Generations alternate slots, so the other slot holds `N - 1`.
    if let SlotState::Valid { generation } = other_slot
        && generation != chosen.generation - 1
    {
        return Err(CowError::RootInconsistent(
            "valid slots are not consecutive generations",
        ));
    }
    // A lone root of an empty, header-sized file is an unfinished `create`
    // (both roots share one barrier): nothing was ever acknowledged.
    if !matches!(other_slot, SlotState::Valid { .. })
        && chosen.generation <= CREATE_GENERATION
        && chosen.block_count == 0
        && chosen.end_offset == HEADER_LEN
        && size == HEADER_LEN
    {
        return Err(CowError::IncompleteCreate { slots: states });
    }
    // Both slots are always populated after `create`, so a blank other slot
    // is damage like any other (a root that rotted to zeros or never landed).
    let damaged = !matches!(other_slot, SlotState::Valid { .. });
    let forward = if damaged {
        find_root_copy(src, size, &chosen, other_index, caps, access)?
    } else {
        None
    };
    Ok(Selection {
        chosen,
        other_slot,
        damaged,
        forward,
    })
}

/// The root copy of generation `chosen + 1` in the last sector, if it parses.
/// A copy that is merely unreadable as a root is "no copy"; a storage read
/// failure is an error.
fn find_root_copy<S: Source + ?Sized>(
    src: &S,
    size: u64,
    chosen: &RootRecord,
    other_slot: usize,
    caps: &Capabilities,
    access: Access,
) -> Result<Option<RootRecord>, CowError> {
    let last = match size.checked_sub(SLOT_SIZE as u64) {
        Some(last) if last >= chosen.end_offset => last,
        _ => return Ok(None),
    };
    match parse_slot_at(src, last, other_slot, caps, access) {
        Ok(Slot::Root(copy))
            if chosen.generation.checked_add(1) == Some(copy.generation)
                && copy.end_offset == size =>
        {
            Ok(Some(copy))
        }
        Ok(_) => Ok(None),
        Err(ParseError {
            kind: ParseKind::Io(s),
            ..
        }) => Err(CowError::Storage(s)),
        Err(_) => Ok(None),
    }
}

// ------------------------------------------------------------------ view

/// Read-only verified state of a file. It has no way to write: it exposes no
/// `&mut` access to the storage.
pub struct CowView<B: StorageBackend> {
    storage: CheckedStorage<B>,
    generation: u64,
    map: Vec<BlockDescriptor>,
    map_offset: u64,
    end_offset: u64,
    /// Feature words of the current root; every commit carries them forward.
    features: Features,
}

struct Recovered<B: StorageBackend> {
    view: CowView<B>,
    report: OpenReport,
    /// Root to write into the damaged slot (roll-forward repair).
    repair: Option<RootRecord>,
}

impl<B: StorageBackend> CowView<B> {
    /// Read-only open of possibly damaged media; never writes. See the module
    /// docs ("Writable vs salvage").
    pub fn salvage(storage: CheckedStorage<B>) -> Result<(Self, OpenReport), CowError> {
        Self::salvage_with(storage, &Capabilities::CURRENT)
    }

    /// Side-effect-free inspection of the media before any mutation: the same
    /// read-only recovery as [`salvage`](Self::salvage). The report predicts
    /// what a writable open would do: `rolled_forward` means it would rewrite
    /// exactly one root slot and flush; any other outcome means it writes
    /// nothing.
    pub fn inspect(storage: CheckedStorage<B>) -> Result<(Self, OpenReport), CowError> {
        Self::salvage(storage)
    }

    /// [`salvage`](Self::salvage) for a caller that understands more mandatory
    /// feature bits than this build defines.
    pub fn salvage_with(
        storage: CheckedStorage<B>,
        caps: &Capabilities,
    ) -> Result<(Self, OpenReport), CowError> {
        let recovered = Self::recover(storage, caps, Access::ReadOnly)?;
        Ok((recovered.view, recovered.report))
    }

    fn recover(
        storage: CheckedStorage<B>,
        caps: &Capabilities,
        access: Access,
    ) -> Result<Recovered<B>, CowError> {
        let size = storage.size()?;
        if size < HEADER_LEN {
            return Err(CowError::NoValidRoot {
                slots: [SlotState::Blank; 2],
            });
        }
        let src = StorageSource(&storage);
        let selection = match select_root(&src, size, caps, access) {
            // Read-only: an unfinished create has no state to show.
            Err(CowError::IncompleteCreate { slots }) if access == Access::ReadOnly => {
                return Err(CowError::NoValidRoot { slots });
            }
            other => other?,
        };
        // Eager verification: the map and every referenced block must read
        // and match their CRCs. Failure refuses the open; the other slot is
        // not consulted.
        let load = |root: &RootRecord| -> Result<Vec<BlockDescriptor>, CowError> {
            let map = parse_map(&src, root).map_err(|e| map_error(e, root))?;
            for (index, block) in map.iter().enumerate() {
                verify_block_data(&src, block).map_err(|e| match e.kind {
                    ParseKind::Io(s) => CowError::Storage(s),
                    _ => CowError::BlockCorrupt {
                        index: index as u64,
                    },
                })?;
            }
            Ok(map)
        };
        // A candidate behind a damaged slot is complete on media (the copy was
        // flushed before the root write), so damage in it is a refusal, never
        // a fallback to the chosen root.
        let (repair, root, map) = match selection.forward {
            Some(copy) => (Some(copy), copy, load(&copy)?),
            None => (None, selection.chosen, load(&selection.chosen)?),
        };
        let report = OpenReport {
            generation: root.generation,
            chosen_slot: (root.generation & 1) as usize,
            other_slot: selection.other_slot,
            rolled_forward: repair.is_some(),
            possible_lost_commit: selection.damaged && repair.is_none(),
            tail_len: size.saturating_sub(root.end_offset),
        };
        let view = CowView {
            storage,
            generation: root.generation,
            map,
            map_offset: root.map_offset,
            end_offset: root.end_offset,
            features: root.features,
        };
        Ok(Recovered {
            view,
            report,
            repair,
        })
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Feature words of the opened (or last committed) root.
    pub fn features(&self) -> Features {
        self.features
    }

    pub fn block_count(&self) -> u64 {
        self.map.len() as u64
    }

    /// Offset of the current block map (for corruption tests).
    pub fn map_offset(&self) -> u64 {
        self.map_offset
    }

    /// End of the published area (`end_offset` of the current root).
    pub fn end_offset(&self) -> u64 {
        self.end_offset
    }

    /// CRC-32 of the current block map as the root stores it. Together with
    /// the generation and block count it identifies the content of this state
    /// (it covers every physical offset and block CRC).
    pub fn map_crc(&self) -> u32 {
        crc32(&encode_map(&self.map))
    }

    /// Read access to the checked storage, for layers that read retained
    /// older states (snapshots). There is no mutable counterpart.
    pub(crate) fn storage(&self) -> &CheckedStorage<B> {
        &self.storage
    }

    /// Physical offset of a published block (for corruption tests).
    pub fn block_offset(&self, index: u64) -> Option<u64> {
        self.map.get(index as usize).map(|e| e.phys)
    }

    /// Reads one block and verifies its CRC; never returns unchecked bytes.
    pub fn read_block(&self, index: u64) -> Result<Vec<u8>, CowError> {
        let entry = usize::try_from(index)
            .ok()
            .and_then(|i| self.map.get(i))
            .ok_or(CowError::BadWrite("block index out of range"))?;
        let data = read_vec(&self.storage, entry.phys, BLOCK_SIZE)?;
        if crc32(&data) != entry.crc {
            return Err(CowError::BlockCorrupt { index });
        }
        Ok(data)
    }

    /// All blocks in logical order.
    pub fn snapshot(&self) -> Result<Vec<Vec<u8>>, CowError> {
        (0..self.block_count())
            .map(|i| self.read_block(i))
            .collect()
    }
}

// ------------------------------------------------------------------ file

struct Plan {
    generation: u64,
    map: Vec<BlockDescriptor>,
    map_offset: u64,
    map_bytes: Vec<u8>,
    copy_offset: u64,
    new_len: u64,
    /// Physical offset of the i-th element of the caller's `writes`.
    block_offsets: Vec<u64>,
}

/// An open, writable copy-on-write file. Not `Clone`; one writer. Read access
/// is through `Deref` to [`CowView`].
pub struct CowFile<B: StorageBackend> {
    view: CowView<B>,
    poisoned: bool,
}

impl<B: StorageBackend> Deref for CowFile<B> {
    type Target = CowView<B>;

    fn deref(&self) -> &CowView<B> {
        &self.view
    }
}

impl<B: StorageBackend> CowFile<B> {
    /// Creates an empty file (generation 2, zero blocks) over an empty
    /// backend. Until the returned ack exists the file may not open at all.
    pub fn create(mut storage: CheckedStorage<B>) -> Result<(Self, DurableAck), CowError> {
        if storage.size()? != 0 {
            return Err(CowError::BadWrite("create needs an empty backend"));
        }
        storage.set_len(HEADER_LEN)?;
        // Two empty roots under one barrier; the first user commit is
        // generation 3. Neither root alone is a created file.
        for generation in [CREATE_GENERATION - 1, CREATE_GENERATION] {
            let root = new_root(generation, 0, HEADER_LEN, crc32(&[]), HEADER_LEN);
            storage.write_all_at(slot_offset(generation), &encode_root(&root))?;
        }
        let ack = storage.flush_file_durable()?;
        let view = CowView {
            storage,
            generation: CREATE_GENERATION,
            map: Vec::new(),
            map_offset: HEADER_LEN,
            end_offset: HEADER_LEN,
            features: Features::default(),
        };
        Ok((
            Self {
                view,
                poisoned: false,
            },
            ack,
        ))
    }

    /// Writable recovery. Refuses unexplained damage; repairs a damaged slot
    /// from a valid root copy (and flushes) before returning. See the module
    /// docs.
    pub fn open(storage: CheckedStorage<B>) -> Result<(Self, OpenReport), CowError> {
        Self::open_with(storage, &Capabilities::CURRENT)
    }

    /// [`open`](Self::open) for a caller that can write through the mandatory
    /// feature bits in `caps` (see [`Capabilities`]); any other mandatory bit
    /// refuses with [`CowError::UnsupportedMandatoryFeature`].
    pub fn open_with(
        storage: CheckedStorage<B>,
        caps: &Capabilities,
    ) -> Result<(Self, OpenReport), CowError> {
        Self::open_gated(storage, caps, |_| Ok(()))
    }

    /// [`open_with`](Self::open_with) with a `gate` that sees the recovered
    /// read-only state **before** the one possible write of a writable open
    /// (the roll-forward repair of a damaged root slot). A gate that returns
    /// an error refuses the open with no byte written. Layers above the CoW
    /// file use it to verify what the file depends on (a parent chain) before
    /// anything is repaired.
    pub(crate) fn open_gated<E: From<CowError>>(
        storage: CheckedStorage<B>,
        caps: &Capabilities,
        gate: impl FnOnce(&CowView<B>) -> Result<(), E>,
    ) -> Result<(Self, OpenReport), E> {
        let Recovered {
            mut view,
            report,
            repair,
        } = CowView::recover(storage, caps, Access::Writable)?;
        if report.possible_lost_commit {
            return Err(CowError::DamagedOtherSlot {
                chosen_generation: view.generation,
                other: report.other_slot,
            }
            .into());
        }
        gate(&view)?;
        if let Some(root) = repair {
            view.storage
                .write_all_at(slot_offset(view.generation), &encode_root(&root))
                .map_err(CowError::from)?;
            let _ack = view.storage.flush_file_durable().map_err(CowError::from)?;
        }
        Ok((
            Self {
                view,
                poisoned: false,
            },
            report,
        ))
    }

    /// True after a failed commit: the durable state is unknown and the
    /// caller must drop the session and reopen (reconcile).
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Commits `writes` (logical block index, exactly [`BLOCK_SIZE`] bytes, at
    /// most [`MAX_TX_BLOCKS`] of them). Indexes may overwrite existing blocks
    /// or extend the file with no gap. Returns the barrier-2 [`DurableAck`];
    /// any `Err` poisons the session (except [`CowError::BadWrite`],
    /// [`CowError::TxTooLarge`], [`CowError::LayoutOverflow`] and generation
    /// exhaustion, which are rejected before any I/O).
    pub fn commit(&mut self, writes: &[(u64, &[u8])]) -> Result<DurableAck, CowError> {
        self.commit_inner(writes, CommitOrder::Correct, &mut |_| false)
    }

    /// [`commit`](Self::commit) with a cancel hook. `cancel` is asked at every
    /// [`CommitStep`] before barrier 1; returning `true` abandons the commit:
    /// the unreferenced tail is cut back to the published end, that cut is
    /// made durable (a barrier), and [`CowError::Cancelled`] is returned
    /// without poisoning the session. If the cut or its barrier fails the
    /// session is poisoned and the storage error is returned instead of
    /// `Cancelled` (the caller must reconcile).
    /// Nothing can be cancelled after the last step: a successful return is
    /// a published, durable commit.
    pub fn commit_cancellable(
        &mut self,
        writes: &[(u64, &[u8])],
        cancel: &mut dyn FnMut(CommitStep) -> bool,
    ) -> Result<DurableAck, CowError> {
        self.commit_inner(writes, CommitOrder::Correct, cancel)
    }

    /// [`commit`](Self::commit) with a selectable step order; see
    /// [`CommitOrder`]. Only `Correct` is a protocol; the broken orders exist
    /// for oracle tests and are deliberately not part of the non-test API (no
    /// ack before barrier 2 can be obtained from it).
    #[cfg(test)]
    pub(crate) fn commit_with_order(
        &mut self,
        writes: &[(u64, &[u8])],
        order: CommitOrder,
    ) -> Result<DurableAck, CowError> {
        self.commit_inner(writes, order, &mut |_| false)
    }

    fn commit_inner(
        &mut self,
        writes: &[(u64, &[u8])],
        order: CommitOrder,
        cancel: &mut dyn FnMut(CommitStep) -> bool,
    ) -> Result<DurableAck, CowError> {
        if self.poisoned {
            return Err(CowError::Poisoned);
        }
        let plan = self.plan(writes)?;
        match self.execute(&plan, writes, order, cancel) {
            Ok(ack) => {
                self.view.generation = plan.generation;
                self.view.map = plan.map;
                self.view.map_offset = plan.map_offset;
                self.view.end_offset = plan.new_len;
                Ok(ack)
            }
            Err(Abort::Cancelled { mutated }) => {
                if mutated {
                    // Cut the abandoned candidate back *and make the cut
                    // durable* before reporting Cancelled. Without the barrier
                    // a crash may lose the resize and keep the written
                    // blocks; together with a stale root copy left in the
                    // tail by an earlier attempt (which recovery ignored as
                    // speculative) those blocks could verify and be adopted,
                    // resurrecting a cancelled commit. The simplest fix is
                    // chosen over truncating the ignored tail at open: the
                    // writer that makes a tail is the one that removes it.
                    // If the cut or its barrier fails the file state is
                    // unknown: poison, and the caller sees an error, not
                    // Cancelled.
                    let end = self.view.end_offset;
                    let storage = &mut self.view.storage;
                    let cut = storage
                        .set_len(end)
                        .and_then(|()| storage.flush_file_durable().map(|_ack| ()));
                    if let Err(e) = cut {
                        self.poisoned = true;
                        return Err(CowError::Storage(e));
                    }
                }
                Err(CowError::Cancelled)
            }
            Err(Abort::Failed(e)) => {
                self.poisoned = true;
                Err(e)
            }
        }
    }

    fn plan(&self, writes: &[(u64, &[u8])]) -> Result<Plan, CowError> {
        if writes.len() > MAX_TX_BLOCKS {
            return Err(CowError::TxTooLarge {
                blocks: writes.len(),
                limit: MAX_TX_BLOCKS,
            });
        }
        let generation = self
            .view
            .generation
            .checked_add(1)
            .filter(|g| *g != u64::MAX)
            .ok_or(CowError::RootInconsistent("generation exhausted"))?;
        let mut seen = std::collections::BTreeSet::new();
        let mut count = self.view.map.len() as u64;
        for (index, data) in writes {
            if data.len() != BLOCK_SIZE {
                return Err(CowError::BadWrite("block must be BLOCK_SIZE bytes"));
            }
            if !seen.insert(*index) {
                return Err(CowError::BadWrite("duplicate block index"));
            }
            count = count.max(index.saturating_add(1));
        }
        if count > MAX_BLOCKS {
            return Err(CowError::BadWrite("block count above limit"));
        }
        let mut slots: Vec<Option<BlockDescriptor>> =
            self.view.map.iter().copied().map(Some).collect();
        slots.resize(count as usize, None);

        // Append at the published end, not at the file size: anything beyond
        // it is an unreferenced leftover of a failed attempt.
        let base = self.view.end_offset;
        let mut block_offsets = Vec::with_capacity(writes.len());
        for (i, (index, data)) in writes.iter().enumerate() {
            let phys = (i as u64)
                .checked_mul(BLOCK_SIZE as u64)
                .and_then(|o| base.checked_add(o))
                .ok_or(CowError::LayoutOverflow)?;
            slots[*index as usize] = Some(BlockDescriptor {
                phys,
                crc: crc32(data),
            });
            block_offsets.push(phys);
        }
        let map: Option<Vec<BlockDescriptor>> = slots.into_iter().collect();
        let map = map.ok_or(CowError::BadWrite("write leaves a hole"))?;
        let map_offset = (writes.len() as u64)
            .checked_mul(BLOCK_SIZE as u64)
            .and_then(|o| base.checked_add(o))
            .ok_or(CowError::LayoutOverflow)?;
        let map_bytes = encode_map(&map);
        let map_end = map_offset
            .checked_add(map_bytes.len() as u64)
            .ok_or(CowError::LayoutOverflow)?;
        let copy_offset = align_up(map_end)?;
        let new_len = copy_offset
            .checked_add(SLOT_SIZE as u64)
            .ok_or(CowError::LayoutOverflow)?;
        Ok(Plan {
            generation,
            map,
            map_offset,
            map_bytes,
            copy_offset,
            new_len,
            block_offsets,
        })
    }

    fn execute(
        &mut self,
        plan: &Plan,
        writes: &[(u64, &[u8])],
        order: CommitOrder,
        cancel: &mut dyn FnMut(CommitStep) -> bool,
    ) -> Result<DurableAck, Abort> {
        let mut root = new_root(
            plan.generation,
            plan.map.len() as u64,
            plan.map_offset,
            crc32(&plan.map_bytes),
            plan.new_len,
        );
        // A feature the opener could write through must survive the commit.
        root.features = self.view.features;
        let sector = encode_root(&root);
        let storage = &mut self.view.storage;
        // Step 1: candidate [blocks][map][root copy]; nothing below the
        // published end is touched.
        if cancel(CommitStep::Resize) {
            return Err(Abort::Cancelled { mutated: false });
        }
        storage.set_len(plan.new_len)?;
        for (i, ((_, data), phys)) in writes.iter().zip(&plan.block_offsets).enumerate() {
            if cancel(CommitStep::Block(i)) {
                return Err(Abort::Cancelled { mutated: true });
            }
            storage.write_all_at(*phys, data)?;
        }
        if cancel(CommitStep::Map) {
            return Err(Abort::Cancelled { mutated: true });
        }
        if !plan.map_bytes.is_empty() {
            storage.write_all_at(plan.map_offset, &plan.map_bytes)?;
        }
        // Last cancel point: the root copy is what makes a candidate adoptable
        // by recovery, so nothing may be cancelled once it is written.
        if cancel(CommitStep::RootCopy) {
            return Err(Abort::Cancelled { mutated: true });
        }
        storage.write_all_at(plan.copy_offset, &sector)?;
        // Step 2: barrier 1.
        let barrier1 = match order {
            CommitOrder::NoDataBarrier => None,
            _ => Some(storage.flush_file_durable()?),
        };
        // Step 3: root into the inactive slot.
        storage.write_all_at(slot_offset(plan.generation), &sector)?;
        // Step 4: barrier 2, then ack.
        match (order, barrier1) {
            (CommitOrder::AckBeforeBarrier2, Some(early)) => Ok(early),
            _ => Ok(storage.flush_file_durable()?),
        }
    }
}
