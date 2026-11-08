//! RAW import and export of prototype images with a verified result (S9-20).
//!
//! **Prototype, not a normative format.** Two operations, both producing a
//! *new* object and neither modifying an existing one:
//!
//! * [`export_raw`]: writes the logical bytes of one immutable state of an
//!   image (the head or a named snapshot, flattened through the verified
//!   parent chain) into a plain RAW target of exactly the logical size.
//! * [`import_raw`]: builds a new image of its own format from a RAW source.
//!
//! # Export: what is fixed, what is proven
//!
//! The source is opened through [`ImageReader`], which uses the same strict
//! parser and layer decoder as `open` but **never writes** (read-only
//! recovery; a source that may have lost an acknowledged commit is refused).
//! The reader fixes one root generation for the whole call and re-checks both
//! root slots at the end ([`ExportError::SourceChanged`]); the host holds a
//! read lease around the call, so a cooperating writer cannot change the
//! file in between. The mutable active disk is never what is read: the caller
//! names a root (`Head` of a closed file, or a snapshot).
//!
//! The target must be a new, empty backend. Data is written in chunks, then
//! made durable (a verified flush; [`ExportOptions::require_file_durability`]
//! refuses a backend that cannot give one *before anything is written*), and
//! only then is the result checked: every chunk of the target is read back
//! through the backend and compared **byte for byte** with a fresh read of the
//! same range from the source (two independent buffers, not a hash of what was
//! written). [`export_raw`] returns `Ok` only after that comparison and the
//! source generation re-check; on any error or cancellation the caller must
//! treat the target as incomplete.
//!
//! This module has no namespace operations (the contract has no paths). How a
//! host keeps an incomplete target from looking complete is the host's job;
//! see `od-image-tool`: it writes to a separate `.partial` name and
//! publishes with an atomic no-replace link only after `Ok`.
//!
//! Limits: the read-back goes through the same handle, so it proves what the
//! backend returns, not what a power cut leaves on media; the durable flush
//! is what orders the data before publication.
//!
//! # Import
//!
//! A RAW source must be a whole number of blocks (the layer cannot record a
//! byte-exact size; a partial last block is refused, never padded) and at
//! most [`MAX_DISK_BLOCKS`](crate::cow_snapshot::MAX_DISK_BLOCKS) blocks. The
//! image is built as a *building* file that no open accepts, reopened from the
//! media, compared with a fresh read of the RAW source and only then
//! published by one commit (the same mechanism as compaction). A crash or
//! error earlier leaves a target that does not open. All blocks are stored
//! (zero blocks included): the dense map has no cheap way to skip a long run.

use crate::cow_parse::{Access, Capabilities, Slot, StorageSource, parse_slot_at};
use crate::cow_proto::{BLOCK_SIZE, CowError, CowView, OpenReport};
use crate::cow_snapshot::{
    Ancestor, DATA_BASE, Header, Level, MAX_DISK_BLOCKS, ParentResolver, SnapError, SnapFile,
    Verified, bit, block_of, chain_read, read_desc, read_range,
};
use crate::io::{CheckedStorage, StorageBackend, StorageError};
use std::collections::BTreeMap;
use std::io::ErrorKind;

const BS: u64 = BLOCK_SIZE as u64;
/// Bytes moved per step by export and import: 64 blocks.
const CHUNK_BLOCKS: u64 = 64;
/// Blocks per import commit (a commit holds at most `MAX_TX_BLOCKS`).
const IMPORT_COMMIT_BLOCKS: u64 = 128;

/// Which immutable state of an image an operation reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateSelection {
    /// The head of a **closed** file (its newest published root).
    Head,
    /// A named, retained snapshot.
    Snapshot(u64),
}

// ------------------------------------------------------------------ reader

/// Read-only, fixed-generation view of an image and its chain.
pub struct ImageReader<B: StorageBackend> {
    view: CowView<B>,
    header: Header,
    presence: [u8; BLOCK_SIZE],
    snapshots: BTreeMap<u64, Level>,
    chain: Vec<Ancestor>,
    report: OpenReport,
}

impl<B: StorageBackend> ImageReader<B> {
    /// Read-only open: CoW recovery without writing, then the header, every
    /// retained snapshot and the whole parent chain, exactly as `open` verifies
    /// them. A source whose newest root may be lost
    /// ([`OpenReport::possible_lost_commit`]) is refused: exporting an older
    /// state as if it were current would be a silent rollback.
    pub fn open<R: ParentResolver>(
        storage: CheckedStorage<B>,
        resolver: &mut R,
    ) -> Result<Self, SnapError> {
        let (view, report) = CowView::salvage(storage)?;
        if report.possible_lost_commit {
            return Err(SnapError::Cow(CowError::DamagedOtherSlot {
                chosen_generation: report.generation,
                other: report.other_slot,
            }));
        }
        let Verified {
            header,
            presence,
            snapshots,
            chain,
        } = SnapFile::<B>::verify_open(&view, resolver)?;
        Ok(Self {
            view,
            header,
            presence,
            snapshots,
            chain,
            report,
        })
    }

    pub fn file_uid(&self) -> u64 {
        self.header.file_uid
    }

    /// Generation of the root this reader fixed.
    pub fn generation(&self) -> u64 {
        self.view.generation()
    }

    pub fn report(&self) -> &OpenReport {
        &self.report
    }

    pub fn depth(&self) -> u32 {
        self.header.depth
    }

    pub fn snapshot_ids(&self) -> Vec<u64> {
        self.header.snapshots.iter().map(|r| r.id).collect()
    }

    /// Disk size in blocks of the selected state.
    pub fn disk_blocks(&self, sel: StateSelection) -> Result<u64, SnapError> {
        match sel {
            StateSelection::Head => Ok(self.header.disk_blocks),
            StateSelection::Snapshot(id) => self
                .snapshots
                .get(&id)
                .map(|l| l.header.disk_blocks)
                .ok_or(SnapError::SnapshotMissing { id }),
        }
    }

    /// Generation of the selected state.
    pub fn state_generation(&self, sel: StateSelection) -> Result<u64, SnapError> {
        match sel {
            StateSelection::Head => Ok(self.view.generation()),
            StateSelection::Snapshot(id) => self
                .header
                .snapshots
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.generation)
                .ok_or(SnapError::SnapshotMissing { id }),
        }
    }

    /// Reads bytes of the selected state; every block is CRC-checked and holes
    /// read as parent data or zeros.
    pub fn read(&self, sel: StateSelection, offset: u64, buf: &mut [u8]) -> Result<(), SnapError> {
        let blocks = self.disk_blocks(sel)?;
        read_range(blocks, offset, buf, |b| match sel {
            StateSelection::Head => {
                if bit(&self.presence, b) {
                    let index = b.checked_add(DATA_BASE).ok_or(SnapError::OutOfRange)?;
                    block_of(self.view.read_block(index)?)
                } else {
                    chain_read(&self.chain, b)
                }
            }
            StateSelection::Snapshot(id) => {
                let level = self
                    .snapshots
                    .get(&id)
                    .ok_or(SnapError::SnapshotMissing { id })?;
                match level.data_desc(b) {
                    Some(d) => read_desc(self.view.storage(), d),
                    None => chain_read(&self.chain, b),
                }
            }
        })
    }

    /// Re-reads both root slots and reports whether the root this reader fixed
    /// is still the newest one with the same map and end. `false` means the
    /// file was changed while it was read (the lease was not honoured by
    /// someone).
    pub fn root_unchanged(&self) -> Result<bool, SnapError> {
        let storage = self.view.storage();
        let size = storage.size().map_err(CowError::Storage)?;
        if size < self.view.end_offset() {
            return Ok(false);
        }
        let src = StorageSource(storage);
        let generation = self.view.generation();
        let mut found_same = false;
        for slot in 0..2usize {
            let offset = (slot as u64) * BS;
            match parse_slot_at(&src, offset, slot, &Capabilities::CURRENT, Access::ReadOnly) {
                Ok(Slot::Root(r)) => {
                    if r.generation > generation {
                        return Ok(false);
                    }
                    if r.generation == generation {
                        if r.map_crc != self.view.map_crc()
                            || r.end_offset != self.view.end_offset()
                        {
                            return Ok(false);
                        }
                        found_same = true;
                    }
                }
                Ok(Slot::Blank) => {}
                Err(e) => {
                    if let crate::cow_parse::ParseKind::Io(s) = e.kind {
                        return Err(SnapError::Cow(CowError::Storage(s)));
                    }
                }
            }
        }
        // A rolled-forward root has no slot of its own yet.
        Ok(found_same || self.report.rolled_forward)
    }
}

// ------------------------------------------------------------------ export

/// Options of [`export_raw`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportOptions {
    /// Refuse a target whose backend cannot give a verified durable flush,
    /// before writing anything. Publication of an export whose bytes were not
    /// made durable first is not allowed, so a host sets this.
    pub require_file_durability: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            require_file_durability: true,
        }
    }
}

/// What a successful export proved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportReport {
    pub selection: StateSelection,
    /// Exact size of the RAW target in bytes.
    pub logical_size: u64,
    pub blocks: u64,
    /// Blocks that are entirely zero (holes and stored zeros alike).
    pub zero_blocks: u64,
    pub source_uid: u64,
    /// Generation of the root the whole export was read from.
    pub source_generation: u64,
    /// Generation of the exported state (the snapshot's own, or the head's).
    pub state_generation: u64,
    /// The target's file contents were made durable before the read-back.
    pub file_durable: bool,
    /// The source was opened by rolling forward a root copy in memory.
    pub rolled_forward: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExportError {
    /// Source did not open, a block failed, or the selection is missing.
    Source(SnapError),
    /// Both root slots no longer describe the generation read at the start.
    SourceChanged,
    /// The target is not empty (or a previous object): nothing was written.
    TargetNotEmpty {
        len: u64,
    },
    /// The target could not be created (an existing name is
    /// [`ErrorKind::AlreadyExists`]); nothing existing was touched.
    TargetCreate(ErrorKind),
    /// The target backend is read-only or cannot resize.
    TargetUnsuitable,
    /// Durability was required and the target cannot provide it.
    DurabilityUnavailable,
    Target(StorageError),
    /// Cancelled before the result was published; the target is incomplete.
    Cancelled,
    /// Read-back of the target differs from the source at `offset`.
    VerifyMismatch {
        offset: u64,
    },
    /// The target does not have exactly the logical size afterwards.
    SizeMismatch {
        expected: u64,
        actual: u64,
    },
}

impl From<SnapError> for ExportError {
    fn from(e: SnapError) -> Self {
        Self::Source(e)
    }
}

impl From<StorageError> for ExportError {
    fn from(e: StorageError) -> Self {
        Self::Target(e)
    }
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "raw export error: {self:?}")
    }
}

impl std::error::Error for ExportError {}

impl ExportError {
    /// Integrity class and stable reason (numbers and fixed codes only).
    pub fn classify(&self) -> (crate::cow_inspect::Outcome, &'static str, Option<u64>) {
        use crate::cow_inspect::{Outcome, classify_snap};
        match self {
            Self::Source(e) => classify_snap(e),
            Self::SourceChanged => (Outcome::Busy, "source_changed", None),
            Self::TargetNotEmpty { len } => (Outcome::Io, "target_not_empty", Some(*len)),
            Self::TargetCreate(ErrorKind::AlreadyExists) => (Outcome::Io, "target_exists", None),
            Self::TargetCreate(_) => (Outcome::Io, "target_create_failed", None),
            Self::TargetUnsuitable => (Outcome::Io, "target_unsuitable", None),
            Self::DurabilityUnavailable => (Outcome::Io, "durability_unavailable", None),
            Self::Target(StorageError::Io {
                kind: ErrorKind::StorageFull,
                ..
            }) => (Outcome::Io, "no_space", None),
            Self::Target(_) => (Outcome::Io, "target_io_error", None),
            Self::Cancelled => (Outcome::Io, "cancelled", None),
            Self::VerifyMismatch { offset } => (Outcome::Io, "verify_mismatch", Some(*offset)),
            Self::SizeMismatch { actual, .. } => (Outcome::Io, "size_mismatch", Some(*actual)),
        }
    }
}

fn chunk_bytes(total: u64, offset: u64) -> Result<usize, SnapError> {
    usize::try_from((CHUNK_BLOCKS * BS).min(total.saturating_sub(offset)))
        .map_err(|_| SnapError::OutOfRange)
}

/// Exports the selected state of `source` into a new, empty RAW target.
///
/// `make_target` is called **only after** the source has opened and verified
/// and the selection exists, so an unusable source or a missing snapshot never
/// creates anything. It must return a new, empty storage (a host refuses an
/// existing path with [`ErrorKind::AlreadyExists`]).
///
/// `cancel` is polled before every chunk and before the durable flush; `true`
/// stops with [`ExportError::Cancelled`] and leaves the target incomplete.
/// `Ok` means: the bytes were written, made durable (when the backend can),
/// read back and equal to a second read of the source, and the source root was
/// unchanged. See the module docs for what that does and does not prove.
pub fn export_raw<S, R, T>(
    source: CheckedStorage<S>,
    resolver: &mut R,
    selection: StateSelection,
    make_target: impl FnOnce() -> Result<CheckedStorage<T>, std::io::Error>,
    options: &ExportOptions,
    cancel: &dyn Fn() -> bool,
) -> Result<ExportReport, ExportError>
where
    S: StorageBackend,
    R: ParentResolver,
    T: StorageBackend,
{
    let reader = ImageReader::open(source, resolver)?;
    let blocks = reader.disk_blocks(selection)?;
    let state_generation = reader.state_generation(selection)?;
    let size = blocks.checked_mul(BS).ok_or(SnapError::OutOfRange)?;

    let mut target = make_target().map_err(|e| ExportError::TargetCreate(e.kind()))?;
    let caps = target.capabilities();
    if !caps.writable || !caps.resize {
        return Err(ExportError::TargetUnsuitable);
    }
    if options.require_file_durability && !caps.file_durability {
        return Err(ExportError::DurabilityUnavailable);
    }
    let existing = target.size()?;
    if existing != 0 {
        return Err(ExportError::TargetNotEmpty { len: existing });
    }

    if size > 0 {
        target.set_len(size)?;
    }
    let mut zero_blocks = 0u64;
    let mut buf = vec![0u8; usize::try_from(CHUNK_BLOCKS * BS).map_err(|_| SnapError::OutOfRange)?];
    let mut offset = 0u64;
    while offset < size {
        if cancel() {
            return Err(ExportError::Cancelled);
        }
        let n = chunk_bytes(size, offset)?;
        let chunk = buf.get_mut(..n).ok_or(SnapError::OutOfRange)?;
        reader.read(selection, offset, chunk)?;
        zero_blocks += chunk
            .chunks(BLOCK_SIZE)
            .filter(|b| b.iter().all(|x| *x == 0))
            .count() as u64;
        target.write_all_at(offset, chunk)?;
        offset += n as u64;
    }
    if cancel() {
        return Err(ExportError::Cancelled);
    }
    let file_durable = if caps.file_durability {
        target.flush_file_durable()?;
        true
    } else {
        false
    };

    // Independent comparison: the target as the backend returns it against a
    // fresh read of the source, in separate buffers.
    let actual = target.size()?;
    if actual != size {
        return Err(ExportError::SizeMismatch {
            expected: size,
            actual,
        });
    }
    let mut from_target = vec![0u8; buf.len()];
    let mut offset = 0u64;
    while offset < size {
        if cancel() {
            return Err(ExportError::Cancelled);
        }
        let n = chunk_bytes(size, offset)?;
        let a = buf.get_mut(..n).ok_or(SnapError::OutOfRange)?;
        let b = from_target.get_mut(..n).ok_or(SnapError::OutOfRange)?;
        reader.read(selection, offset, a)?;
        target.read_exact_at(offset, b)?;
        if let Some(i) = a.iter().zip(b.iter()).position(|(x, y)| x != y) {
            return Err(ExportError::VerifyMismatch {
                offset: offset + i as u64,
            });
        }
        offset += n as u64;
    }
    if !reader.root_unchanged()? {
        return Err(ExportError::SourceChanged);
    }
    Ok(ExportReport {
        selection,
        logical_size: size,
        blocks,
        zero_blocks,
        source_uid: reader.file_uid(),
        source_generation: reader.generation(),
        state_generation,
        file_durable,
        rolled_forward: reader.report().rolled_forward,
    })
}

// ------------------------------------------------------------------ import

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportReport {
    pub logical_size: u64,
    pub blocks: u64,
    pub file_uid: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImportError {
    Raw(StorageError),
    /// The RAW source is not a whole number of blocks; it is never padded.
    NotBlockAligned {
        len: u64,
    },
    /// More blocks than one layer can address.
    TooLarge {
        blocks: u64,
        limit: u64,
    },
    /// The target is not empty: nothing was written.
    TargetNotEmpty {
        len: u64,
    },
    Image(SnapError),
    Cancelled,
    /// The built image read back differently from the RAW source.
    VerifyMismatch {
        offset: u64,
    },
}

impl From<SnapError> for ImportError {
    fn from(e: SnapError) -> Self {
        Self::Image(e)
    }
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "raw import error: {self:?}")
    }
}

impl std::error::Error for ImportError {}

/// Builds a new base image in the empty `target` from the RAW `raw`.
///
/// The target is a *building* file until the very last commit, so an error,
/// a cancellation or a crash leaves something that does not open, never a
/// partial image. Before that commit the image is reopened from the media and
/// compared, chunk by chunk, with a **fresh read** of `raw`. `raw` is only
/// read. `new_uid` is the identity of the new file (non-zero).
pub fn import_raw<R, T>(
    raw: &CheckedStorage<R>,
    target: CheckedStorage<T>,
    new_uid: u64,
    cancel: &dyn Fn() -> bool,
) -> Result<(SnapFile<T>, ImportReport), ImportError>
where
    R: StorageBackend,
    T: StorageBackend,
{
    let len = raw.size().map_err(ImportError::Raw)?;
    if !len.is_multiple_of(BS) {
        return Err(ImportError::NotBlockAligned { len });
    }
    let blocks = len / BS;
    if blocks > MAX_DISK_BLOCKS {
        return Err(ImportError::TooLarge {
            blocks,
            limit: MAX_DISK_BLOCKS,
        });
    }
    let existing = target
        .size()
        .map_err(|e| ImportError::Image(SnapError::Cow(e.into())))?;
    if existing != 0 {
        return Err(ImportError::TargetNotEmpty { len: existing });
    }
    let mut image = SnapFile::create_building(target, new_uid)?;
    let step = IMPORT_COMMIT_BLOCKS * BS;
    let mut buf = vec![0u8; usize::try_from(step).map_err(|_| SnapError::OutOfRange)?];
    let mut offset = 0u64;
    while offset < len {
        if cancel() {
            return Err(ImportError::Cancelled);
        }
        let n = usize::try_from(step.min(len - offset)).map_err(|_| SnapError::OutOfRange)?;
        let chunk = buf.get_mut(..n).ok_or(SnapError::OutOfRange)?;
        raw.read_exact_at(offset, chunk).map_err(ImportError::Raw)?;
        image.write_at(offset, chunk)?;
        offset += n as u64;
    }
    if cancel() {
        return Err(ImportError::Cancelled);
    }
    let (image, _ack) = image.finish_building(|built| {
        if built.disk_size() != len {
            return Err(ImportError::VerifyMismatch {
                offset: built.disk_size().min(len),
            });
        }
        let mut from_image = vec![0u8; buf.len()];
        let mut from_raw = vec![0u8; buf.len()];
        let mut offset = 0u64;
        while offset < len {
            let n = usize::try_from(step.min(len - offset)).map_err(|_| SnapError::OutOfRange)?;
            let a = from_image.get_mut(..n).ok_or(SnapError::OutOfRange)?;
            let b = from_raw.get_mut(..n).ok_or(SnapError::OutOfRange)?;
            built.read_at(offset, a)?;
            raw.read_exact_at(offset, b).map_err(ImportError::Raw)?;
            if let Some(i) = a.iter().zip(b.iter()).position(|(x, y)| x != y) {
                return Err(ImportError::VerifyMismatch {
                    offset: offset + i as u64,
                });
            }
            offset += n as u64;
        }
        Ok(())
    })?;
    Ok((
        image,
        ImportReport {
            logical_size: len,
            blocks,
            file_uid: new_uid,
        },
    ))
}
