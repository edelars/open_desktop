//! Salvage of a damaged prototype image into a **new** copy, with an honest
//! report (S9-21).
//!
//! **Prototype, not a normative format.** Salvage is a separate, explicit
//! operation. It never writes the source (it holds only shared read access to
//! it), never repairs it, and never presents guesses as data. What it can
//! *prove* is narrow and stated here; everything else is reported, not
//! reconstructed.
//!
//! # What counts as evidence
//!
//! Bytes are recoverable only through **metadata that verifies**: a root slot
//! (or the root copy that normal recovery would roll forward to), the block
//! map that root points to (its CRC), the layer header and presence bitmap
//! stored in that map, and then each data block against the CRC the map
//! records. A block that merely exists somewhere in the append-only area (an
//! older version of an overwritten block, an unreferenced tail, a block of a
//! deleted snapshot) has **no provenance** and is never copied or counted as
//! valid, even when its own CRC is fine: nothing says it is the data the
//! chosen root meant.
//!
//! Root selection reuses the recovery rules of [`crate::cow_proto`]
//! (`select_root`); there is no second decoder. Fallback to an older root
//! because the newest root's map is unreadable is **not** done: that would
//! show stale data as current.
//!
//! # Classes of range
//!
//! Every byte of the selected state falls in exactly one extent:
//!
//! * `valid`: referenced by the chosen root and its CRC verified, or a hole
//!   that reads zeros by the format (a base file), or a hole resolved through
//!   a verified parent chain. The basis is stated per extent.
//! * `lost`: the chosen metadata refers to the block but its bytes fail the
//!   CRC (or lie beyond the file). The data existed and is damaged.
//! * `unknown`: the metadata needed to judge the range is not available (the
//!   parent of a child is missing, so its holes cannot be resolved), or a read
//!   failed with an I/O error that may be transient.
//! * `conflicting`: two CRC-valid roots that cannot be ordered (their
//!   generations are not consecutive, so neither provably supersedes the
//!   other) disagree about the bytes. Where both agree the range is `valid`.
//!
//! # Recoverability
//!
//! * [`Recoverability::Complete`]: every range of the selected state is valid
//!   and the newest root is proven (no possible lost commit).
//! * [`Recoverability::Partial`]: some range is not valid, or the opened root
//!   is an older one because the newer root may be lost (then the bytes are
//!   valid *as of that older generation*, stated in the report).
//! * [`Recoverability::Unrecoverable`]: the metadata needed to know what the
//!   disk is cannot be recovered (no valid root, unreadable or inconsistent
//!   map, lost layer header or presence bitmap, unsupported format). The
//!   report lists what was examined ([`Finding`]s) and **no copy is made**:
//!   this is an open criterion, never a success with zero data.
//!
//! Reconstruction beyond this (guessing a geometry from raw blocks, merging
//! generations, picking a winner between conflicting roots) is out of the
//! first version and refused explicitly.
//!
//! # The copy
//!
//! The copy is a self-contained **base** image of the selected state (a child
//! is flattened through its verified chain), built like an import: as a
//! *building* file that no open accepts, reopened from the media, compared with
//! the planned bytes and then published by one commit. Ranges that are not
//! valid are zero in the copy because a dense image needs some bytes there;
//! the report is the only authority on which ranges those are. For that
//! reason a partial recovery is **not** copied unless the caller opts in
//! ([`SalvageOptions::publish_partial`]); without it the target is never even
//! created. Snapshots of the source are not carried into the copy; the report
//! lists their individual state ([`SnapshotStatus`]) for the normal (single
//! root) case.
//!
//! The report contains numbers and fixed codes only (no payload, no paths).

use crate::cow_export::StateSelection;
use crate::cow_inspect::{Outcome, classify_snap};
use crate::cow_parse::{
    Access, BlockDescriptor, Capabilities, ParseError, ParseKind, RootRecord, Section, Slot,
    StorageSource, parse_map, parse_slot_at,
};
use crate::cow_proto::{BLOCK_SIZE, CowError, HEADER_LEN, crc32, select_root};
use crate::cow_snapshot::{
    Ancestor, DATA_BASE, Header, MAX_DISK_BLOCKS, ParentResolver, SnapError, SnapFile, bit,
    bitmap_within, chain_read, decode_header, load_level, resolve_chain,
};
use crate::io::{CheckedStorage, StorageBackend, StorageError};
use std::fmt::Write as _;
use std::io::ErrorKind;

const BS: u64 = BLOCK_SIZE as u64;
type Block = [u8; BLOCK_SIZE];
/// Blocks per commit while writing the copy.
const COPY_COMMIT_BLOCKS: u64 = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RangeClass {
    Valid,
    Unknown,
    Lost,
    Conflicting,
}

impl RangeClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Unknown => "unknown",
            Self::Lost => "lost",
            Self::Conflicting => "conflicting",
        }
    }
}

/// A byte range of the selected state with its class and the reason for it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Extent {
    pub offset: u64,
    pub len: u64,
    pub class: RangeClass,
    /// Stable code naming the evidence (`crc_verified`, `block_crc_mismatch`,
    /// `parent_unavailable`, `roots_disagree`, ...).
    pub basis: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Recoverability {
    Complete,
    Partial,
    Unrecoverable,
}

impl Recoverability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Unrecoverable => "unrecoverable",
        }
    }
}

/// One thing the salvage examined: a slot, the map, the layer header, a
/// parent. `status` is a fixed code; `detail` locates it (generation, file
/// offset).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Finding {
    pub subject: &'static str,
    pub status: &'static str,
    pub detail: Option<u64>,
}

/// State of one named snapshot of the source (normal single-root case only).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapshotStatus {
    pub id: u64,
    /// `intact`, `damaged` (some stored block fails its CRC), `unverified`
    /// (an I/O error), `metadata_lost` or `record_invalid`.
    pub status: &'static str,
    /// Stored blocks of this snapshot that fail their CRC.
    pub lost_blocks: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CopyStatus {
    /// The copy exists and is published (it opens).
    Published,
    /// No target was created, for this reason.
    NotCreated(&'static str),
    /// A target was requested but could not be completed: if it exists it is
    /// unpublished (it does not open).
    Failed(&'static str),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SalvageReport {
    pub selection: StateSelection,
    pub recoverability: Recoverability,
    /// `ok`, or the first reason the recovery is not complete.
    pub reason: &'static str,
    pub source_len: u64,
    /// Generation of every root the state was read from (two for conflicting
    /// roots).
    pub root_generations: Vec<u64>,
    /// `slot0`, `slot1`, `root_copy` or `ambiguous`.
    pub root_origin: &'static str,
    /// A newer root may have been lost; the state is that of an older root.
    pub possible_lost_commit: bool,
    pub tail_len: Option<u64>,
    pub logical_size: Option<u64>,
    pub extents: Vec<Extent>,
    pub findings: Vec<Finding>,
    pub snapshots: Vec<SnapshotStatus>,
    pub copy: CopyStatus,
}

impl SalvageReport {
    fn new(selection: StateSelection, source_len: u64) -> Self {
        Self {
            selection,
            recoverability: Recoverability::Unrecoverable,
            reason: "ok",
            source_len,
            root_generations: Vec::new(),
            root_origin: "none",
            possible_lost_commit: false,
            tail_len: None,
            logical_size: None,
            extents: Vec::new(),
            findings: Vec::new(),
            snapshots: Vec::new(),
            copy: CopyStatus::NotCreated("not_attempted"),
        }
    }

    fn find(&mut self, subject: &'static str, status: &'static str, detail: Option<u64>) {
        self.findings.push(Finding {
            subject,
            status,
            detail,
        });
    }

    /// Total bytes of one class.
    pub fn bytes(&self, class: RangeClass) -> u64 {
        self.extents
            .iter()
            .filter(|e| e.class == class)
            .map(|e| e.len)
            .sum()
    }

    /// Integrity class for a process exit status: complete is `Ok`; anything
    /// else is corrupt, except an unsupported format.
    pub fn outcome(&self) -> Outcome {
        match (self.recoverability, self.reason) {
            (Recoverability::Complete, _) => Outcome::Ok,
            (
                _,
                "unsupported_root"
                | "unsupported_mandatory_feature"
                | "not_a_layer_file"
                | "unsupported_layer_header",
            ) => Outcome::Unsupported,
            _ => Outcome::Corrupt,
        }
    }

    /// `key=value` lines of numbers and fixed codes. At most `max_extents`
    /// extents are listed; the totals are always complete and
    /// `extents_listed` says whether the list is.
    pub fn render(&self, max_extents: usize) -> String {
        let mut out = String::new();
        let opt = |v: Option<u64>| v.map_or_else(|| "none".to_string(), |x| x.to_string());
        let mut line = |k: &str, v: String| {
            let _ = writeln!(out, "{k}={v}");
        };
        line(
            "selection",
            match self.selection {
                StateSelection::Head => "head".to_string(),
                StateSelection::Snapshot(id) => format!("snapshot:{id}"),
            },
        );
        line("recoverability", self.recoverability.as_str().to_string());
        line("reason", self.reason.to_string());
        line("source_len", self.source_len.to_string());
        let gens: Vec<String> = self.root_generations.iter().map(u64::to_string).collect();
        line("root_generations", gens.join(","));
        line("root_origin", self.root_origin.to_string());
        line(
            "possible_lost_commit",
            self.possible_lost_commit.to_string(),
        );
        line("tail_len", opt(self.tail_len));
        line("logical_size", opt(self.logical_size));
        for c in [
            RangeClass::Valid,
            RangeClass::Lost,
            RangeClass::Unknown,
            RangeClass::Conflicting,
        ] {
            line(&format!("{}_bytes", c.as_str()), self.bytes(c).to_string());
        }
        line("extents_total", self.extents.len().to_string());
        line(
            "extents_listed",
            (self.extents.len() <= max_extents).to_string(),
        );
        for f in &self.findings {
            line(
                "finding",
                format!("{}:{}:{}", f.subject, f.status, opt(f.detail)),
            );
        }
        for s in &self.snapshots {
            line(
                "snapshot",
                format!("{}:{}:lost_blocks={}", s.id, s.status, s.lost_blocks),
            );
        }
        for e in self.extents.iter().take(max_extents) {
            line(
                "extent",
                format!("{}+{}:{}:{}", e.offset, e.len, e.class.as_str(), e.basis),
            );
        }
        match self.copy {
            CopyStatus::Published => line("copy", "published".to_string()),
            CopyStatus::NotCreated(r) => line("copy", format!("not_created:{r}")),
            CopyStatus::Failed(r) => line("copy", format!("failed:{r}")),
        }
        out
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SalvageOptions {
    pub selection: StateSelection,
    /// Also make the copy when the recovery is [`Recoverability::Partial`].
    /// Invalid ranges are then zero in the copy (see the module docs).
    pub publish_partial: bool,
}

/// The analysis itself could not be carried out.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SalvageError {
    /// The source could not be read (an I/O error on metadata that decides
    /// everything else). Nothing is claimed about the image.
    Source(StorageError),
    /// `new_uid` is zero.
    BadUid,
}

impl std::fmt::Display for SalvageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "salvage error: {self:?}")
    }
}

impl std::error::Error for SalvageError {}

pub struct SalvageOutcome<T: StorageBackend> {
    pub report: SalvageReport,
    /// The published copy, when [`CopyStatus::Published`].
    pub copy: Option<SnapFile<T>>,
}

// ---------------------------------------------------------------- internals

/// Why the metadata could not be established.
enum Fatal {
    Reason(&'static str),
    Source(StorageError),
}

enum BlockErr {
    Crc,
    Truncated,
    Io(StorageError),
}

fn read_block_desc<B: StorageBackend>(
    storage: &CheckedStorage<B>,
    d: &BlockDescriptor,
) -> Result<Block, BlockErr> {
    let mut buf = [0u8; BLOCK_SIZE];
    match storage.read_exact_at(d.phys, &mut buf) {
        Ok(()) => {}
        Err(
            StorageError::ShortRead { .. }
            | StorageError::OutOfRange { .. }
            | StorageError::OffsetOverflow { .. },
        ) => return Err(BlockErr::Truncated),
        Err(e) => return Err(BlockErr::Io(e)),
    }
    if crc32(&buf) != d.crc {
        return Err(BlockErr::Crc);
    }
    Ok(buf)
}

fn fatal_block(e: BlockErr, reason: &'static str) -> Fatal {
    match e {
        BlockErr::Io(s) => Fatal::Source(s),
        _ => Fatal::Reason(reason),
    }
}

/// A selected state reduced to what classification needs.
struct State {
    header: Header,
    head: Header,
    presence: Block,
    map: Vec<BlockDescriptor>,
    root: RootRecord,
}

fn load_state<B: StorageBackend>(
    storage: &CheckedStorage<B>,
    root: &RootRecord,
    map: Vec<BlockDescriptor>,
    selection: StateSelection,
) -> Result<State, Fatal> {
    let (Some(h), Some(p)) = (map.first(), map.get(1)) else {
        return Err(Fatal::Reason("layer_blocks_missing"));
    };
    let hb = read_block_desc(storage, h).map_err(|e| fatal_block(e, "layer_header_lost"))?;
    let head = decode_header(&hb).map_err(|e| {
        Fatal::Reason(match e {
            SnapError::NotSnapFile(_) => "not_a_layer_file",
            SnapError::UnsupportedHeader(_) => "unsupported_layer_header",
            _ => "layer_header_invalid",
        })
    })?;
    if head.building {
        return Err(Fatal::Reason("unpublished_target"));
    }
    match selection {
        StateSelection::Head => {
            let presence =
                read_block_desc(storage, p).map_err(|e| fatal_block(e, "presence_lost"))?;
            let materialized = (map.len() as u64).saturating_sub(DATA_BASE);
            if !bitmap_within(&presence, head.disk_blocks)
                || (materialized..MAX_DISK_BLOCKS).any(|b| bit(&presence, b))
            {
                return Err(Fatal::Reason("presence_invalid"));
            }
            Ok(State {
                header: head.clone(),
                head,
                presence,
                map,
                root: *root,
            })
        }
        StateSelection::Snapshot(id) => {
            let rec = head
                .snapshots
                .iter()
                .find(|r| r.id == id)
                .ok_or(Fatal::Reason("snapshot_missing"))?;
            if rec.generation >= root.generation || rec.end_offset > root.end_offset {
                return Err(Fatal::Reason("snapshot_record_invalid"));
            }
            let level = load_level(storage, rec).map_err(|e| match e {
                SnapError::Cow(CowError::Storage(s)) if !is_data_loss(&s) => Fatal::Source(s),
                _ => Fatal::Reason("snapshot_metadata_lost"),
            })?;
            if level.header.file_uid != head.file_uid || level.header.parent != head.parent {
                return Err(Fatal::Reason("snapshot_record_invalid"));
            }
            Ok(State {
                header: level.header,
                head,
                presence: level.presence,
                map: level.map,
                root: *root,
            })
        }
    }
}

/// A storage error that means "the bytes are not there" rather than "the
/// read failed".
fn is_data_loss(e: &StorageError) -> bool {
    matches!(
        e,
        StorageError::ShortRead { .. }
            | StorageError::OutOfRange { .. }
            | StorageError::OffsetOverflow { .. }
    )
}

fn map_reason(e: &ParseError) -> &'static str {
    match (e.section, e.kind) {
        (Section::Map, ParseKind::Crc { .. }) => "map_crc_mismatch",
        (Section::Slot, _) => "root_fields_invalid",
        (Section::Block, _) => "map_entry_invalid",
        _ => "map_invalid",
    }
}

fn slot_status(r: &Result<Slot, ParseError>) -> Result<(&'static str, Option<u64>), StorageError> {
    Ok(match r {
        Ok(Slot::Blank) => ("blank", None),
        Ok(Slot::Root(root)) => ("valid", Some(root.generation)),
        Err(e) => (
            match e.kind {
                ParseKind::Io(s) => return Err(s),
                ParseKind::Magic { .. } | ParseKind::Crc { .. } | ParseKind::Truncated { .. } => {
                    "damaged"
                }
                ParseKind::Generation => "misplaced_generation",
                ParseKind::Version { .. } | ParseKind::Identity => "unsupported_version",
                ParseKind::UnsupportedMandatoryFeature(_) => "unsupported_mandatory_feature",
                _ => "invalid",
            },
            Some(e.offset),
        ),
    })
}

struct Verdict {
    class: RangeClass,
    basis: &'static str,
    /// Bytes of a valid block that are not simply zero by format.
    data: Option<Block>,
}

struct Resolved {
    /// `Some` for a verified chain (empty for a base file); `None` when the
    /// parent is unavailable.
    chain: Option<Vec<Ancestor>>,
}

fn resolve<R: ParentResolver>(
    state: &State,
    resolver: &mut R,
    rep: &mut SalvageReport,
) -> Resolved {
    let Some(p) = state.header.parent else {
        return Resolved {
            chain: Some(Vec::new()),
        };
    };
    match resolve_chain(resolver, &p, state.header.file_uid) {
        Ok(chain) if chain.len() == state.header.depth as usize => {
            rep.find("parent", "verified", Some(p.file_uid));
            Resolved { chain: Some(chain) }
        }
        Ok(_) => {
            rep.find("parent", "chain_inconsistent", Some(p.file_uid));
            Resolved { chain: None }
        }
        Err(e) => {
            let (_, reason, _) = classify_snap(&e);
            rep.find("parent", reason, Some(p.file_uid));
            Resolved { chain: None }
        }
    }
}

fn classify_block<B: StorageBackend>(
    storage: &CheckedStorage<B>,
    state: &State,
    resolved: &Resolved,
    stale_root: bool,
    b: u64,
) -> Verdict {
    let verdict = |class, basis, data| Verdict { class, basis, data };
    if bit(&state.presence, b) {
        let Some(d) = b
            .checked_add(DATA_BASE)
            .and_then(|i| usize::try_from(i).ok())
            .and_then(|i| state.map.get(i))
        else {
            return verdict(RangeClass::Lost, "map_entry_missing", None);
        };
        return match read_block_desc(storage, d) {
            Ok(blk) => verdict(
                RangeClass::Valid,
                if stale_root {
                    "crc_verified_older_root"
                } else {
                    "crc_verified"
                },
                Some(blk),
            ),
            Err(BlockErr::Crc) => verdict(RangeClass::Lost, "block_crc_mismatch", None),
            Err(BlockErr::Truncated) => verdict(RangeClass::Lost, "block_truncated", None),
            Err(BlockErr::Io(_)) => verdict(RangeClass::Unknown, "block_io_error", None),
        };
    }
    if state.header.parent.is_none() {
        return verdict(RangeClass::Valid, "hole_zero", None);
    }
    match &resolved.chain {
        Some(chain) => match chain_read(chain, b) {
            Ok(blk) => verdict(RangeClass::Valid, "parent_chain", Some(blk)),
            Err(SnapError::BlockCorrupt) => {
                verdict(RangeClass::Lost, "parent_block_crc_mismatch", None)
            }
            Err(_) => verdict(RangeClass::Unknown, "parent_block_io_error", None),
        },
        None => verdict(RangeClass::Unknown, "parent_unavailable", None),
    }
}

/// Appends one block's verdict, merging with the previous extent when class
/// and basis are the same.
fn push_extent(extents: &mut Vec<Extent>, b: u64, class: RangeClass, basis: &'static str) {
    if let Some(last) = extents.last_mut()
        && last.class == class
        && last.basis == basis
        && last.offset + last.len == b * BS
    {
        last.len += BS;
        return;
    }
    extents.push(Extent {
        offset: b * BS,
        len: BS,
        class,
        basis,
    });
}

fn snapshot_statuses<B: StorageBackend>(
    storage: &CheckedStorage<B>,
    head: &Header,
    root: &RootRecord,
) -> Vec<SnapshotStatus> {
    head.snapshots
        .iter()
        .map(|rec| {
            let status = |status, lost_blocks| SnapshotStatus {
                id: rec.id,
                status,
                lost_blocks,
            };
            if rec.generation >= root.generation || rec.end_offset > root.end_offset {
                return status("record_invalid", 0);
            }
            let Ok(level) = load_level(storage, rec) else {
                return status("metadata_lost", 0);
            };
            let (mut lost, mut unknown) = (0u64, 0u64);
            for b in 0..level.header.disk_blocks {
                if let Some(d) = level.data_desc(b) {
                    match read_block_desc(storage, d) {
                        Ok(_) => {}
                        Err(BlockErr::Io(_)) => unknown += 1,
                        Err(_) => lost += 1,
                    }
                }
            }
            if lost > 0 {
                status("damaged", lost)
            } else if unknown > 0 {
                status("unverified", 0)
            } else {
                status("intact", 0)
            }
        })
        .collect()
}

/// The planned content of the copy and what was decided about it.
struct Analysis {
    report: SalvageReport,
    /// Disk bytes of the selected state; zeros where a range is not valid.
    data: Vec<u8>,
    source_uid: Option<u64>,
}

fn unrecoverable(mut rep: SalvageReport, reason: &'static str) -> Result<Analysis, SalvageError> {
    rep.recoverability = Recoverability::Unrecoverable;
    rep.reason = reason;
    Ok(Analysis {
        report: rep,
        data: Vec::new(),
        source_uid: None,
    })
}

fn analyze<S, R>(
    source: &CheckedStorage<S>,
    resolver: &mut R,
    selection: StateSelection,
) -> Result<Analysis, SalvageError>
where
    S: StorageBackend,
    R: ParentResolver,
{
    let size = source.size().map_err(SalvageError::Source)?;
    let mut rep = SalvageReport::new(selection, size);
    let src = StorageSource(source);
    if size < HEADER_LEN {
        rep.find("file", "shorter_than_root_slots", Some(size));
        return unrecoverable(rep, "no_valid_root");
    }
    let caps = Capabilities::CURRENT;
    let slots = [
        parse_slot_at(&src, 0, 0, &caps, Access::ReadOnly),
        parse_slot_at(&src, BS, 1, &caps, Access::ReadOnly),
    ];
    for (i, slot) in slots.iter().enumerate() {
        let (status, detail) = slot_status(slot).map_err(SalvageError::Source)?;
        rep.find(if i == 0 { "slot0" } else { "slot1" }, status, detail);
    }

    // Which roots the state is read from.
    let mut roots: Vec<(RootRecord, &'static str)> = Vec::new();
    let mut stale_root = false;
    match select_root(&src, size, &caps, Access::ReadOnly) {
        Ok(sel) => {
            let (root, origin) = match sel.forward {
                Some(copy) => (copy, "root_copy"),
                None => (
                    sel.chosen,
                    if sel.chosen.generation & 1 == 0 {
                        "slot0"
                    } else {
                        "slot1"
                    },
                ),
            };
            stale_root = sel.damaged && sel.forward.is_none();
            if sel.forward.is_some() {
                rep.find("root_copy", "adopted", Some(root.generation));
            }
            if stale_root {
                rep.find("newest_root", "possibly_lost", Some(root.generation));
            }
            rep.root_origin = origin;
            rep.root_generations.push(root.generation);
            rep.possible_lost_commit = stale_root;
            rep.tail_len = Some(size.saturating_sub(root.end_offset));
            roots.push((root, origin));
        }
        Err(CowError::Storage(s)) => return Err(SalvageError::Source(s)),
        Err(CowError::RootInconsistent(_) | CowError::GenerationConflict { .. }) => {
            // Two CRC-valid roots that cannot be ordered: both are examined,
            // neither is preferred.
            match (&slots[0], &slots[1]) {
                (Ok(Slot::Root(a)), Ok(Slot::Root(b))) => {
                    rep.root_origin = "ambiguous";
                    rep.root_generations = vec![a.generation, b.generation];
                    rep.find("roots", "unordered", None);
                    roots.push((*a, "slot0"));
                    roots.push((*b, "slot1"));
                }
                _ => return unrecoverable(rep, "root_inconsistent"),
            }
        }
        Err(CowError::NoValidRoot { .. }) => return unrecoverable(rep, "no_valid_root"),
        Err(CowError::IncompleteCreate { .. }) => return unrecoverable(rep, "incomplete_create"),
        Err(CowError::UnsupportedRoot(_)) => return unrecoverable(rep, "unsupported_root"),
        Err(CowError::UnsupportedMandatoryFeature(_)) => {
            return unrecoverable(rep, "unsupported_mandatory_feature");
        }
        Err(_) => return unrecoverable(rep, "root_invalid"),
    }

    // The map and the layer metadata of every root; any failure is fatal for
    // the recovery (no fallback to an older root).
    let ambiguous = roots.len() > 1;
    let mut states: Vec<State> = Vec::new();
    for (root, origin) in &roots {
        let map_subject = match (ambiguous, *origin) {
            (false, _) => "map",
            (true, "slot0") => "map_slot0",
            (true, _) => "map_slot1",
        };
        let map = match parse_map(&src, root) {
            Ok(m) => m,
            Err(e) => {
                if let ParseKind::Io(s) = e.kind {
                    return Err(SalvageError::Source(s));
                }
                let reason = map_reason(&e);
                rep.find(map_subject, reason, Some(e.offset));
                return unrecoverable(
                    rep,
                    if ambiguous {
                        "root_conflict_unresolved"
                    } else {
                        reason
                    },
                );
            }
        };
        rep.find(map_subject, "ok", Some(root.generation));
        match load_state(source, root, map, selection) {
            Ok(state) => {
                rep.find(
                    if ambiguous {
                        "layer_state_ok"
                    } else {
                        "layer_state"
                    },
                    "ok",
                    Some(state.header.disk_blocks),
                );
                states.push(state);
            }
            Err(Fatal::Source(s)) => return Err(SalvageError::Source(s)),
            Err(Fatal::Reason(r)) => {
                rep.find("layer_state", r, None);
                return unrecoverable(
                    rep,
                    if ambiguous {
                        "root_conflict_unresolved"
                    } else {
                        r
                    },
                );
            }
        }
    }
    let Some(first) = states.first() else {
        return unrecoverable(rep, "root_invalid");
    };
    let source_uid = first.head.file_uid;
    if !ambiguous {
        rep.snapshots = snapshot_statuses(source, &first.head, &first.root);
    }

    // Per-block classification.
    let resolved: Vec<Resolved> = states
        .iter()
        .map(|s| resolve(s, resolver, &mut rep))
        .collect();
    let disk_blocks = states
        .iter()
        .map(|s| s.header.disk_blocks)
        .max()
        .unwrap_or(0);
    let total = usize::try_from(disk_blocks.saturating_mul(BS))
        .map_err(|_| SalvageError::Source(StorageError::OffsetOverflow { offset: 0, len: 0 }))?;
    let mut data = vec![0u8; total];
    let mut extents: Vec<Extent> = Vec::new();
    for b in 0..disk_blocks {
        let verdicts: Vec<Option<Verdict>> = states
            .iter()
            .zip(resolved.iter())
            .map(|(s, r)| {
                (b < s.header.disk_blocks).then(|| classify_block(source, s, r, stale_root, b))
            })
            .collect();
        let (class, basis, bytes) = match verdicts.as_slice() {
            [Some(v)] => (v.class, v.basis, v.data),
            [a, b2] => combine(a.as_ref(), b2.as_ref()),
            _ => (RangeClass::Unknown, "no_state", None),
        };
        push_extent(&mut extents, b, class, basis);
        if let (RangeClass::Valid, Some(blk)) = (class, bytes) {
            let at = usize::try_from(b * BS).ok();
            if let Some(dst) = at.and_then(|a| data.get_mut(a..a + BLOCK_SIZE)) {
                dst.copy_from_slice(&blk);
            }
        }
    }
    rep.logical_size = Some(disk_blocks * BS);
    rep.extents = extents;
    let all_valid = rep.extents.iter().all(|e| e.class == RangeClass::Valid);
    // Complete needs a proven newest root: two unordered roots never prove one.
    if all_valid && !stale_root && !ambiguous {
        rep.recoverability = Recoverability::Complete;
        rep.reason = "ok";
    } else {
        rep.recoverability = Recoverability::Partial;
        rep.reason = if let Some(e) = rep.extents.iter().find(|e| e.class != RangeClass::Valid) {
            e.basis
        } else if ambiguous {
            "unordered_roots"
        } else {
            "possible_lost_commit"
        };
    }
    Ok(Analysis {
        report: rep,
        data,
        source_uid: Some(source_uid),
    })
}

/// Combines the verdicts of two unordered roots for one block.
fn combine(a: Option<&Verdict>, b: Option<&Verdict>) -> (RangeClass, &'static str, Option<Block>) {
    let (Some(a), Some(b)) = (a, b) else {
        return (RangeClass::Conflicting, "roots_disagree_on_size", None);
    };
    match (a.class, b.class) {
        (RangeClass::Valid, RangeClass::Valid) => {
            let zero = [0u8; BLOCK_SIZE];
            let (x, y) = (a.data.unwrap_or(zero), b.data.unwrap_or(zero));
            if x == y {
                (RangeClass::Valid, "agreed_by_both_roots", a.data)
            } else {
                (RangeClass::Conflicting, "roots_disagree", None)
            }
        }
        (RangeClass::Lost, RangeClass::Lost) => {
            (RangeClass::Lost, "block_damaged_in_both_roots", None)
        }
        _ => (RangeClass::Unknown, "one_root_unverified", None),
    }
}

enum CopyFail {
    Snap(SnapError),
    Mismatch,
}

impl From<SnapError> for CopyFail {
    fn from(e: SnapError) -> Self {
        Self::Snap(e)
    }
}

fn copy_fail_reason(f: &CopyFail) -> &'static str {
    match f {
        CopyFail::Mismatch => "verify_mismatch",
        CopyFail::Snap(SnapError::Cow(CowError::Storage(StorageError::Io {
            kind: ErrorKind::StorageFull,
            ..
        }))) => "no_space",
        CopyFail::Snap(SnapError::Cow(CowError::Storage(
            StorageError::UnsupportedDurability { .. },
        ))) => "durability_unavailable",
        CopyFail::Snap(_) => "target_io_error",
    }
}

fn build_copy<T: StorageBackend>(
    target: CheckedStorage<T>,
    new_uid: u64,
    data: &[u8],
) -> Result<SnapFile<T>, CopyFail> {
    let len = data.len() as u64;
    let mut image = SnapFile::create_building(target, new_uid)?;
    let step = COPY_COMMIT_BLOCKS * BS;
    let mut offset = 0u64;
    while offset < len {
        let n = step.min(len - offset);
        let (a, z) = (
            usize::try_from(offset).map_err(|_| SnapError::OutOfRange)?,
            usize::try_from(offset + n).map_err(|_| SnapError::OutOfRange)?,
        );
        let chunk = data.get(a..z).ok_or(SnapError::OutOfRange)?;
        image.write_at(offset, chunk)?;
        offset += n;
    }
    let (image, _ack) = image.finish_building(|built: &SnapFile<T>| {
        if built.disk_size() != len {
            return Err(CopyFail::Mismatch);
        }
        let mut back = vec![0u8; data.len()];
        let mut at = 0usize;
        while at < back.len() {
            let n = (step as usize).min(back.len() - at);
            let dst = back.get_mut(at..at + n).ok_or(SnapError::OutOfRange)?;
            built.read_at(at as u64, dst)?;
            at += n;
        }
        if back == data {
            Ok(())
        } else {
            Err(CopyFail::Mismatch)
        }
    })?;
    Ok(image)
}

/// Salvages the selected state of `source` into a new base image.
///
/// `source` is only read. The analysis runs first and decides everything
/// that can be decided without a target; `make_target` is called **only** if a
/// copy is to be made (a recoverable state, and for a partial recovery only
/// when [`SalvageOptions::publish_partial`] is set), and must return a new,
/// empty storage (an existing path is the host's refusal, reported as
/// [`CopyStatus::Failed`]`("target_exists")`; nothing existing is touched).
/// A target that is not writable and resizable (`"target_unsuitable"`) or
/// cannot make its file contents durable (`"durability_unavailable"`) is
/// refused before anything is written to it.
/// `new_uid` is the identity of the copy; it must differ from the source's.
///
/// The returned report is always complete for what was found, including for
/// an unrecoverable source and for a failed copy. A failed copy leaves at most
/// a *building* target that does not open; a target refused for its
/// capabilities is left exactly as `make_target` returned it.
pub fn salvage_to_copy<S, R, T>(
    source: CheckedStorage<S>,
    resolver: &mut R,
    options: &SalvageOptions,
    new_uid: u64,
    make_target: impl FnOnce() -> Result<CheckedStorage<T>, std::io::Error>,
) -> Result<SalvageOutcome<T>, SalvageError>
where
    S: StorageBackend,
    R: ParentResolver,
    T: StorageBackend,
{
    if new_uid == 0 {
        return Err(SalvageError::BadUid);
    }
    let Analysis {
        mut report,
        data,
        source_uid,
    } = analyze(&source, resolver, options.selection)?;
    let want_copy = match report.recoverability {
        Recoverability::Unrecoverable => {
            report.copy = CopyStatus::NotCreated("unrecoverable");
            false
        }
        Recoverability::Partial if !options.publish_partial => {
            report.copy = CopyStatus::NotCreated("partial_not_accepted");
            false
        }
        _ => true,
    };
    if !want_copy {
        return Ok(SalvageOutcome { report, copy: None });
    }
    if source_uid == Some(new_uid) {
        report.copy = CopyStatus::Failed("uid_not_new");
        return Ok(SalvageOutcome { report, copy: None });
    }
    let target = match make_target() {
        Ok(t) => t,
        Err(e) => {
            report.copy = CopyStatus::Failed(if e.kind() == ErrorKind::AlreadyExists {
                "target_exists"
            } else {
                "target_create_failed"
            });
            return Ok(SalvageOutcome { report, copy: None });
        }
    };
    // Refused before any write, as in export: the copy is published by a
    // durable commit, so a target that cannot be written, resized or made
    // durable would otherwise be left as a building image.
    let caps = target.capabilities();
    if !caps.writable || !caps.resize {
        report.copy = CopyStatus::Failed("target_unsuitable");
        return Ok(SalvageOutcome { report, copy: None });
    }
    if !caps.file_durability {
        report.copy = CopyStatus::Failed("durability_unavailable");
        return Ok(SalvageOutcome { report, copy: None });
    }
    match target.size() {
        Ok(0) => {}
        Ok(_) => {
            report.copy = CopyStatus::Failed("target_not_empty");
            return Ok(SalvageOutcome { report, copy: None });
        }
        Err(_) => {
            report.copy = CopyStatus::Failed("target_io_error");
            return Ok(SalvageOutcome { report, copy: None });
        }
    }
    match build_copy(target, new_uid, &data) {
        Ok(image) => {
            report.copy = CopyStatus::Published;
            Ok(SalvageOutcome {
                report,
                copy: Some(image),
            })
        }
        Err(f) => {
            report.copy = CopyStatus::Failed(copy_fail_reason(&f));
            Ok(SalvageOutcome { report, copy: None })
        }
    }
}
