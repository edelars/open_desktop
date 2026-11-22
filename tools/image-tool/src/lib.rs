//! Read-only inspection of prototype disk images (S9-19).
//!
//! This crate is a thin host layer. It owns **no** parser: every byte of an
//! image is read through `od-storage` ([`od_storage::cow_inspect`], which uses
//! the strict parser and the layer decoder that `open` uses). What lives here
//! is only what the library cannot know: taking a read lease on a host file,
//! turning an outcome into a process exit status, and parsing arguments.
//! `import-install-media` converts the supported ISO subset to a new raw disk
//! after storage verifies the generated GPT, FAT32 and copied file bytes.
//!
//! # `make-data-disk` and `add-install-file` (S8-27, S8-22)
//!
//! Both build a new raw disk with the same GPT and FAT32 writer and publish
//! it like `import-install-media` (`<out>.partial`, read-back, no-replace
//! link; an existing output or `.partial` is refused before any input is
//! read). Inputs are only read, under share locks held until publication.
//!
//! * `make-data-disk <manifest.json> <out.raw> [--nonce VALUE]` builds the
//!   payload disk: one Microsoft Basic Data partition (not an ESP, so Windows
//!   assigns a drive letter) with a FAT32 volume labelled `OD8PAYLOAD` and the
//!   directory `OD8\`: `nonce.txt` (the value of `--nonce`, which must be
//!   exactly 32 lowercase hex digits, else exit 3 `nonce_invalid` and nothing
//!   is created; empty without `--nonce`),
//!   an empty `markers.txt` and the manifest's files. The SHA-256 of every
//!   input is checked against the manifest before the output is created and
//!   again before it is published; a mismatch is exit 2
//!   (`input_hash_mismatch`, `detail` is the index in the manifest). A
//!   mismatch in the first check creates nothing (`partial_left=false`); a
//!   mismatch in the second check, which happens after `<out>.partial` was
//!   written, leaves that file behind (`partial_left=true`, `published=false`)
//!   and the caller deletes it. The manifest format is described in
//!   `tests/compat/payloads/` (example) terms: `version` (1), optional
//!   `comment`, optional `spare_mib` (free space left for the guest, default
//!   512), and `files`: `path` (inside `OD8/`), `source` (relative to the
//!   manifest), `sha256`, optional `origin`. A malformed manifest is exit 3.
//!   `spare_mib` is 1 to 65535 (`manifest_spare` otherwise). The reader of
//!   the storage crate accepts a FAT of at most 64 MiB (about 64 GiB of 4 KiB
//!   clusters) and directories at most 16 deep (`OD8` counts as the first);
//!   both are checked before anything is created: a path deeper than that is
//!   `manifest_depth` and a volume whose FAT would be larger (a big payload
//!   next to a big `spare_mib`) is `fat_capacity`, both exit 3. The writer
//!   does not check the reader's other limits (entry count, directory size),
//!   which the manifest limit of 128 files keeps far away.
//! * `add-install-file <in.raw> <out.raw> <path> <file>` writes a new disk
//!   that is the converted installation disk `<in.raw>` plus `<file>` at
//!   `<path>` (for example `sources\Autounattend.xml`; the directory must
//!   exist, the file must not). `<in.raw>` is never changed; the output is
//!   laid out again by the writer, not patched in place. The output reports
//!   `input_sha256`, `added_file_sha256` and `output_sha256`, and fails
//!   (`input_changed`, exit 2) when an input changed during the run; as with
//!   the hash check above, a change found only before publication leaves
//!   `<out>.partial` (`partial_left=true`).
//!
//! # Exit statuses
//!
//! | code | meaning |
//! |---|---|
//! | 0 | image opened and every check of the chosen mode passed |
//! | 1 | usage error (nothing was read) |
//! | 2 | corrupt, inconsistent or incomplete (parent missing, identity mismatch, possible lost commit, unpublished target, ...) |
//! | 3 | unsupported (unknown mandatory feature, root version, or not an image of this format; for `import`, a RAW that is not whole 512-byte blocks or exceeds one layer) |
//! | 4 | busy: a writer holds the file (or an ancestor) |
//! | 5 | I/O error (missing file, not a regular file, read failure) |
//!
//! # Read-only by construction (`info`, `check`)
//!
//! The file is opened through a shared lease (`flock(LOCK_SH)`) with a
//! read-only backend: these commands have no write path. A root that a writable open
//! would roll forward is reported as `repair_needed=true` and left alone;
//! mutating recovery is a separate, explicit operation that this crate does
//! not offer. Output is `key=value` lines of numbers and fixed codes, at most
//! 4096 bytes, and never contains image payload, paths or text from the file.
//!
//! # `export` and `salvage` (S9-20, S9-21)
//!
//! Both read the source under the same shared lease (never the active disk of
//! a writer: a held writer is exit 4) and write **only new files**; neither
//! ever truncates, replaces or removes an existing path, and the source is
//! never written.
//!
//! * `export <image> <out.raw> [--snapshot ID] [--parent UID=PATH]...` writes
//!   the head (or a named snapshot) as a plain RAW of exactly the logical size.
//!   The data goes to `<out.raw>.partial` (created exclusively), is made
//!   durable, read back and compared byte for byte with a second read of the
//!   source, and only then gets the name `<out.raw>` by an atomic no-replace
//!   hard link. On any failure before the link `<out.raw>` is **not created**; the
//!   file that is left is `<out.raw>.partial`. If the link was made but the name
//!   no longer names the written file, the run exits 5 with `target_unverified`
//!   and `target_left=true`: `<out.raw>` is left in place, is not removed, and
//!   its bytes must not be trusted, named as incomplete (it is never deleted:
//!   without namespace guarantees the tool does not remove paths it cannot
//!   prove it still owns). An existing `<out.raw>` or `.partial` is refused
//!   (exit 5, `target_exists` / `partial_exists`) before anything is written.
//!   Not covered: durable atomic rename (ISSUE-055; the new name has no
//!   directory flush), and a host without file durability is refused
//!   (`durability_unavailable`).
//! * `salvage <image> <new.img> <report.txt> [--snapshot ID]
//!   [--parent UID=PATH]... [--accept-partial]` analyses a possibly damaged
//!   image and writes a **new** image plus a report file (see
//!   [`od_storage::cow_salvage`]). The exit status says how much was
//!   recovered: 0 only when everything was proven valid; 2 for a partial or
//!   unrecoverable source; 3 for an unsupported format; 5 when the copy or
//!   report could not be written. An unrecoverable source gets no `<new.img>`
//!   at all, and a partial one gets none unless `--accept-partial` is given.
//!   A copy that failed midway is a *building* image that does not open. A
//!   host without file durability is refused before anything is written
//!   (exit 5, `copy=failed:durability_unavailable`); the empty `<new.img>`
//!   created exclusively is left in place, so a rerun reports `target_exists`.
//!
//! # `import` (S9-20, S9-22)
//!
//! `import <in.raw> <new.img>` is the one supported way into the CoW v1
//! format: it builds a new base image from a plain RAW file (see
//! [`od_storage::cow_export::import_raw`]). The RAW is read under the same
//! shared lease (a held writer is exit 4) and never written. A RAW that is not
//! a whole number of 512-byte blocks or has more than `MAX_DISK_BLOCKS` blocks
//! (the limit of one prototype layer) is exit 3 and creates nothing. Otherwise
//! `<new.img>` is created exclusively (an existing name is exit 5,
//! `target_exists`, untouched); the image is not published until it was read
//! back, compared with a fresh read of the RAW and committed durably. Every
//! run prints `target_left` (whether this run created `<new.img>`; a host
//! I/O error inside the exclusive create is reported as not created). A failure
//! after creation is exit 5 with `target_left=true`: the file stays in place
//! and either does not open as an image (an unfinished building image or a
//! file without a valid header) or, when the fault hit only the
//! acknowledgement of the publishing commit, is a complete copy (`check`
//! tells which); it never opens as a partial image. A host without file
//! durability is refused before anything is written (`durability_unavailable`;
//! an empty `<new.img>` is left). Not covered: durable directory entry
//! (ISSUE-055).

use od_host::storage_io::{HostFileStorage, LeaseOpenError, ReadLease, WriterOpenError};
use od_host::storage_policy::{
    ParentLease, ParentLeaseError, ParentLeaseStorage, PolicyError, PublishError,
    export_with_lease, publish_no_replace,
};
use od_storage::cow_export::{ExportOptions, ImportError, StateSelection, export_raw, import_raw};
use od_storage::cow_inspect::{InspectMode, InspectReport, Outcome, inspect_storage};
use od_storage::cow_salvage::{CopyStatus, SalvageError, SalvageOptions, salvage_to_copy};
use od_storage::cow_snapshot::{ParentRef, ParentResolver, ResolveError};
use od_storage::install_media::InstallMedia;
use od_storage::io::{BackendError, CheckedStorage, StorageBackend, StorageCapabilities};
use od_storage::testing::rng::SplitMix64;
mod media;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::io;
use std::io::Write as _;
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

const USAGE: &str = "usage: od-image-tool info <image>\n       od-image-tool check <image> [--parent UID=PATH]...\n       od-image-tool export <image> <out.raw> [--snapshot ID] [--parent UID=PATH]...\n       od-image-tool salvage <image> <new.img> <report.txt> [--snapshot ID] [--parent UID=PATH]... [--accept-partial]\n       od-image-tool import <in.raw> <new.img>\n       od-image-tool create-raw <directory> <name> <capacity-bytes> (macOS)\n       od-image-tool import-install-media <image.iso> <out.raw>\n       od-image-tool make-data-disk <manifest.json> <out.raw> [--nonce VALUE]\n       od-image-tool add-install-file <in.raw> <out.raw> <path-in-media> <file>\n";

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    Info,
    Check,
    Export,
    Import,
    Salvage,
    CreateRaw,
    ImportInstallMedia,
    MakeDataDisk,
    AddInstallFile,
}

impl Command {
    fn positionals(self) -> usize {
        match self {
            Self::Info | Self::Check => 1,
            Self::Export | Self::Import | Self::ImportInstallMedia => 2,
            Self::Salvage | Self::CreateRaw => 3,
            Self::MakeDataDisk => 2,
            Self::AddInstallFile => 4,
        }
    }

    fn takes_parent(self) -> bool {
        matches!(self, Self::Check | Self::Export | Self::Salvage)
    }

    fn takes_snapshot(self) -> bool {
        matches!(self, Self::Export | Self::Salvage)
    }
}

struct Parsed<'a> {
    command: Command,
    paths: Vec<&'a OsString>,
    parents: Vec<(u64, PathBuf)>,
    snapshot: Option<u64>,
    accept_partial: bool,
    nonce: Option<&'a str>,
}

fn parse(args: &[OsString]) -> Result<Parsed<'_>, Run> {
    let Some(name) = args.first().and_then(|a| a.to_str()) else {
        return Err(usage("missing or invalid command"));
    };
    let command = match name {
        "info" => Command::Info,
        "check" => Command::Check,
        "export" => Command::Export,
        "salvage" => Command::Salvage,
        "import" => Command::Import,
        "import-install-media" => Command::ImportInstallMedia,
        "make-data-disk" => Command::MakeDataDisk,
        "add-install-file" => Command::AddInstallFile,
        #[cfg(target_os = "macos")]
        "create-raw" => Command::CreateRaw,
        _ => return Err(usage("unknown command")),
    };
    let mut parsed = Parsed {
        command,
        paths: Vec::new(),
        parents: Vec::new(),
        snapshot: None,
        accept_partial: false,
        nonce: None,
    };
    let mut rest = args.iter().skip(1);
    while let Some(arg) = rest.next() {
        match arg.to_str() {
            Some("--parent") if command.takes_parent() => {
                let Some(spec) = rest.next().and_then(|s| s.to_str()) else {
                    return Err(usage("--parent needs UID=PATH"));
                };
                let Some((uid, p)) = spec.split_once('=') else {
                    return Err(usage("--parent needs UID=PATH"));
                };
                let Ok(uid) = uid.parse::<u64>() else {
                    return Err(usage("--parent uid is not a number"));
                };
                if p.is_empty() || parsed.parents.len() >= 16 {
                    return Err(usage("--parent path is empty or too many parents"));
                }
                parsed.parents.push((uid, PathBuf::from(p)));
            }
            Some("--snapshot") if command.takes_snapshot() => {
                let Some(id) = rest.next().and_then(|s| s.to_str()) else {
                    return Err(usage("--snapshot needs an id"));
                };
                match id.parse::<u64>() {
                    Ok(id) if id != 0 && parsed.snapshot.is_none() => parsed.snapshot = Some(id),
                    _ => return Err(usage("--snapshot id is not a non-zero number or repeated")),
                }
            }
            Some("--accept-partial") if command == Command::Salvage => {
                parsed.accept_partial = true;
            }
            Some("--nonce") if command == Command::MakeDataDisk => {
                match rest.next().and_then(|s| s.to_str()) {
                    Some(nonce) if parsed.nonce.is_none() => parsed.nonce = Some(nonce),
                    _ => return Err(usage("--nonce needs a value and may not repeat")),
                }
            }
            Some(a) if a.starts_with("--") => return Err(usage("unexpected argument")),
            _ => {
                if parsed.paths.len() >= command.positionals() {
                    return Err(usage("unexpected argument"));
                }
                parsed.paths.push(arg);
            }
        }
    }
    if parsed.paths.len() != command.positionals() {
        return Err(usage(match command {
            Command::Info | Command::Check => "missing image path",
            Command::Export => "export needs an image and an output path",
            Command::Import => "import needs a RAW input and a new image path",
            Command::Salvage => "salvage needs an image, a new image path and a report path",
            Command::CreateRaw => "create-raw needs a directory, name and capacity-bytes",
            Command::ImportInstallMedia => "import-install-media needs an ISO and an output path",
            Command::MakeDataDisk => "make-data-disk needs a manifest and an output path",
            Command::AddInstallFile => {
                "add-install-file needs an input disk, an output path, a path in the media and a file"
            }
        }));
    }
    Ok(parsed)
}

/// Runs the tool for `args` (without the program name).
pub fn run(args: &[OsString]) -> Run {
    let parsed = match parse(args) {
        Ok(p) => p,
        Err(run) => return run,
    };
    let selection = parsed
        .snapshot
        .map_or(StateSelection::Head, StateSelection::Snapshot);
    match (parsed.command, parsed.paths.as_slice()) {
        (Command::Info | Command::Check, [path]) => {
            let mode = if parsed.command == Command::Info {
                InspectMode::Info
            } else {
                InspectMode::Check
            };
            let report = inspect_file(Path::new(path), mode, &parsed.parents);
            Run {
                exit: exit_for(report.outcome),
                stdout: report.render(),
                stderr: String::new(),
            }
        }
        (Command::Export, [src, out]) => {
            export_file(Path::new(src), Path::new(out), selection, &parsed.parents)
        }
        (Command::Import, [src, new]) => import_file(Path::new(src), Path::new(new)),
        (Command::Salvage, [src, new, report]) => salvage_file(
            Path::new(src),
            Path::new(new),
            Path::new(report),
            selection,
            &parsed.parents,
            parsed.accept_partial,
        ),
        (Command::ImportInstallMedia, [src, out]) => {
            import_install_media(Path::new(src), Path::new(out))
        }
        (Command::MakeDataDisk, [manifest, out]) => {
            media::make_data_disk(Path::new(manifest), Path::new(out), parsed.nonce)
        }
        (Command::AddInstallFile, [input, out, media_path, file]) => match media_path.to_str() {
            Some(media_path) => media::add_install_file(
                Path::new(input),
                Path::new(out),
                media_path,
                Path::new(file),
            ),
            None => usage("path in the media must be UTF-8"),
        },
        #[cfg(target_os = "macos")]
        (Command::CreateRaw, [directory, name, capacity]) => {
            create_raw_file(Path::new(directory), name, capacity)
        }
        _ => usage("missing path"),
    }
}

#[cfg(target_os = "macos")]
fn create_raw_file(directory: &Path, name: &OsString, capacity: &OsString) -> Run {
    let Some(name) = name.to_str() else {
        return usage("RAW name must be UTF-8");
    };
    let Some(capacity) = capacity.to_str().and_then(|s| s.parse::<u64>().ok()) else {
        return usage("capacity-bytes must be an unsigned integer");
    };
    match od_host::create_raw_for_image_tool(directory, name.as_bytes(), capacity) {
        Ok(()) => Run {
            exit: Exit::Ok,
            stdout: format!("created=1\ncapacity_bytes={capacity}\n"),
            stderr: String::new(),
        },
        Err(status) => Run {
            exit: match status {
                7 => Exit::Busy,
                1 | 2 => Exit::Usage,
                _ => Exit::Io,
            },
            stdout: String::new(),
            stderr: format!("create_failed=status_{status}\n"),
        },
    }
}

// ------------------------------------------------------------ shared pieces

/// A read-only backend over a lease that the caller keeps holding (the lease
/// is taken by [`export_with_lease`], which owns it for the whole call).
struct LeaseRef<'a>(&'a mut ReadLease);

impl StorageBackend for LeaseRef<'_> {
    fn capabilities(&self) -> StorageCapabilities {
        self.0.capabilities()
    }

    fn size(&self) -> Result<u64, BackendError> {
        self.0.size()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, BackendError> {
        self.0.read_at(offset, buf)
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<usize, BackendError> {
        self.0.write_at(offset, buf)
    }
}

/// A writable backend over the exclusive writer lock that [`export_file`]
/// keeps until publication. Export writes through this handle, and the lock
/// stays with the shared writer, so `.partial` is never unlocked in between.
struct SharedWriter(std::rc::Rc<std::cell::RefCell<HostFileStorage>>);

impl StorageBackend for SharedWriter {
    fn capabilities(&self) -> StorageCapabilities {
        self.0.borrow().capabilities()
    }

    fn size(&self) -> Result<u64, BackendError> {
        self.0.borrow().size()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, BackendError> {
        self.0.borrow().read_at(offset, buf)
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<usize, BackendError> {
        self.0.borrow_mut().write_at(offset, buf)
    }

    fn set_len(&mut self, len: u64) -> Result<(), BackendError> {
        self.0.borrow_mut().set_len(len)
    }

    fn flush_file_durable(&mut self) -> Result<(), BackendError> {
        self.0.borrow_mut().flush_file_durable()
    }

    fn flush_namespace_durable(&mut self) -> Result<(), BackendError> {
        self.0.borrow_mut().flush_namespace_durable()
    }
}

/// `key=value` output of a command that is not an inspection.
struct Lines(String);

impl Lines {
    fn new(command: &str) -> Self {
        let mut s = Self(String::new());
        s.put("command", command);
        s
    }

    fn put(&mut self, key: &str, value: impl std::fmt::Display) -> &mut Self {
        let _ = writeln!(self.0, "{key}={value}");
        self
    }

    fn finish(mut self) -> String {
        if self.0.len() > MAX_REPORT_BYTES {
            self.0.truncate(MAX_REPORT_BYTES - 11);
            self.0.push_str("\ntruncated\n");
        }
        self.0
    }
}

fn failure(command: &str, outcome: Outcome, reason: &str, detail: Option<u64>) -> Lines {
    let mut l = Lines::new(command);
    l.put("outcome", outcome.as_str())
        .put("reason", reason)
        .put(
            "detail",
            detail.map_or_else(|| "none".to_string(), |d| d.to_string()),
        );
    l
}

fn io_reason(e: &io::Error) -> &'static str {
    match e.kind() {
        io::ErrorKind::NotFound => "not_found",
        io::ErrorKind::InvalidInput => "not_a_regular_file",
        io::ErrorKind::PermissionDenied => "permission_denied",
        io::ErrorKind::AlreadyExists => "target_exists",
        io::ErrorKind::StorageFull => "no_space",
        _ => "io_error",
    }
}

fn policy_failure(command: &str, e: &PolicyError) -> (Lines, Outcome) {
    let (outcome, reason) = match e {
        PolicyError::ExportBusy | PolicyError::RepairBusy => (Outcome::Busy, "busy"),
        PolicyError::IdentityMismatch => (Outcome::Io, "path_changed"),
        PolicyError::Io(e) => (Outcome::Io, io_reason(e)),
    };
    (failure(command, outcome, reason, None), outcome)
}

fn run_of(lines: Lines, outcome: Outcome) -> Run {
    Run {
        exit: exit_for(outcome),
        stdout: lines.finish(),
        stderr: String::new(),
    }
}

/// Whether anything (even a dangling link) already has this name.
fn name_taken(path: &Path) -> io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Appends `suffix` to the final component.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// A fresh, non-zero file identity for a new image. Identity only has to
/// differ between files; it is not a secret.
fn fresh_uid() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let mut rng = SplitMix64(nanos ^ (u64::from(std::process::id()) << 32));
    loop {
        let uid = rng.next_u64();
        if uid != 0 {
            return uid;
        }
    }
}

fn import_install_media(input: &Path, target: &Path) -> Run {
    const COMMAND: &str = "import-install-media";
    match convert_install_media(input, target) {
        Ok(lines) => run_of(lines, Outcome::Ok),
        Err(refusal) => refusal.run(COMMAND),
    }
}

fn convert_install_media(input: &Path, target: &Path) -> Result<Lines, media::Refusal> {
    let partial = with_suffix(target, ".partial");
    media::refuse_existing(target, &partial)?;
    let lease = ReadLease::open(input).map_err(media::lease_refusal)?;
    let source =
        CheckedStorage::new(lease).map_err(|_| media::Refusal::new(Outcome::Io, "io_error"))?;
    let plan = InstallMedia::from_iso(&source).map_err(|e| media::plan_refusal(&e))?;
    let written = media::write_and_publish(&plan, &source, target, fresh_uid(), false, || Ok(()))?;
    let mut lines = Lines::new("import-install-media");
    lines
        .put("outcome", "ok")
        .put("reason", "ok")
        .put("logical_size", plan.disk_bytes())
        .put("files", plan.file_count())
        .put("verified", true)
        .put("file_durable", true)
        .put("published", true)
        .put("namespace_durable", false)
        .put("partial_removed", written.partial_removed);
    Ok(lines)
}

// ------------------------------------------------------------------ export

/// How one export attempt inside the lease ended.
enum ExportStep {
    Done(od_storage::cow_export::ExportReport),
    Failed(Outcome, &'static str, Option<u64>),
}

/// Exports the selected state of `src` into the new file `out`. See the crate
/// docs ("`export` and `salvage`") for the publication rule.
pub fn export_file(
    src: &Path,
    out: &Path,
    selection: StateSelection,
    parents: &[(u64, PathBuf)],
) -> Run {
    let partial = with_suffix(out, ".partial");
    for (path, reason) in [
        (out, "target_exists"),
        (partial.as_path(), "partial_exists"),
    ] {
        match name_taken(path) {
            Ok(false) => {}
            Ok(true) => {
                let mut l = failure("export", Outcome::Io, reason, None);
                l.put("published", false)
                    .put("partial_left", reason == "partial_exists");
                return run_of(l, Outcome::Io);
            }
            Err(e) => {
                let mut l = failure("export", Outcome::Io, io_reason(&e), None);
                l.put("published", false).put("partial_left", false);
                return run_of(l, Outcome::Io);
            }
        }
    }
    let mut resolver = PathResolver {
        paths: parents.iter().cloned().collect(),
    };
    // Created only after the source has opened and verified; `created` and
    // `identity` record what the closure did so the failure paths can say
    // whether a partial file exists.
    let created = std::cell::Cell::new(false);
    let held: std::cell::RefCell<Option<std::rc::Rc<std::cell::RefCell<HostFileStorage>>>> =
        std::cell::RefCell::new(None);
    let step = export_with_lease(src, |lease| {
        let Ok(source) = CheckedStorage::new(LeaseRef(lease)) else {
            return ExportStep::Failed(Outcome::Io, "io_error", None);
        };
        let result = export_raw(
            source,
            &mut resolver,
            selection,
            || {
                let file =
                    HostFileStorage::create_new_exclusive(&partial, 0).map_err(|e| match e {
                        WriterOpenError::Io(e) => e,
                        WriterOpenError::WriterBusy => io::Error::from(io::ErrorKind::WouldBlock),
                        WriterOpenError::IdentityMismatch => io::Error::from(io::ErrorKind::Other),
                    })?;
                created.set(true);
                let shared = std::rc::Rc::new(std::cell::RefCell::new(file));
                *held.borrow_mut() = Some(std::rc::Rc::clone(&shared));
                CheckedStorage::new(SharedWriter(shared))
                    .map_err(|_| io::Error::other("capabilities"))
            },
            &ExportOptions::default(),
            &|| false,
        );
        match result {
            Ok(report) => ExportStep::Done(report),
            Err(e) => {
                let (outcome, reason, detail) = e.classify();
                ExportStep::Failed(outcome, reason, detail)
            }
        }
    });
    let step = match step {
        Ok(step) => step,
        Err(e) => {
            let (mut l, outcome) = policy_failure("export", &e);
            l.put("published", false).put("partial_left", created.get());
            return run_of(l, outcome);
        }
    };
    let report = match step {
        ExportStep::Failed(outcome, reason, detail) => {
            let mut l = failure("export", outcome, reason, detail);
            l.put("published", false).put("partial_left", created.get());
            return run_of(l, outcome);
        }
        ExportStep::Done(report) => report,
    };
    // The writer lock on `.partial` is held from its creation until publication
    // has returned.
    let Some(shared) = held.borrow_mut().take() else {
        let mut l = failure("export", Outcome::Io, "io_error", None);
        l.put("published", false).put("partial_left", created.get());
        return run_of(l, Outcome::Io);
    };
    // Verified and durable: only now does the target name appear.
    let published = match publish_no_replace(&partial, out, &shared.borrow()) {
        Ok(p) => p,
        Err(e) => {
            let reason = match &e {
                PublishError::TargetExists => "target_exists",
                PublishError::PartialChanged => "partial_changed",
                PublishError::TargetUnverified(_) => "target_unverified",
                PublishError::Io(_) => "publish_failed",
            };
            let target_left = matches!(e, PublishError::TargetUnverified(_));
            let mut l = failure("export", Outcome::Io, reason, None);
            l.put("published", false)
                .put("partial_left", true)
                .put("target_left", target_left);
            return run_of(l, Outcome::Io);
        }
    };
    let mut l = Lines::new("export");
    l.put("outcome", "ok")
        .put("reason", "ok")
        .put(
            "selection",
            match report.selection {
                StateSelection::Head => "head".to_string(),
                StateSelection::Snapshot(id) => format!("snapshot:{id}"),
            },
        )
        .put("logical_size", report.logical_size)
        .put("blocks", report.blocks)
        .put("zero_blocks", report.zero_blocks)
        .put("source_uid", report.source_uid)
        .put("source_generation", report.source_generation)
        .put("state_generation", report.state_generation)
        .put("verified", true)
        .put("file_durable", report.file_durable)
        .put("published", true)
        .put("namespace_durable", false)
        .put("partial_removed", published.partial_removed);
    run_of(l, Outcome::Ok)
}

// ------------------------------------------------------------------ import

/// Imports the RAW file `src` into the new base image `new_image`. See the
/// crate docs ("`import`").
pub fn import_file(src: &Path, new_image: &Path) -> Run {
    // `target_left` is printed on every path: whether this run created
    // `<new.img>` (a failure after that leaves the file in place).
    let refused = |reason: &str| {
        let mut l = failure("import", Outcome::Io, reason, None);
        l.put("target_left", false);
        run_of(l, Outcome::Io)
    };
    match name_taken(new_image) {
        Ok(false) => {}
        Ok(true) => return refused("target_exists"),
        Err(e) => return refused(io_reason(&e)),
    }
    let uid = fresh_uid();
    let created = std::cell::Cell::new(false);
    let result = export_with_lease(src, |lease| {
        let source =
            CheckedStorage::new(LeaseRef(lease)).map_err(|_| io::Error::other("capabilities"))?;
        // Called by the library only after the RAW size was accepted. A lost
        // `flock` race leaves the just-created empty file behind (see
        // `create_new_exclusive`), so it counts as created; an `Io` error after
        // the open cannot be told apart from a failed open and is reported as
        // not created.
        let make_target = || {
            let file =
                HostFileStorage::create_new_exclusive(new_image, 0).map_err(|e| match e {
                    WriterOpenError::Io(e) => e,
                    WriterOpenError::WriterBusy => {
                        created.set(true);
                        io::Error::from(io::ErrorKind::WouldBlock)
                    }
                    WriterOpenError::IdentityMismatch => io::Error::from(io::ErrorKind::Other),
                })?;
            created.set(true);
            CheckedStorage::new(file).map_err(|_| io::Error::other("capabilities"))
        };
        // The new image (if any) is released here with its descriptor and lock.
        Ok::<_, io::Error>(import_raw(&source, make_target, uid, &|| false).map(|(_, r)| r))
    });
    let report = match result {
        Err(e) => {
            let (mut l, outcome) = policy_failure("import", &e);
            l.put("target_left", false);
            return run_of(l, outcome);
        }
        Ok(Err(e)) => {
            let mut l = failure("import", Outcome::Io, io_reason(&e), None);
            l.put("target_left", created.get());
            return run_of(l, Outcome::Io);
        }
        Ok(Ok(Err(e))) => {
            let (outcome, reason, detail) = import_failure(&e);
            let mut l = failure("import", outcome, reason, detail);
            l.put("target_left", created.get());
            return run_of(l, outcome);
        }
        Ok(Ok(Ok(r))) => r,
    };
    // `import_raw` refuses a target without file durability before writing,
    // so a published image was made durable; the directory entry was not.
    let mut l = Lines::new("import");
    l.put("outcome", Outcome::Ok.as_str())
        .put("reason", "ok")
        .put("verified", true)
        .put("logical_size", report.logical_size)
        .put("blocks", report.blocks)
        .put("file_uid", report.file_uid)
        .put("file_durable", true)
        .put("namespace_durable", false)
        .put("target_left", true);
    run_of(l, Outcome::Ok)
}

/// Outcome, reason and detail of a failed import. Whether `<new.img>` exists
/// is tracked by the caller (`target_left`), not derived from the error.
fn import_failure(e: &ImportError) -> (Outcome, &'static str, Option<u64>) {
    use od_storage::cow_proto::CowError;
    use od_storage::cow_snapshot::SnapError;
    use od_storage::io::StorageError;
    match e {
        ImportError::NotBlockAligned { len } => {
            (Outcome::Unsupported, "not_block_aligned", Some(*len))
        }
        ImportError::TooLarge { blocks, .. } => (Outcome::Unsupported, "too_large", Some(*blocks)),
        ImportError::Raw(_) => (Outcome::Io, "source_read_error", None),
        ImportError::TargetCreate(io::ErrorKind::AlreadyExists) => {
            (Outcome::Io, "target_exists", None)
        }
        ImportError::TargetCreate(_) => (Outcome::Io, "target_create_failed", None),
        ImportError::TargetNotEmpty { .. } => (Outcome::Io, "target_not_empty", None),
        ImportError::TargetUnsuitable => (Outcome::Io, "target_unsuitable", None),
        ImportError::DurabilityUnavailable => (Outcome::Io, "durability_unavailable", None),
        ImportError::VerifyMismatch { offset } => (Outcome::Io, "verify_mismatch", Some(*offset)),
        ImportError::Cancelled => (Outcome::Io, "cancelled", None),
        ImportError::Image(SnapError::Cow(CowError::Storage(StorageError::Io {
            kind: io::ErrorKind::StorageFull,
            ..
        }))) => (Outcome::Io, "no_space", None),
        ImportError::Image(_) => (Outcome::Io, "target_io_error", None),
    }
}

// ----------------------------------------------------------------- salvage

/// Salvages `src` into the new image `new_image` and writes the report to the
/// new file `report`. See the crate docs ("`export` and `salvage`").
pub fn salvage_file(
    src: &Path,
    new_image: &Path,
    report: &Path,
    selection: StateSelection,
    parents: &[(u64, PathBuf)],
    accept_partial: bool,
) -> Run {
    for (path, reason) in [(new_image, "target_exists"), (report, "report_exists")] {
        match name_taken(path) {
            Ok(false) => {}
            Ok(true) => {
                let mut l = failure("salvage", Outcome::Io, reason, None);
                l.put("copy", "not_created:target_exists");
                return run_of(l, Outcome::Io);
            }
            Err(e) => {
                return run_of(
                    failure("salvage", Outcome::Io, io_reason(&e), None),
                    Outcome::Io,
                );
            }
        }
    }
    let mut resolver = PathResolver {
        paths: parents.iter().cloned().collect(),
    };
    let options = SalvageOptions {
        selection,
        publish_partial: accept_partial,
    };
    let result = export_with_lease(src, |lease| {
        let source =
            CheckedStorage::new(LeaseRef(lease)).map_err(|_| io::Error::other("capabilities"))?;
        let outcome = salvage_to_copy(source, &mut resolver, &options, fresh_uid(), || {
            let file =
                HostFileStorage::create_new_exclusive(new_image, 0).map_err(|e| match e {
                    WriterOpenError::Io(e) => e,
                    WriterOpenError::WriterBusy => io::Error::from(io::ErrorKind::WouldBlock),
                    WriterOpenError::IdentityMismatch => io::Error::from(io::ErrorKind::Other),
                })?;
            CheckedStorage::new(file).map_err(|_| io::Error::other("capabilities"))
        });
        // The copy (if any) is released here; its file descriptor and lock go
        // with it. Only the report leaves the lease.
        Ok::<_, io::Error>(outcome.map(|o| o.report))
    });
    let analysed = match result {
        Err(e) => {
            let (l, outcome) = policy_failure("salvage", &e);
            return run_of(l, outcome);
        }
        Ok(Err(e)) => {
            return run_of(
                failure("salvage", Outcome::Io, io_reason(&e), None),
                Outcome::Io,
            );
        }
        Ok(Ok(Err(SalvageError::Source(_)))) => {
            return run_of(
                failure("salvage", Outcome::Io, "source_read_error", None),
                Outcome::Io,
            );
        }
        Ok(Ok(Err(SalvageError::BadUid))) => {
            return run_of(
                failure("salvage", Outcome::Io, "internal_uid", None),
                Outcome::Io,
            );
        }
        Ok(Ok(Ok(r))) => r,
    };
    // The full report goes to its own new file; the console gets a summary.
    let report_written = write_new_file(report, analysed.render(REPORT_FILE_EXTENTS).as_bytes());
    let mut l = Lines::new("salvage");
    let copy_failed = matches!(analysed.copy, CopyStatus::Failed(_));
    let outcome = if copy_failed || report_written.is_err() {
        Outcome::Io
    } else {
        analysed.outcome()
    };
    l.put("outcome", outcome.as_str());
    let mut text = l.0;
    text.push_str(&analysed.render(CONSOLE_EXTENTS));
    let mut l = Lines(text);
    l.put(
        "report_file",
        if report_written.is_ok() {
            "written"
        } else {
            "not_written"
        },
    );
    Run {
        exit: exit_for(outcome),
        stdout: l.finish(),
        stderr: String::new(),
    }
}

/// Extents listed on the console and in the report file.
const CONSOLE_EXTENTS: usize = 8;
const REPORT_FILE_EXTENTS: usize = 1024;

/// Creates `path` (it must not exist), writes `bytes` and syncs the file.
fn write_new_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}
