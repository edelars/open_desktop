//! Read-only inspection of prototype disk images (S9-19).
//!
//! This crate is a thin host layer. It owns **no** parser: every byte of an
//! image is read through `od-storage` ([`od_storage::cow_inspect`], which uses
//! the strict parser and the layer decoder that `open` uses). What lives here
//! is only what the library cannot know: taking a read lease on a host file,
//! turning an outcome into a process exit status, and parsing arguments.
//!
//! # Exit statuses
//!
//! | code | meaning |
//! |---|---|
//! | 0 | image opened and every check of the chosen mode passed |
//! | 1 | usage error (nothing was read) |
//! | 2 | corrupt, inconsistent or incomplete (parent missing, identity mismatch, possible lost commit, unpublished target, ...) |
//! | 3 | unsupported (unknown mandatory feature, root version, or not an image of this format) |
//! | 4 | busy: a writer holds the file (or an ancestor) |
//! | 5 | I/O error (missing file, not a regular file, read failure) |
//!
//! # Read-only by construction
//!
//! The file is opened through a shared lease (`flock(LOCK_SH)`) with a
//! read-only backend: the tool has no write path. A root that a writable open
//! would roll forward is reported as `repair_needed=true` and left alone;
//! mutating recovery is a separate, explicit operation that this crate does
//! not offer. Output is `key=value` lines of numbers and fixed codes, at most
//! 4096 bytes, and never contains image payload, paths or text from the file.

use od_host::storage_io::{LeaseOpenError, ReadLease};
use od_host::storage_policy::{ParentLease, ParentLeaseError, ParentLeaseStorage};
use od_storage::cow_inspect::{InspectMode, InspectReport, Outcome, inspect_storage};
use od_storage::cow_snapshot::{ParentRef, ParentResolver, ResolveError};
use od_storage::io::CheckedStorage;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

pub use od_storage::cow_inspect::MAX_REPORT_BYTES;

/// Process exit status of the tool.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Exit {
    Ok = 0,
    Usage = 1,
    Corrupt = 2,
    Unsupported = 3,
    Busy = 4,
    Io = 5,
}

impl Exit {
    pub fn code(self) -> u8 {
        self as u8
    }
}

/// Maps an integrity outcome to its exit status.
pub fn exit_for(outcome: Outcome) -> Exit {
    match outcome {
        Outcome::Ok => Exit::Ok,
        Outcome::Corrupt => Exit::Corrupt,
        Outcome::Unsupported => Exit::Unsupported,
        Outcome::Busy => Exit::Busy,
        Outcome::Io => Exit::Io,
    }
}

/// Resolves parent files from `uid=path` arguments. A path is only where to
/// look: what is found there must still carry the recorded uid and snapshot
/// content, which the library verifies; the lease re-checks the file's
/// identity on every read.
struct PathResolver {
    paths: BTreeMap<u64, PathBuf>,
}

impl ParentResolver for PathResolver {
    type Backend = ParentLeaseStorage;

    fn open_parent(
        &mut self,
        parent: &ParentRef,
    ) -> Result<CheckedStorage<ParentLeaseStorage>, ResolveError> {
        let path = self
            .paths
            .get(&parent.file_uid)
            .ok_or(ResolveError::Missing)?;
        let lease = ParentLease::open(path).map_err(|e| match e {
            ParentLeaseError::Busy => ResolveError::Busy,
            ParentLeaseError::IdentityMismatch => ResolveError::IdentityMismatch,
            ParentLeaseError::Io(e) if e.kind() == io::ErrorKind::NotFound => ResolveError::Missing,
            ParentLeaseError::Io(e) => ResolveError::Io(e.kind()),
            ParentLeaseError::Backend(_) => ResolveError::Io(io::ErrorKind::Other),
        })?;
        CheckedStorage::new(ParentLeaseStorage::new(lease)).map_err(ResolveError::Storage)
    }
}

/// Inspects the image at `path` under a shared read lease. `parents` maps the
/// file uids of ancestors to paths (used by [`InspectMode::Check`] of a child).
/// Never writes.
pub fn inspect_file(path: &Path, mode: InspectMode, parents: &[(u64, PathBuf)]) -> InspectReport {
    let lease = match ReadLease::open(path) {
        Ok(lease) => lease,
        Err(LeaseOpenError::LeaseBusy) => {
            return InspectReport::failed(mode, Outcome::Busy, "busy", None);
        }
        Err(LeaseOpenError::IdentityMismatch) => {
            return InspectReport::failed(mode, Outcome::Io, "path_changed", None);
        }
        Err(LeaseOpenError::Io(e)) => {
            let reason = match e.kind() {
                io::ErrorKind::NotFound => "not_found",
                io::ErrorKind::InvalidInput => "not_a_regular_file",
                io::ErrorKind::PermissionDenied => "permission_denied",
                _ => "io_error",
            };
            return InspectReport::failed(mode, Outcome::Io, reason, None);
        }
    };
    let Ok(storage) = CheckedStorage::new(lease) else {
        return InspectReport::failed(mode, Outcome::Io, "io_error", None);
    };
    let mut resolver = PathResolver {
        paths: parents.iter().cloned().collect(),
    };
    inspect_storage(storage, &mut resolver, mode)
}

/// [`inspect_file`] in [`InspectMode::Check`].
pub fn check_file(path: &Path, parents: &[(u64, PathBuf)]) -> InspectReport {
    inspect_file(path, InspectMode::Check, parents)
}

const USAGE: &str = "usage: od-image-tool info <image>\n       od-image-tool check <image> [--parent UID=PATH]...\n";

/// Result of one invocation: the status and what to print.
#[derive(Debug, Eq, PartialEq)]
pub struct Run {
    pub exit: Exit,
    pub stdout: String,
    pub stderr: String,
}

fn usage(message: &str) -> Run {
    Run {
        exit: Exit::Usage,
        stdout: String::new(),
        stderr: format!("{message}\n{USAGE}"),
    }
}

/// Runs the tool for `args` (without the program name).
pub fn run(args: &[OsString]) -> Run {
    let Some(command) = args.first().and_then(|a| a.to_str()) else {
        return usage("missing or invalid command");
    };
    let mode = match command {
        "info" => InspectMode::Info,
        "check" => InspectMode::Check,
        _ => return usage("unknown command"),
    };
    let mut path: Option<&OsString> = None;
    let mut parents: Vec<(u64, PathBuf)> = Vec::new();
    let mut rest = args.iter().skip(1);
    while let Some(arg) = rest.next() {
        if arg.to_str() == Some("--parent") && mode == InspectMode::Check {
            let Some(spec) = rest.next().and_then(|s| s.to_str()) else {
                return usage("--parent needs UID=PATH");
            };
            let Some((uid, p)) = spec.split_once('=') else {
                return usage("--parent needs UID=PATH");
            };
            let Ok(uid) = uid.parse::<u64>() else {
                return usage("--parent uid is not a number");
            };
            if p.is_empty() || parents.len() >= 16 {
                return usage("--parent path is empty or too many parents");
            }
            parents.push((uid, PathBuf::from(p)));
        } else if arg.to_str().is_some_and(|a| a.starts_with("--")) || path.is_some() {
            return usage("unexpected argument");
        } else {
            path = Some(arg);
        }
    }
    let Some(path) = path else {
        return usage("missing image path");
    };
    let report = inspect_file(Path::new(path), mode, &parents);
    Run {
        exit: exit_for(report.outcome),
        stdout: report.render(),
        stderr: String::new(),
    }
}
