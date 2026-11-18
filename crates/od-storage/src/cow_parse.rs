//! Strict read-only parser for the CoW v1 layout (S9-09).
//!
//! The layout is CoW v1 described in [`crate::cow_proto`], normative per
//! STORAGE_FORMAT §9. This module never writes and never calls a mutating backend
//! operation: its only inputs are a [`Source`] (a byte slice or a
//! [`CheckedStorage`] seen through its read-only methods) and the explicit
//! [`Capabilities`] of the caller.
//!
//! # Root slot (512 bytes, little endian)
//!
//! ```text
//!   0.. 8  magic                      68..508  reserved, must be zero
//!   8..12  version                   508..512  CRC-32 of bytes 0..508
//!  12..16  block size (identity)
//!  16..24  generation
//!  24..32  block_count
//!  32..40  map_offset
//!  40..44  map CRC-32
//!  44..52  end_offset
//!  52..60  mandatory feature bits
//!  60..68  optional feature bits
//! ```
//!
//! The two feature words occupy bytes that the prototype writes as zero
//! padding, so every file the prototype produces parses as "no features".
//!
//! # Errors and their place
//!
//! Every failure is a [`ParseError`] `{ section, offset, kind }`. `offset` is
//! the absolute file offset of the offending field or structure: a root field
//! for anything the root declares wrongly (including "the file is shorter than
//! the root says"), the map start for a map CRC mismatch, the map entry for a
//! bad block descriptor. [`ParseKind::Truncated`] at a slot reports the start
//! of the slot that cannot be read in full.
//!
//! # Limits
//!
//! [`MAX_BLOCKS`], [`MAX_MAP_BYTES`] and [`MAX_FILE_BYTES`] are checked on the
//! values read from the file **before** any allocation or traversal. The
//! largest buffer the parser ever allocates is the map ([`MAX_MAP_BYTES`]) plus
//! one descriptor per block, and a sort of the descriptors.
//!
//! # Not applicable
//!
//! The prototype has no on-disk references between metadata objects (a root
//! points to one map, a map to data blocks, data blocks to nothing), so there
//! is no graph to walk: cycle and depth limits do not exist here and are not
//! tested. A format with nested maps or snapshot chains must add them.
//!
//! # Not covered
//!
//! Root selection between the two slots, root-copy roll-forward and the
//! publication-status rules belong to S9-10; this module parses one slot (or
//! one root copy) at a time. Data block contents are only checked by
//! [`verify_block_data`]; [`parse_committed`] validates metadata.

use crate::cow_proto::{
    BLOCK_SIZE, ENTRY_LEN, HEADER_LEN, MAGIC, MAX_BLOCKS as PROTO_MAX_BLOCKS, SLOT_SIZE, VERSION,
    crc32,
};
use crate::io::{CheckedStorage, StorageBackend, StorageError};
use std::fmt;

/// Most logical blocks a root may declare (CoW v1 limit, STORAGE_FORMAT §9.3).
pub const MAX_BLOCKS: u64 = PROTO_MAX_BLOCKS;
/// Largest block map the parser reads into memory.
pub const MAX_MAP_BYTES: u64 = MAX_BLOCKS * ENTRY_LEN as u64;
/// Largest published area (`end_offset`) a root may declare (CoW v1 limit).
pub const MAX_FILE_BYTES: u64 = 1 << 32;
/// Number of root slots; generation `g` lives in slot `g & 1`.
pub const SLOT_COUNT: usize = 2;

// Field offsets inside a root slot.
pub const MAGIC_AT: usize = 0;
pub const VERSION_AT: usize = 8;
pub const BLOCK_SIZE_AT: usize = 12;
pub const GENERATION_AT: usize = 16;
pub const BLOCK_COUNT_AT: usize = 24;
pub const MAP_OFFSET_AT: usize = 32;
pub const MAP_CRC_AT: usize = 40;
pub const END_OFFSET_AT: usize = 44;
pub const MANDATORY_AT: usize = 52;
pub const OPTIONAL_AT: usize = 60;
pub const RESERVED_AT: usize = 68;
pub const SLOT_CRC_AT: usize = SLOT_SIZE - 4;

// ------------------------------------------------------------------ errors

/// Structure a [`ParseError`] was found in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Section {
    /// Whole-source properties (size query).
    Container,
    Slot,
    Map,
    /// A block descriptor (a map entry) or a data block.
    Block,
}

/// Limit that was exceeded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Limit {
    Blocks,
    MapBytes,
    FileBytes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseKind {
    /// Fewer bytes than the structure needs: `needed` counts from the start of
    /// the structure for a slot read, and is the declared end for a root that
    /// points past the end of the source.
    Truncated {
        needed: u64,
        available: u64,
    },
    Magic {
        found: [u8; 8],
    },
    Version {
        found: u32,
    },
    /// A mandatory feature bit (lowest offending index) the caller cannot
    /// honor for the requested access.
    UnsupportedMandatoryFeature(u8),
    Crc {
        stored: u32,
        computed: u32,
    },
    /// Offset or length arithmetic does not fit in `u64`.
    Overflow,
    /// A value lies outside the range the layout allows.
    OutOfRange,
    /// Ranges intersect; `other_offset` is the start of the other range.
    Overlap {
        other_offset: u64,
    },
    /// Two descriptors name the same physical block.
    Duplicate {
        first_index: u64,
    },
    /// Zero, maximal or wrong-parity generation.
    Generation,
    /// Geometry that identifies the format (block size) differs.
    Identity,
    LimitExceeded(Limit),
    /// Reserved bytes are not zero.
    Reserved,
    /// The source reported an I/O failure.
    Io(StorageError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseError {
    pub section: Section,
    pub offset: u64,
    pub kind: ParseKind,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cow parse error in {:?} at offset {}: {:?}",
            self.section, self.offset, self.kind
        )
    }
}

impl std::error::Error for ParseError {}

fn err(section: Section, offset: u64, kind: ParseKind) -> ParseError {
    ParseError {
        section,
        offset,
        kind,
    }
}

// ------------------------------------------------------------ capabilities

/// Which mandatory feature bits the caller understands.
///
/// A set mandatory bit is a promise by the writer that readers who do not
/// understand it must not interpret the file. Optional bits never block any
/// open and are reported in [`Features`] only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Capabilities {
    /// Bits whose semantics are understood well enough to read.
    pub read_mandatory: u64,
    /// Bits that may additionally be written through; only bits that are also
    /// in `read_mandatory` count.
    pub write_mandatory: u64,
}

impl Capabilities {
    /// This build: the prototype defines no features yet.
    pub const CURRENT: Self = Self {
        read_mandatory: 0,
        write_mandatory: 0,
    };

    fn allowed(&self, access: Access) -> u64 {
        match access {
            Access::ReadOnly => self.read_mandatory,
            Access::Writable => self.read_mandatory & self.write_mandatory,
        }
    }
}

/// Intent of the open. Any mandatory bit the caller cannot write through
/// forbids [`Writable`](Self::Writable); read-only needs the weaker "understood
/// semantics" condition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Access {
    ReadOnly,
    Writable,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Features {
    pub mandatory: u64,
    pub optional: u64,
}

// ------------------------------------------------------------------ source

/// Read failure of a [`Source`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadError {
    OutOfBounds,
    Storage(StorageError),
}

/// Read-only view of the bytes being parsed.
pub trait Source {
    fn size(&self) -> Result<u64, StorageError>;
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), ReadError>;
}

impl Source for [u8] {
    fn size(&self) -> Result<u64, StorageError> {
        Ok(self.len() as u64)
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), ReadError> {
        let start = usize::try_from(offset).map_err(|_| ReadError::OutOfBounds)?;
        let end = start.checked_add(buf.len()).ok_or(ReadError::OutOfBounds)?;
        let src = self.get(start..end).ok_or(ReadError::OutOfBounds)?;
        buf.copy_from_slice(src);
        Ok(())
    }
}

/// [`Source`] over a [`CheckedStorage`]; uses only its read methods.
pub struct StorageSource<'a, B: StorageBackend>(pub &'a CheckedStorage<B>);

impl<B: StorageBackend> Source for StorageSource<'_, B> {
    fn size(&self) -> Result<u64, StorageError> {
        self.0.size()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), ReadError> {
        self.0
            .read_exact_at(offset, buf)
            .map_err(ReadError::Storage)
    }
}

fn container_size<S: Source + ?Sized>(src: &S) -> Result<u64, ParseError> {
    src.size()
        .map_err(|e| err(Section::Container, 0, ParseKind::Io(e)))
}

fn read_checked<S: Source + ?Sized>(
    src: &S,
    section: Section,
    offset: u64,
    buf: &mut [u8],
) -> Result<(), ParseError> {
    src.read_at(offset, buf).map_err(|e| match e {
        ReadError::OutOfBounds => err(
            section,
            offset,
            ParseKind::Truncated {
                needed: buf.len() as u64,
                available: 0,
            },
        ),
        ReadError::Storage(s) => err(section, offset, ParseKind::Io(s)),
    })
}

// ------------------------------------------------------------ field access

/// Fixed-size field reader over a buffer that starts at file offset `base`.
struct Fields<'a> {
    buf: &'a [u8],
    base: u64,
    section: Section,
}

impl Fields<'_> {
    fn array<const N: usize>(&self, at: usize) -> Result<[u8; N], ParseError> {
        let end = at.checked_add(N);
        end.and_then(|e| self.buf.get(at..e))
            .and_then(|s| <[u8; N]>::try_from(s).ok())
            .ok_or_else(|| {
                err(
                    self.section,
                    self.base.saturating_add(at as u64),
                    ParseKind::Truncated {
                        needed: (at + N) as u64,
                        available: self.buf.len() as u64,
                    },
                )
            })
    }

    fn u32(&self, at: usize) -> Result<u32, ParseError> {
        self.array::<4>(at).map(u32::from_le_bytes)
    }

    fn u64(&self, at: usize) -> Result<u64, ParseError> {
        self.array::<8>(at).map(u64::from_le_bytes)
    }

    fn at(&self, at: usize) -> u64 {
        self.base.saturating_add(at as u64)
    }
}

// ------------------------------------------------------------------- slots

/// A CRC-valid, structurally valid root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RootRecord {
    /// File offset the record was read from (error offsets derive from it).
    pub slot_offset: u64,
    pub generation: u64,
    pub block_count: u64,
    pub map_offset: u64,
    pub map_crc: u32,
    pub end_offset: u64,
    pub features: Features,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Slot {
    /// Every byte is zero.
    Blank,
    Root(RootRecord),
}

/// Parses the 512-byte record at `offset`, which must hold generations of
/// parity `slot` (slot 0 or 1; also used for the root copy at the file end).
///
/// Check order: truncation, blank, magic, CRC, version, identity, features,
/// reserved bytes, generation, limits. The CRC comes before the fields it
/// covers, so a flipped bit is reported as corruption, not as a "wrong"
/// version.
pub fn parse_slot_at<S: Source + ?Sized>(
    src: &S,
    offset: u64,
    slot: usize,
    caps: &Capabilities,
    access: Access,
) -> Result<Slot, ParseError> {
    if slot >= SLOT_COUNT {
        return Err(err(Section::Slot, offset, ParseKind::OutOfRange));
    }
    let size = container_size(src)?;
    let end = offset
        .checked_add(SLOT_SIZE as u64)
        .ok_or_else(|| err(Section::Slot, offset, ParseKind::Overflow))?;
    if end > size {
        return Err(err(
            Section::Slot,
            offset,
            ParseKind::Truncated {
                needed: SLOT_SIZE as u64,
                available: size.saturating_sub(offset),
            },
        ));
    }
    let mut buf = [0u8; SLOT_SIZE];
    read_checked(src, Section::Slot, offset, &mut buf)?;
    let f = Fields {
        buf: &buf,
        base: offset,
        section: Section::Slot,
    };
    if buf.iter().all(|&b| b == 0) {
        return Ok(Slot::Blank);
    }
    let magic = f.array::<8>(MAGIC_AT)?;
    if magic != MAGIC {
        return Err(err(
            Section::Slot,
            f.at(MAGIC_AT),
            ParseKind::Magic { found: magic },
        ));
    }
    let stored = f.u32(SLOT_CRC_AT)?;
    let computed = crc32(buf.get(..SLOT_CRC_AT).unwrap_or(&[]));
    if stored != computed {
        return Err(err(
            Section::Slot,
            f.at(SLOT_CRC_AT),
            ParseKind::Crc { stored, computed },
        ));
    }
    let version = f.u32(VERSION_AT)?;
    if version != VERSION {
        return Err(err(
            Section::Slot,
            f.at(VERSION_AT),
            ParseKind::Version { found: version },
        ));
    }
    if f.u32(BLOCK_SIZE_AT)? as usize != BLOCK_SIZE {
        return Err(err(Section::Slot, f.at(BLOCK_SIZE_AT), ParseKind::Identity));
    }
    let features = Features {
        mandatory: f.u64(MANDATORY_AT)?,
        optional: f.u64(OPTIONAL_AT)?,
    };
    let unknown = features.mandatory & !caps.allowed(access);
    if unknown != 0 {
        return Err(err(
            Section::Slot,
            f.at(MANDATORY_AT),
            ParseKind::UnsupportedMandatoryFeature(unknown.trailing_zeros() as u8),
        ));
    }
    let reserved = buf.get(RESERVED_AT..SLOT_CRC_AT).unwrap_or(&[]);
    if let Some(i) = reserved.iter().position(|&b| b != 0) {
        return Err(err(
            Section::Slot,
            f.at(RESERVED_AT + i),
            ParseKind::Reserved,
        ));
    }
    let generation = f.u64(GENERATION_AT)?;
    // `u64::MAX` is refused so that `generation + 1` stays representable.
    if generation == 0 || generation == u64::MAX || (generation & 1) as usize != slot {
        return Err(err(
            Section::Slot,
            f.at(GENERATION_AT),
            ParseKind::Generation,
        ));
    }
    let block_count = f.u64(BLOCK_COUNT_AT)?;
    if block_count > MAX_BLOCKS {
        return Err(err(
            Section::Slot,
            f.at(BLOCK_COUNT_AT),
            ParseKind::LimitExceeded(Limit::Blocks),
        ));
    }
    let end_offset = f.u64(END_OFFSET_AT)?;
    if end_offset > MAX_FILE_BYTES {
        return Err(err(
            Section::Slot,
            f.at(END_OFFSET_AT),
            ParseKind::LimitExceeded(Limit::FileBytes),
        ));
    }
    Ok(Slot::Root(RootRecord {
        slot_offset: offset,
        generation,
        block_count,
        map_offset: f.u64(MAP_OFFSET_AT)?,
        map_crc: f.u32(MAP_CRC_AT)?,
        end_offset,
        features,
    }))
}

// --------------------------------------------------------------------- map

/// One map entry: where a logical block lives and the CRC of its bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockDescriptor {
    pub phys: u64,
    pub crc: u32,
}

/// First collision among `(offset, len)` ranges, as `(later_index, kind)` where
/// `later_index` is the larger index of the two colliding ranges. Equal offsets
/// are [`ParseKind::Duplicate`]; otherwise an intersection is
/// [`ParseKind::Overlap`]. Zero-length ranges collide with nothing. Ranges must
/// not overflow `u64`.
pub(crate) fn first_collision(ranges: &[(u64, u64)]) -> Option<(usize, ParseKind)> {
    let mut order: Vec<usize> = (0..ranges.len())
        .filter(|&i| ranges.get(i).is_some_and(|r| r.1 > 0))
        .collect();
    order.sort_by_key(|&i| (ranges.get(i).map(|r| r.0), i));
    let mut best: Option<(usize, ParseKind)> = None;
    for pair in order.windows(2) {
        let &[a, b] = pair else {
            continue;
        };
        let (Some(&(a_off, a_len)), Some(&(b_off, _))) = (ranges.get(a), ranges.get(b)) else {
            continue;
        };
        let kind = if a_off == b_off {
            ParseKind::Duplicate {
                first_index: a.min(b) as u64,
            }
        } else if a_off.saturating_add(a_len) > b_off {
            ParseKind::Overlap {
                other_offset: if a > b { b_off } else { a_off },
            }
        } else {
            continue;
        };
        let later = a.max(b);
        if best.is_none_or(|(l, _)| later < l) {
            best = Some((later, kind));
        }
    }
    best
}

/// Parses and validates the map a root points to.
///
/// Order: root-declared fields (`end_offset`, `block_count`, map range), map
/// CRC, then entries in index order (overflow, range, overlap with the map),
/// then collisions among blocks. Root-field problems are reported at the root
/// field's offset; entry problems at the entry's offset in the map.
pub fn parse_map<S: Source + ?Sized>(
    src: &S,
    root: &RootRecord,
) -> Result<Vec<BlockDescriptor>, ParseError> {
    let field = |at: usize| root.slot_offset.saturating_add(at as u64);
    let size = container_size(src)?;

    let end = root.end_offset;
    if end > MAX_FILE_BYTES {
        return Err(err(
            Section::Slot,
            field(END_OFFSET_AT),
            ParseKind::LimitExceeded(Limit::FileBytes),
        ));
    }
    if end < HEADER_LEN || !end.is_multiple_of(BLOCK_SIZE as u64) {
        return Err(err(
            Section::Slot,
            field(END_OFFSET_AT),
            ParseKind::OutOfRange,
        ));
    }
    if end > size {
        return Err(err(
            Section::Slot,
            field(END_OFFSET_AT),
            ParseKind::Truncated {
                needed: end,
                available: size,
            },
        ));
    }
    // Limits first: nothing below allocates or loops over `block_count`
    // before these checks.
    if root.block_count > MAX_BLOCKS {
        return Err(err(
            Section::Slot,
            field(BLOCK_COUNT_AT),
            ParseKind::LimitExceeded(Limit::Blocks),
        ));
    }
    let map_len = root
        .block_count
        .checked_mul(ENTRY_LEN as u64)
        .ok_or_else(|| err(Section::Slot, field(BLOCK_COUNT_AT), ParseKind::Overflow))?;
    if map_len > MAX_MAP_BYTES {
        return Err(err(
            Section::Slot,
            field(BLOCK_COUNT_AT),
            ParseKind::LimitExceeded(Limit::MapBytes),
        ));
    }
    let map_offset = root.map_offset;
    let map_end = map_offset
        .checked_add(map_len)
        .ok_or_else(|| err(Section::Slot, field(MAP_OFFSET_AT), ParseKind::Overflow))?;
    // The last sector of a published area above the header is the root copy;
    // metadata and blocks end before it.
    let payload_end = if end > HEADER_LEN {
        end - SLOT_SIZE as u64
    } else {
        end
    };
    if map_offset < HEADER_LEN
        || !map_offset.is_multiple_of(BLOCK_SIZE as u64)
        || map_end > payload_end
    {
        return Err(err(
            Section::Slot,
            field(MAP_OFFSET_AT),
            ParseKind::OutOfRange,
        ));
    }

    let map_len_usize = usize::try_from(map_len)
        .map_err(|_| err(Section::Slot, field(BLOCK_COUNT_AT), ParseKind::Overflow))?;
    let mut bytes = vec![0u8; map_len_usize];
    read_checked(src, Section::Map, map_offset, &mut bytes)?;
    let computed = crc32(&bytes);
    if computed != root.map_crc {
        return Err(err(
            Section::Map,
            map_offset,
            ParseKind::Crc {
                stored: root.map_crc,
                computed,
            },
        ));
    }

    let mut blocks = Vec::with_capacity(map_len_usize / ENTRY_LEN);
    for (i, chunk) in bytes.as_chunks::<ENTRY_LEN>().0.iter().enumerate() {
        let entry_offset = map_offset.saturating_add((i * ENTRY_LEN) as u64);
        let f = Fields {
            buf: chunk.as_slice(),
            base: entry_offset,
            section: Section::Block,
        };
        let phys = f.u64(0)?;
        let crc = f.u32(8)?;
        let phys_end = phys
            .checked_add(BLOCK_SIZE as u64)
            .ok_or_else(|| err(Section::Block, entry_offset, ParseKind::Overflow))?;
        if phys < HEADER_LEN || !phys.is_multiple_of(BLOCK_SIZE as u64) || phys_end > payload_end {
            return Err(err(Section::Block, entry_offset, ParseKind::OutOfRange));
        }
        if phys < map_end && map_offset < phys_end {
            return Err(err(
                Section::Block,
                entry_offset,
                ParseKind::Overlap {
                    other_offset: map_offset,
                },
            ));
        }
        blocks.push(BlockDescriptor { phys, crc });
    }

    let ranges: Vec<(u64, u64)> = blocks.iter().map(|b| (b.phys, BLOCK_SIZE as u64)).collect();
    if let Some((later, kind)) = first_collision(&ranges) {
        let entry_offset = map_offset.saturating_add((later * ENTRY_LEN) as u64);
        return Err(err(Section::Block, entry_offset, kind));
    }
    Ok(blocks)
}

// --------------------------------------------------------------- committed

/// A root together with its validated map.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Committed {
    pub root: RootRecord,
    pub blocks: Vec<BlockDescriptor>,
}

/// Parses slot `slot` (at `slot * 512`) and its map. A blank slot is not a
/// root and is reported as [`ParseKind::Magic`].
pub fn parse_committed<S: Source + ?Sized>(
    src: &S,
    slot: usize,
    caps: &Capabilities,
    access: Access,
) -> Result<Committed, ParseError> {
    let offset = (slot as u64).saturating_mul(SLOT_SIZE as u64);
    match parse_slot_at(src, offset, slot, caps, access)? {
        Slot::Blank => Err(err(
            Section::Slot,
            offset,
            ParseKind::Magic { found: [0; 8] },
        )),
        Slot::Root(root) => {
            let blocks = parse_map(src, &root)?;
            Ok(Committed { root, blocks })
        }
    }
}

/// Reads one data block and checks it against its descriptor's CRC. The error
/// offset is the block's physical offset.
pub fn verify_block_data<S: Source + ?Sized>(
    src: &S,
    block: &BlockDescriptor,
) -> Result<(), ParseError> {
    let size = container_size(src)?;
    let end = block
        .phys
        .checked_add(BLOCK_SIZE as u64)
        .ok_or_else(|| err(Section::Block, block.phys, ParseKind::Overflow))?;
    if end > size {
        return Err(err(
            Section::Block,
            block.phys,
            ParseKind::Truncated {
                needed: BLOCK_SIZE as u64,
                available: size.saturating_sub(block.phys),
            },
        ));
    }
    let mut data = [0u8; BLOCK_SIZE];
    read_checked(src, Section::Block, block.phys, &mut data)?;
    let computed = crc32(&data);
    if computed != block.crc {
        return Err(err(
            Section::Block,
            block.phys,
            ParseKind::Crc {
                stored: block.crc,
                computed,
            },
        ));
    }
    Ok(())
}
