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
//! # Recovery
//!
//! Open reads both slots, discards blank and CRC-invalid ones, and takes the
//! valid root with the highest generation `N`. The other slot must be blank
//! (only for `N == 1`) or hold generation `N - 1`; anything else is
//! [`CowError::RootInconsistent`]. If the other slot is blank for `N >= 2` or
//! damaged, recovery looks at the last sector of the file: a CRC-valid root
//! there with generation `N + 1` and `end_offset == file size` is the copy
//! stored before barrier 1, so the candidate was complete on media and the
//! damaged slot is its torn root write (or bit-rot of it). Recovery **rolls
//! forward** to it; a writable open first rewrites the copy into the damaged
//! slot and flushes. Without such a copy the damage is unexplained. The map and
//! **every** referenced block are verified against their CRCs afterwards; a
//! failure is a refusal, never a fallback to the other slot and never
//! zero-fill. No valid root is a refusal.
//!
//! Only a crash before barrier 1 or before the root write leaves the other
//! slot valid (generation `N - 1`): the candidate is simply an unreferenced
//! tail and `possible_lost_commit` is `false`.
//!
//! # Writable vs salvage
//!
//! [`CowFile::open`] returns a writable file only for a state that is
//! explained by the model (clean, or rolled forward and repaired). Unexplained
//! damage is a refusal ([`CowError::DamagedOtherSlot`]); no path commits on top
//! of a damaged slot. [`CowView::salvage`] opens the same media read-only (it
//! has no `commit` and never writes): it rolls forward in memory when possible,
//! otherwise it shows generation `N` and sets
//! [`OpenReport::possible_lost_commit`] because a newer acknowledged commit may
//! have been lost.
//!
//! # Not covered
//!
//! No garbage collection (old blocks and maps stay in the file), no refcounts,
//! no snapshots, no encryption, no parser hardening beyond the cheap limits
//! below (that is S9-09), and open verifies all blocks eagerly (O(file)). The
//! copy only helps when the file is not shortened or extended after barrier 1
//! by something other than this protocol.

use crate::io::{CheckedStorage, DurableAck, StorageBackend, StorageError};
use std::ops::Deref;

/// Hypothesis: 512-byte blocks (equal to the fault model's sector so a block is
/// one atomic piece). S9-40 decides the real size.
pub const BLOCK_SIZE: usize = 512;
/// Size of one root slot; also one sector.
pub const SLOT_SIZE: usize = 512;
/// Bytes before the append-only area.
pub const HEADER_LEN: u64 = 2 * SLOT_SIZE as u64;
/// Prototype limit on logical blocks; bounds allocations driven by a root.
pub const MAX_BLOCKS: u64 = 1 << 16;

const MAGIC: [u8; 8] = *b"ODCOWP\0\x01";
const VERSION: u32 = 1;
const ENTRY_LEN: usize = 12;
const CRC_AT: usize = SLOT_SIZE - 4;

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
    /// CRC-valid root of this generation.
    Valid { generation: u64 },
    /// Not blank and not a valid root (bad CRC, magic or slot parity).
    Damaged,
}

/// Which step order [`CowFile::commit_with_order`] uses. Everything except
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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CowError {
    Storage(StorageError),
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
    /// CRC-valid root with an unknown version or block size.
    UnsupportedRoot {
        version: u32,
        block_size: u32,
    },
    /// Root fields are impossible (limits, ranges, generations).
    RootInconsistent(&'static str),
    MapCorrupt,
    MapEntryOutOfRange {
        index: u64,
    },
    BlockCorrupt {
        index: u64,
    },
    /// Offset arithmetic of a new candidate overflowed.
    LayoutOverflow,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Entry {
    phys: u64,
    crc: u32,
}

#[derive(Clone, Copy, Debug)]
struct Root {
    generation: u64,
    block_count: u64,
    map_offset: u64,
    map_crc: u32,
    end_offset: u64,
}

enum Decoded {
    Blank,
    Valid(Root),
    Damaged,
}

/// Byte offset of the slot that holds `generation`.
pub fn slot_offset(generation: u64) -> u64 {
    (generation & 1) * SLOT_SIZE as u64
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
    encode_root(&Root {
        generation,
        block_count,
        map_offset,
        map_crc,
        end_offset,
    })
}

fn le_u32(b: &[u8], at: usize) -> u32 {
    let mut a = [0u8; 4];
    a.copy_from_slice(&b[at..at + 4]);
    u32::from_le_bytes(a)
}

fn le_u64(b: &[u8], at: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(a)
}

fn encode_root(root: &Root) -> [u8; SLOT_SIZE] {
    let mut s = [0u8; SLOT_SIZE];
    s[0..8].copy_from_slice(&MAGIC);
    s[8..12].copy_from_slice(&VERSION.to_le_bytes());
    s[12..16].copy_from_slice(&(BLOCK_SIZE as u32).to_le_bytes());
    s[16..24].copy_from_slice(&root.generation.to_le_bytes());
    s[24..32].copy_from_slice(&root.block_count.to_le_bytes());
    s[32..40].copy_from_slice(&root.map_offset.to_le_bytes());
    s[40..44].copy_from_slice(&root.map_crc.to_le_bytes());
    s[44..52].copy_from_slice(&root.end_offset.to_le_bytes());
    let crc = crc32(&s[..CRC_AT]);
    s[CRC_AT..].copy_from_slice(&crc.to_le_bytes());
    s
}

fn decode_slot(slot: usize, s: &[u8; SLOT_SIZE]) -> Result<Decoded, CowError> {
    if s.iter().all(|&b| b == 0) {
        return Ok(Decoded::Blank);
    }
    if crc32(&s[..CRC_AT]) != le_u32(s, CRC_AT) || s[0..8] != MAGIC {
        return Ok(Decoded::Damaged);
    }
    let (version, block_size) = (le_u32(s, 8), le_u32(s, 12));
    if version != VERSION || block_size as usize != BLOCK_SIZE {
        return Err(CowError::UnsupportedRoot {
            version,
            block_size,
        });
    }
    let generation = le_u64(s, 16);
    if generation == 0 || (generation & 1) as usize != slot {
        return Ok(Decoded::Damaged);
    }
    if generation == u64::MAX {
        // `generation + 1` must stay representable.
        return Err(CowError::RootInconsistent("generation at maximum"));
    }
    Ok(Decoded::Valid(Root {
        generation,
        block_count: le_u64(s, 24),
        map_offset: le_u64(s, 32),
        map_crc: le_u32(s, 40),
        end_offset: le_u64(s, 44),
    }))
}

fn encode_map(map: &[Entry]) -> Vec<u8> {
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

// ------------------------------------------------------------------ view

/// Read-only verified state of a file. It has no way to write: it exposes no
/// `&mut` access to the storage.
pub struct CowView<B: StorageBackend> {
    storage: CheckedStorage<B>,
    generation: u64,
    map: Vec<Entry>,
    map_offset: u64,
    end_offset: u64,
}

struct Recovered<B: StorageBackend> {
    view: CowView<B>,
    report: OpenReport,
    /// Root sector to write into the damaged slot (roll-forward repair).
    repair: Option<[u8; SLOT_SIZE]>,
}

impl<B: StorageBackend> CowView<B> {
    /// Read-only open of possibly damaged media; never writes. See the module
    /// docs ("Writable vs salvage").
    pub fn salvage(storage: CheckedStorage<B>) -> Result<(Self, OpenReport), CowError> {
        let recovered = Self::recover(storage)?;
        Ok((recovered.view, recovered.report))
    }

    fn recover(storage: CheckedStorage<B>) -> Result<Recovered<B>, CowError> {
        let size = storage.size()?;
        if size < HEADER_LEN {
            return Err(CowError::NoValidRoot {
                slots: [SlotState::Blank; 2],
            });
        }
        let mut roots: [Option<Root>; 2] = [None, None];
        let mut states = [SlotState::Blank; 2];
        for slot in 0..2usize {
            let mut buf = [0u8; SLOT_SIZE];
            storage.read_exact_at((slot * SLOT_SIZE) as u64, &mut buf)?;
            match decode_slot(slot, &buf)? {
                Decoded::Blank => {}
                Decoded::Damaged => states[slot] = SlotState::Damaged,
                Decoded::Valid(root) => {
                    states[slot] = SlotState::Valid {
                        generation: root.generation,
                    };
                    roots[slot] = Some(root);
                }
            }
        }
        let (chosen_slot, chosen) = match (roots[0], roots[1]) {
            (Some(a), Some(b)) if a.generation > b.generation => (0, a),
            (Some(_), Some(b)) => (1, b),
            (Some(a), None) => (0, a),
            (None, Some(b)) => (1, b),
            (None, None) => return Err(CowError::NoValidRoot { slots: states }),
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
        // Blank is normal only for the first generation.
        let damaged = match other_slot {
            SlotState::Damaged => true,
            SlotState::Blank => chosen.generation >= 2,
            SlotState::Valid { .. } => false,
        };
        let forward = if damaged {
            Self::find_root_copy(&storage, &chosen, other_index, size)?
        } else {
            None
        };
        let (root, repair) = match forward {
            Some((root, sector)) => (root, Some(sector)),
            None => (chosen, None),
        };
        let (map, end_offset) = Self::load_map(&storage, &root, size)?;
        let view = CowView {
            storage,
            generation: root.generation,
            map,
            map_offset: root.map_offset,
            end_offset,
        };
        // Eager verification: every referenced block must read and match its
        // CRC. Failure refuses the open; the other slot is not consulted.
        for index in 0..view.map.len() {
            view.read_block(index as u64)?;
        }
        let report = OpenReport {
            generation: root.generation,
            chosen_slot: (root.generation & 1) as usize,
            other_slot,
            rolled_forward: repair.is_some(),
            possible_lost_commit: damaged && repair.is_none(),
        };
        Ok(Recovered {
            view,
            report,
            repair,
        })
    }

    /// The root copy of generation `chosen + 1` in the last sector, if valid.
    /// A copy that is merely unreadable as a root is "no copy"; a storage
    /// read failure is an error.
    fn find_root_copy(
        storage: &CheckedStorage<B>,
        chosen: &Root,
        other_slot: usize,
        size: u64,
    ) -> Result<Option<(Root, [u8; SLOT_SIZE])>, CowError> {
        let last = match size.checked_sub(SLOT_SIZE as u64) {
            Some(last) if last >= chosen.end_offset => last,
            _ => return Ok(None),
        };
        let mut buf = [0u8; SLOT_SIZE];
        storage.read_exact_at(last, &mut buf)?;
        let Ok(Decoded::Valid(copy)) = decode_slot(other_slot, &buf) else {
            return Ok(None);
        };
        if chosen.generation.checked_add(1) == Some(copy.generation) && copy.end_offset == size {
            Ok(Some((copy, buf)))
        } else {
            Ok(None)
        }
    }

    fn load_map(
        storage: &CheckedStorage<B>,
        root: &Root,
        size: u64,
    ) -> Result<(Vec<Entry>, u64), CowError> {
        if root.block_count > MAX_BLOCKS {
            return Err(CowError::RootInconsistent("block_count above limit"));
        }
        let end = root.end_offset;
        if end < HEADER_LEN || !end.is_multiple_of(BLOCK_SIZE as u64) || end > size {
            return Err(CowError::RootInconsistent("end_offset outside file"));
        }
        let count = root.block_count as usize;
        let map_len = count * ENTRY_LEN;
        let map_end = root
            .map_offset
            .checked_add(map_len as u64)
            .ok_or(CowError::RootInconsistent("map range overflows"))?;
        if root.map_offset < HEADER_LEN || map_end > end {
            return Err(CowError::RootInconsistent("map outside file"));
        }
        let bytes = read_vec(storage, root.map_offset, map_len)?;
        if crc32(&bytes) != root.map_crc {
            return Err(CowError::MapCorrupt);
        }
        let mut map = Vec::with_capacity(count);
        for i in 0..count {
            let chunk = &bytes[i * ENTRY_LEN..(i + 1) * ENTRY_LEN];
            let entry = Entry {
                phys: le_u64(chunk, 0),
                crc: le_u32(chunk, 8),
            };
            let block_end = entry.phys.checked_add(BLOCK_SIZE as u64);
            let in_file = entry.phys >= HEADER_LEN
                && entry.phys.is_multiple_of(BLOCK_SIZE as u64)
                && block_end.is_some_and(|e| e <= end);
            if !in_file {
                return Err(CowError::MapEntryOutOfRange { index: i as u64 });
            }
            map.push(entry);
        }
        Ok((map, end))
    }

    pub fn generation(&self) -> u64 {
        self.generation
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
    map: Vec<Entry>,
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
    /// Creates an empty file (generation 1, zero blocks) over an empty
    /// backend. Until the returned ack exists the file may not open at all.
    pub fn create(mut storage: CheckedStorage<B>) -> Result<(Self, DurableAck), CowError> {
        if storage.size()? != 0 {
            return Err(CowError::BadWrite("create needs an empty backend"));
        }
        storage.set_len(HEADER_LEN)?;
        let root = Root {
            generation: 1,
            block_count: 0,
            map_offset: HEADER_LEN,
            map_crc: crc32(&[]),
            end_offset: HEADER_LEN,
        };
        storage.write_all_at(slot_offset(1), &encode_root(&root))?;
        let ack = storage.flush_file_durable()?;
        let view = CowView {
            storage,
            generation: 1,
            map: Vec::new(),
            map_offset: HEADER_LEN,
            end_offset: HEADER_LEN,
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
        let Recovered {
            mut view,
            report,
            repair,
        } = CowView::recover(storage)?;
        if report.possible_lost_commit {
            return Err(CowError::DamagedOtherSlot {
                chosen_generation: view.generation,
                other: report.other_slot,
            });
        }
        if let Some(sector) = repair {
            view.storage
                .write_all_at(slot_offset(view.generation), &sector)?;
            let _ack = view.storage.flush_file_durable()?;
        }
        Ok((
            Self {
                view,
                poisoned: false,
            },
            report,
        ))
    }

    /// Commits `writes` (logical block index, exactly [`BLOCK_SIZE`] bytes).
    /// Indexes may overwrite existing blocks or extend the file with no gap.
    /// Returns the barrier-2 [`DurableAck`]; any `Err` poisons the session
    /// (except [`CowError::BadWrite`], [`CowError::LayoutOverflow`] and
    /// generation exhaustion, which are rejected before any I/O).
    pub fn commit(&mut self, writes: &[(u64, &[u8])]) -> Result<DurableAck, CowError> {
        self.commit_with_order(writes, CommitOrder::Correct)
    }

    /// [`commit`](Self::commit) with a selectable step order; see
    /// [`CommitOrder`]. Only `Correct` is a protocol.
    pub fn commit_with_order(
        &mut self,
        writes: &[(u64, &[u8])],
        order: CommitOrder,
    ) -> Result<DurableAck, CowError> {
        if self.poisoned {
            return Err(CowError::Poisoned);
        }
        let plan = self.plan(writes)?;
        match self.execute(&plan, writes, order) {
            Ok(ack) => {
                self.view.generation = plan.generation;
                self.view.map = plan.map;
                self.view.map_offset = plan.map_offset;
                self.view.end_offset = plan.new_len;
                Ok(ack)
            }
            Err(e) => {
                self.poisoned = true;
                Err(e)
            }
        }
    }

    fn plan(&self, writes: &[(u64, &[u8])]) -> Result<Plan, CowError> {
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
        let mut slots: Vec<Option<Entry>> = self.view.map.iter().copied().map(Some).collect();
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
            slots[*index as usize] = Some(Entry {
                phys,
                crc: crc32(data),
            });
            block_offsets.push(phys);
        }
        let map: Option<Vec<Entry>> = slots.into_iter().collect();
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
    ) -> Result<DurableAck, CowError> {
        let root = Root {
            generation: plan.generation,
            block_count: plan.map.len() as u64,
            map_offset: plan.map_offset,
            map_crc: crc32(&plan.map_bytes),
            end_offset: plan.new_len,
        };
        let sector = encode_root(&root);
        let storage = &mut self.view.storage;
        // Step 1: candidate [blocks][map][root copy]; nothing below the
        // published end is touched.
        storage.set_len(plan.new_len)?;
        for ((_, data), phys) in writes.iter().zip(&plan.block_offsets) {
            storage.write_all_at(*phys, data)?;
        }
        if !plan.map_bytes.is_empty() {
            storage.write_all_at(plan.map_offset, &plan.map_bytes)?;
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
