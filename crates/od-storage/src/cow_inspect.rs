//! Read-only inspection and check of a prototype CoW image (S9-19).
//!
//! **Prototype, not a normative format.** This is the one library path a
//! tool uses to look at an image: it goes through the same strict parser and
//! the same layer decoder as [`SnapFile::open`](crate::cow_snapshot::SnapFile::open)
//! (no second decoder), but it never writes: it uses
//! [`CowView::inspect`](crate::cow_proto::CowView::inspect) (read-only
//! recovery) and read-only ancestors. A damaged file is *reported*, never
//! repaired: a root that a writable open would roll forward is shown as
//! `repair_needed`, and mutating recovery is a separate, explicit operation
//! that is not part of this module.
//!
//! # Modes
//!
//! * [`InspectMode::Info`]: recovery of the root (the head and every block it
//!   references are verified by the parser), the layer header, the recorded
//!   parent link. Retained snapshots and the parent chain are not verified.
//! * [`InspectMode::Check`]: `Info` plus every retained snapshot (its map,
//!   header, bitmap and **every block** it stores) and, for a child, the
//!   whole parent chain through the caller's [`ParentResolver`]; a parent the
//!   resolver cannot supply is [`Outcome::Corrupt`] with reason
//!   `parent_missing`, exactly as an open would refuse.
//!
//! # Output
//!
//! A report carries numbers and fixed reason codes only: ids, generations,
//! sizes, counts, and a `&'static str` reason. It never contains a block's
//! bytes, a path, or any text taken from the file, so diagnostics cannot leak
//! guest payload or secrets. [`InspectReport::render`] is bounded by
//! [`MAX_REPORT_BYTES`]; every list in it is bounded by a format limit.

use crate::cow_proto::{BLOCK_SIZE, CowError, CowView, OpenClass, SlotState, VERSION};
use crate::cow_snapshot::{
    HEADER_VERSION, ParentRef, ParentResolver, SnapError, chain_links, load_own_snapshots,
    read_desc, read_state_with, resolve_chain,
};
use crate::io::{CheckedStorage, StorageBackend};
use std::fmt::Write as _;

/// Largest rendered report, in bytes.
pub const MAX_REPORT_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InspectMode {
    Info,
    Check,
}

/// Integrity outcome. `Busy` is produced by the host layer when it cannot
/// take a read lease; the library itself never reports it for an open storage
/// except when a resolver says a parent is held by a writer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Ok,
    Corrupt,
    Unsupported,
    Busy,
    Io,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Corrupt => "corrupt",
            Self::Unsupported => "unsupported",
            Self::Busy => "busy",
            Self::Io => "io_error",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayerKind {
    Base,
    Child,
}

/// What the layer header says.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerInfo {
    pub kind: LayerKind,
    /// An unpublished compaction target: not a usable image.
    pub building: bool,
    pub file_uid: u64,
    pub logical_size: u64,
    pub disk_blocks: u64,
    /// Number of ancestors recorded in the header.
    pub depth: u32,
    pub next_snapshot_id: u64,
    pub snapshot_ids: Vec<u64>,
    pub parent: Option<ParentRef>,
}

/// One link of the parent chain: a file uid and the snapshot of it in use.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChainLink {
    pub file_uid: u64,
    pub snapshot_id: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectReport {
    pub mode: InspectMode,
    pub outcome: Outcome,
    /// Stable machine-readable reason (`"ok"` when the outcome is `Ok`).
    pub reason: &'static str,
    /// A number that locates the reason (block index, feature bit), if any.
    pub detail: Option<u64>,
    pub file_len: Option<u64>,
    pub block_size: u32,
    pub cow_version: u32,
    pub layer_version: u32,
    pub root_generation: Option<u64>,
    pub open_class: Option<OpenClass>,
    /// State of the root slot that does not hold the opened generation.
    pub other_slot: Option<&'static str>,
    /// A writable open would write (it would roll a damaged slot forward). The
    /// inspection itself wrote nothing.
    pub repair_needed: bool,
    pub layer: Option<LayerInfo>,
    /// Parent chain, nearest first. Verified only when `chain_verified`.
    pub chain: Vec<ChainLink>,
    pub chain_verified: bool,
}

impl InspectReport {
    fn new(mode: InspectMode) -> Self {
        Self {
            mode,
            outcome: Outcome::Ok,
            reason: "ok",
            detail: None,
            file_len: None,
            block_size: BLOCK_SIZE as u32,
            cow_version: VERSION,
            layer_version: HEADER_VERSION,
            root_generation: None,
            open_class: None,
            other_slot: None,
            repair_needed: false,
            layer: None,
            chain: Vec::new(),
            chain_verified: false,
        }
    }

    /// A report for a failure that happened before any storage was read (the
    /// host layer builds these for a missing or busy file).
    pub fn failed(
        mode: InspectMode,
        outcome: Outcome,
        reason: &'static str,
        detail: Option<u64>,
    ) -> Self {
        Self {
            outcome,
            reason,
            detail,
            ..Self::new(mode)
        }
    }

    /// First failure wins; later findings are still collected as fields.
    fn fail(&mut self, outcome: Outcome, reason: &'static str, detail: Option<u64>) {
        if self.outcome == Outcome::Ok {
            self.outcome = outcome;
            self.reason = reason;
            self.detail = detail;
        }
    }

    /// `key=value` lines, ASCII, at most [`MAX_REPORT_BYTES`] bytes. Contains
    /// only numbers and fixed codes.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let mut line = |k: &str, v: String| {
            let _ = writeln!(out, "{k}={v}");
        };
        let opt = |v: Option<u64>| v.map_or_else(|| "none".to_string(), |x| x.to_string());
        line("mode", format!("{:?}", self.mode).to_lowercase());
        line("outcome", self.outcome.as_str().to_string());
        line("reason", self.reason.to_string());
        line("detail", opt(self.detail));
        line("file_len", opt(self.file_len));
        line("block_size", self.block_size.to_string());
        line("cow_version", self.cow_version.to_string());
        line("layer_version", self.layer_version.to_string());
        line("root_generation", opt(self.root_generation));
        line(
            "open_class",
            self.open_class
                .map_or("none", |c| match c {
                    OpenClass::Committed => "committed",
                    OpenClass::UnfinishedCandidate => "unfinished_candidate",
                    OpenClass::RolledForward => "rolled_forward",
                    OpenClass::PossibleLostCommit => "possible_lost_commit",
                })
                .to_string(),
        );
        line("other_slot", self.other_slot.unwrap_or("none").to_string());
        line("repair_needed", self.repair_needed.to_string());
        if let Some(l) = &self.layer {
            let kind = match l.kind {
                LayerKind::Base => "base",
                LayerKind::Child => "child",
            };
            line("kind", kind.to_string());
            line("building", l.building.to_string());
            line("file_uid", l.file_uid.to_string());
            line("logical_size", l.logical_size.to_string());
            line("disk_blocks", l.disk_blocks.to_string());
            line("depth", l.depth.to_string());
            line("next_snapshot_id", l.next_snapshot_id.to_string());
            let ids: Vec<String> = l.snapshot_ids.iter().map(u64::to_string).collect();
            line("snapshots", ids.join(","));
        }
        let chain: Vec<String> = self
            .chain
            .iter()
            .map(|c| format!("{}:{}", c.file_uid, c.snapshot_id))
            .collect();
        line("chain", chain.join(","));
        line("chain_verified", self.chain_verified.to_string());
        if out.len() > MAX_REPORT_BYTES {
            // Unreachable with the format limits; bounded regardless.
            out.truncate(MAX_REPORT_BYTES - 11);
            out.push_str("\ntruncated\n");
        }
        out
    }
}

pub fn classify_cow(e: &CowError) -> (Outcome, &'static str, Option<u64>) {
    match e {
        CowError::Storage(_) => (Outcome::Io, "io_error", None),
        CowError::UnsupportedRoot(_) => (Outcome::Unsupported, "unsupported_root", None),
        CowError::UnsupportedMandatoryFeature(bit) => (
            Outcome::Unsupported,
            "unsupported_mandatory_feature",
            Some(u64::from(*bit)),
        ),
        CowError::NoValidRoot { .. } => (Outcome::Corrupt, "no_valid_root", None),
        CowError::IncompleteCreate { .. } => (Outcome::Corrupt, "incomplete_create", None),
        CowError::DamagedOtherSlot { .. } => (Outcome::Corrupt, "damaged_other_slot", None),
        CowError::GenerationConflict { generation } => {
            (Outcome::Corrupt, "generation_conflict", Some(*generation))
        }
        CowError::RootInconsistent(_) => (Outcome::Corrupt, "root_inconsistent", None),
        CowError::MapCorrupt => (Outcome::Corrupt, "map_corrupt", None),
        CowError::MapEntryOutOfRange { index } | CowError::MapEntryCollision { index } => {
            (Outcome::Corrupt, "map_entry_invalid", Some(*index))
        }
        CowError::BlockCorrupt { index } => (Outcome::Corrupt, "block_corrupt", Some(*index)),
        _ => (Outcome::Corrupt, "corrupt", None),
    }
}

pub fn classify_snap(e: &SnapError) -> (Outcome, &'static str, Option<u64>) {
    match e {
        SnapError::Cow(c) => classify_cow(c),
        SnapError::NotSnapFile(_) => (Outcome::Unsupported, "not_a_layer_file", None),
        SnapError::UnsupportedHeader(_) => (Outcome::Unsupported, "unsupported_layer_header", None),
        SnapError::HeaderCorrupt(_) => (Outcome::Corrupt, "layer_header_corrupt", None),
        SnapError::SnapshotCorrupt { id } => (Outcome::Corrupt, "snapshot_corrupt", Some(*id)),
        SnapError::SnapshotMissing { id } => (Outcome::Corrupt, "snapshot_missing", Some(*id)),
        SnapError::BlockCorrupt => (Outcome::Corrupt, "block_corrupt", None),
        SnapError::ParentMissing => (Outcome::Corrupt, "parent_missing", None),
        SnapError::ParentIdentityMismatch => (Outcome::Corrupt, "parent_identity_mismatch", None),
        SnapError::ParentBusy => (Outcome::Busy, "parent_busy", None),
        SnapError::ParentStorage(_) | SnapError::ParentIo(_) => {
            (Outcome::Io, "parent_io_error", None)
        }
        SnapError::ParentCorrupt(c) => {
            let (o, _, d) = classify_cow(c);
            (
                if o == Outcome::Io {
                    o
                } else {
                    Outcome::Corrupt
                },
                "parent_corrupt",
                d,
            )
        }
        SnapError::ParentDamaged => (Outcome::Corrupt, "parent_damaged", None),
        SnapError::ParentCycle => (Outcome::Corrupt, "parent_cycle", None),
        SnapError::ChainTooDeep { depth } => {
            (Outcome::Corrupt, "chain_too_deep", Some(u64::from(*depth)))
        }
        SnapError::ChainInconsistent(_) => (Outcome::Corrupt, "chain_inconsistent", None),
        SnapError::BuildInProgress => (Outcome::Corrupt, "unpublished_compaction_target", None),
        _ => (Outcome::Corrupt, "corrupt", None),
    }
}

fn slot_name(s: SlotState) -> &'static str {
    match s {
        SlotState::Blank => "blank",
        SlotState::Valid { .. } => "valid",
        SlotState::Damaged => "damaged",
    }
}

/// Inspects the image behind `storage` without writing to it.
///
/// `resolver` supplies read-only parents for [`InspectMode::Check`] of a
/// child; pass [`NoParents`](crate::cow_snapshot::NoParents) when there is
/// none (a child is then `parent_missing`). The storage is consumed only to
/// be read.
pub fn inspect_storage<B: StorageBackend, R: ParentResolver>(
    storage: CheckedStorage<B>,
    resolver: &mut R,
    mode: InspectMode,
) -> InspectReport {
    let mut rep = InspectReport::new(mode);
    match storage.size() {
        Ok(n) => rep.file_len = Some(n),
        Err(_) => {
            rep.fail(Outcome::Io, "io_error", None);
            return rep;
        }
    }
    let (view, report) = match CowView::inspect(storage) {
        Ok(v) => v,
        Err(e) => {
            let (o, r, d) = classify_cow(&e);
            rep.fail(o, r, d);
            return rep;
        }
    };
    rep.root_generation = Some(report.generation);
    rep.open_class = Some(report.class());
    rep.other_slot = Some(slot_name(report.other_slot));
    rep.repair_needed = report.rolled_forward;
    if report.possible_lost_commit {
        // Read-only recovery shows an older root; a newer acknowledged commit
        // may be lost. Reported as damage, never silently accepted.
        rep.fail(Outcome::Corrupt, "possible_lost_commit", None);
    }
    inspect_layer(&view, resolver, mode, &mut rep);
    rep
}

fn inspect_layer<B: StorageBackend, R: ParentResolver>(
    view: &CowView<B>,
    resolver: &mut R,
    mode: InspectMode,
    rep: &mut InspectReport,
) {
    let (header, _presence) = match read_state_with(view, true) {
        Ok(v) => v,
        Err(e) => {
            let (o, r, d) = classify_snap(&e);
            rep.fail(o, r, d);
            return;
        }
    };
    rep.layer = Some(LayerInfo {
        kind: if header.parent.is_some() {
            LayerKind::Child
        } else {
            LayerKind::Base
        },
        building: header.building,
        file_uid: header.file_uid,
        logical_size: header.disk_blocks.saturating_mul(BLOCK_SIZE as u64),
        disk_blocks: header.disk_blocks,
        depth: header.depth,
        next_snapshot_id: header.next_snapshot_id,
        snapshot_ids: header.snapshots.iter().map(|r| r.id).collect(),
        parent: header.parent,
    });
    if header.building {
        rep.fail(Outcome::Corrupt, "unpublished_compaction_target", None);
    }
    if let Some(p) = &header.parent {
        rep.chain.push(ChainLink {
            file_uid: p.file_uid,
            snapshot_id: p.snapshot_id,
        });
    }
    if mode == InspectMode::Info {
        return;
    }

    // Check: every retained snapshot, with every block it stores.
    match load_own_snapshots(view, &header) {
        Err(e) => {
            let (o, r, d) = classify_snap(&e);
            rep.fail(o, r, d);
        }
        Ok(levels) => {
            for (id, level) in &levels {
                for b in 0..level.header.disk_blocks {
                    if let Some(d) = level.data_desc(b)
                        && let Err(e) = read_desc(view.storage(), d)
                    {
                        let (o, _, _) = classify_snap(&e);
                        rep.fail(o, "snapshot_block_corrupt", Some(*id));
                        break;
                    }
                }
            }
        }
    }
    // A snapshot that no longer loads is already reported; keep going to the
    // chain, which is independent.
    if let Some(p) = &header.parent {
        match resolve_chain(resolver, p, header.file_uid) {
            Ok(chain) => {
                if chain.len() != header.depth as usize {
                    rep.fail(Outcome::Corrupt, "chain_inconsistent", None);
                } else {
                    rep.chain = chain_links(p, &chain)
                        .iter()
                        .map(|l| ChainLink {
                            file_uid: l.file_uid,
                            snapshot_id: l.snapshot_id,
                        })
                        .collect();
                    rep.chain_verified = true;
                }
            }
            Err(e) => {
                let (o, r, d) = classify_snap(&e);
                rep.fail(o, r, d);
            }
        }
    } else {
        rep.chain_verified = true;
    }
}
