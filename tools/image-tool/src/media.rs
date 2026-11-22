//! `make-data-disk` and `add-install-file`: new GPT/FAT32 raw disks built by
//! the writer of `od_storage::install_media`. Both write a new file only: an
//! existing output name is refused before any input is read, and an input is
//! never written.

use super::{Lines, Run, failure, io_reason, name_taken, run_of, with_suffix};
use od_host::storage_io::{HostFileStorage, LeaseOpenError, ReadLease, WriterOpenError};
use od_host::storage_policy::{PublishError, publish_no_replace};
use od_storage::cow_inspect::Outcome;
use od_storage::install_media::{AddedFile, ImportError, InstallMedia};
use od_storage::io::{
    BackendError, CheckedStorage, StorageBackend, StorageCapabilities, StorageError,
};
use sha2::{Digest, Sha256};
use std::io;
use std::path::{Path, PathBuf};

pub(super) const PAYLOAD_LABEL: &str = "OD8PAYLOAD";
const PAYLOAD_DIRECTORY: &str = "OD8";
const DEFAULT_SPARE_MIB: u64 = 512;
/// The largest `spare_mib` that any payload can still be planned with. Above
/// 64 MiB of volume the writer uses 4 KiB clusters; the reader accepts a FAT
/// of 64 MiB (`MAX_FAT_BYTES` of `od_storage::install_media`), that is
/// 16_777_214 clusters, and the volume needs 16 clusters of slack plus the
/// root directory and `OD8`. So 65535 MiB (16_776_960 clusters) is the most
/// that fits next to an almost empty payload and 65536 MiB (16_777_216
/// clusters) never fits. A big payload lowers the real limit; the writer
/// refuses that with `fat_capacity` before anything is created.
const MAX_SPARE_MIB: u64 = 65535;
/// The deepest directory the reader of `od_storage::install_media` accepts
/// (its `MAX_DEPTH`; the root directory is 0, `OD8` is 1). The constant is
/// private to that module, so it is repeated here; the test
/// `directory_depth_at_the_reader_limit_reads_back_and_one_more_is_refused`
/// builds a disk at exactly this depth and reads it back, which fails if the
/// two values drift apart.
const MAX_DIRECTORY_DEPTH: usize = 16;
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
/// Every input stays open (and share-locked) from hashing until publication.
const MAX_INPUT_FILES: usize = 128;
const HASH_CHUNK: usize = 1024 * 1024;

type Digest32 = [u8; 32];

// ------------------------------------------------------------------ shared

/// Why a command stopped; rendered by [`Refusal::run`].
pub(super) struct Refusal {
    pub(super) outcome: Outcome,
    pub(super) reason: &'static str,
    pub(super) detail: Option<u64>,
    pub(super) partial_left: bool,
    pub(super) target_left: bool,
    extra: Vec<(&'static str, String)>,
}

impl Refusal {
    pub(super) fn new(outcome: Outcome, reason: &'static str) -> Self {
        Self {
            outcome,
            reason,
            detail: None,
            partial_left: false,
            target_left: false,
            extra: Vec::new(),
        }
    }

    fn io() -> Self {
        Self::new(Outcome::Io, "io_error")
    }

    fn detail(mut self, detail: u64) -> Self {
        self.detail = Some(detail);
        self
    }

    fn partial(mut self) -> Self {
        self.partial_left = true;
        self
    }

    fn with(mut self, key: &'static str, value: String) -> Self {
        self.extra.push((key, value));
        self
    }

    pub(super) fn run(self, command: &str) -> Run {
        let mut lines = failure(command, self.outcome, self.reason, self.detail);
        for (key, value) in &self.extra {
            lines.put(key, value);
        }
        lines
            .put("published", false)
            .put("partial_left", self.partial_left)
            .put("target_left", self.target_left);
        run_of(lines, self.outcome)
    }
}

pub(super) fn plan_refusal(error: &ImportError) -> Refusal {
    match error {
        ImportError::Corrupt(reason) => Refusal::new(Outcome::Corrupt, reason),
        ImportError::Unsupported(reason) => Refusal::new(Outcome::Unsupported, reason),
        ImportError::Output(reason) => Refusal::new(Outcome::Io, reason),
        ImportError::Storage(_) => Refusal::io(),
    }
}

pub(super) fn lease_refusal(error: LeaseOpenError) -> Refusal {
    match error {
        LeaseOpenError::LeaseBusy => Refusal::new(Outcome::Busy, "busy"),
        LeaseOpenError::IdentityMismatch => Refusal::new(Outcome::Io, "path_changed"),
        LeaseOpenError::Io(error) => Refusal::new(Outcome::Io, io_reason(&error)),
    }
}

/// The names that a new output would use must both be free.
pub(super) fn refuse_existing(target: &Path, partial: &Path) -> Result<(), Refusal> {
    for (path, reason, partial_left) in [
        (target, "target_exists", false),
        (partial, "partial_exists", true),
    ] {
        match name_taken(path) {
            Ok(false) => {}
            Ok(true) => {
                let mut refusal = Refusal::new(Outcome::Io, reason);
                refusal.partial_left = partial_left;
                return Err(refusal);
            }
            Err(error) => return Err(Refusal::new(Outcome::Io, io_reason(&error))),
        }
    }
    Ok(())
}

pub(super) struct Written {
    pub(super) partial_removed: bool,
    pub(super) output_sha256: Option<Digest32>,
}

/// Writes `plan` to `<target>.partial` (created exclusively), reads it back
/// against a second emission, runs `before_publish`, and gives the file the
/// name `target` without replacing anything.
pub(super) fn write_and_publish<S: StorageBackend>(
    plan: &InstallMedia,
    source: &CheckedStorage<S>,
    target: &Path,
    identity: u64,
    hash_output: bool,
    before_publish: impl FnOnce() -> Result<(), Refusal>,
) -> Result<Written, Refusal> {
    let partial = with_suffix(target, ".partial");
    let writer = match HostFileStorage::create_new_exclusive(&partial, 0) {
        Ok(writer) => writer,
        Err(error) => {
            let reason = match &error {
                WriterOpenError::WriterBusy => "busy",
                WriterOpenError::IdentityMismatch => "path_changed",
                WriterOpenError::Io(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    "partial_exists"
                }
                WriterOpenError::Io(e) => io_reason(e),
            };
            let mut refusal = Refusal::new(Outcome::Io, reason);
            refusal.partial_left = name_taken(&partial).unwrap_or(false);
            return Err(refusal);
        }
    };
    let Ok(mut destination) = CheckedStorage::new(writer) else {
        return Err(Refusal::io().partial());
    };
    if let Err(error) = plan.write_and_verify(source, &mut destination, identity) {
        let reason = match error {
            ImportError::Output(reason) => reason,
            _ => "io_error",
        };
        return Err(Refusal::new(Outcome::Io, reason).partial());
    }
    let Ok(writer) = destination.into_backend() else {
        return Err(Refusal::io().partial());
    };
    before_publish().map_err(Refusal::partial)?;
    let output_sha256 = if hash_output {
        match hash_backend(&writer) {
            Ok(digest) => Some(digest),
            Err(_) => return Err(Refusal::io().partial()),
        }
    } else {
        None
    };
    match publish_no_replace(&partial, target, &writer) {
        Ok(published) => Ok(Written {
            partial_removed: published.partial_removed,
            output_sha256,
        }),
        Err(error) => {
            let reason = match error {
                PublishError::TargetExists => "target_exists",
                PublishError::PartialChanged => "partial_changed",
                PublishError::TargetUnverified(_) => "target_unverified",
                PublishError::Io(_) => "publish_failed",
            };
            let mut refusal = Refusal::new(Outcome::Io, reason).partial();
            refusal.target_left = reason == "target_unverified";
            Err(refusal)
        }
    }
}

fn hex(digest: &Digest32) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn parse_hex(text: &str) -> Option<Digest32> {
    if text.len() != 64 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut digest = [0; 32];
    for (byte, pair) in digest.iter_mut().zip(text.as_bytes().chunks_exact(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(digest)
}

fn hash_backend<S: StorageBackend>(backend: &S) -> Result<Digest32, BackendError> {
    let size = backend.size()?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; HASH_CHUNK];
    let mut offset = 0;
    while offset < size {
        let want = buffer.len().min((size - offset) as usize);
        let got = backend.read_at(offset, &mut buffer[..want])?;
        if got == 0 {
            return Err(BackendError::Unsupported);
        }
        hasher.update(&buffer[..got]);
        offset += got as u64;
    }
    Ok(hasher.finalize().into())
}

/// SHA-256 of `length` bytes at `offset` of the checked source.
fn hash_range<B: StorageBackend>(
    source: &CheckedStorage<B>,
    offset: u64,
    length: u64,
) -> Result<Digest32, StorageError> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; HASH_CHUNK];
    let mut done = 0;
    while done < length {
        let count = buffer.len().min((length - done) as usize);
        source.read_exact_at(offset + done, &mut buffer[..count])?;
        hasher.update(&buffer[..count]);
        done += count as u64;
    }
    Ok(hasher.finalize().into())
}

enum Segment {
    File(ReadLease),
    Bytes(Vec<u8>),
}

/// Read-only concatenation of share-locked files and small in-memory pieces:
/// the one source object whose ranges the FAT writer copies from.
struct ConcatSource {
    parts: Vec<(u64, u64, Segment)>,
    size: u64,
}

impl ConcatSource {
    fn new() -> Self {
        Self {
            parts: Vec::new(),
            size: 0,
        }
    }

    /// Appends a part and returns its offset.
    fn push(&mut self, length: u64, segment: Segment) -> u64 {
        let offset = self.size;
        self.size += length;
        if length != 0 {
            self.parts.push((offset, length, segment));
        }
        offset
    }
}

impl StorageBackend for ConcatSource {
    fn capabilities(&self) -> StorageCapabilities {
        StorageCapabilities::read_only(u64::MAX >> 1)
    }

    fn size(&self) -> Result<u64, BackendError> {
        Ok(self.size)
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, BackendError> {
        let index = self.parts.partition_point(|(start, _, _)| *start <= offset);
        let Some((start, length, segment)) = index.checked_sub(1).map(|i| &self.parts[i]) else {
            return Ok(0);
        };
        let inside = offset - start;
        if inside >= *length {
            return Ok(0);
        }
        let count = buf.len().min((length - inside) as usize);
        match segment {
            Segment::File(lease) => lease.read_at(inside, &mut buf[..count]),
            Segment::Bytes(bytes) => {
                buf[..count].copy_from_slice(&bytes[inside as usize..inside as usize + count]);
                Ok(count)
            }
        }
    }

    fn write_at(&mut self, _offset: u64, _buf: &[u8]) -> Result<usize, BackendError> {
        Err(BackendError::Unsupported)
    }
}

fn open_input(path: &Path) -> Result<(ReadLease, u64), Refusal> {
    let lease = ReadLease::open(path).map_err(lease_refusal)?;
    let length = lease.size().map_err(|_| Refusal::io())?;
    Ok((lease, length))
}

// ------------------------------------------------------------ make-data-disk

struct ManifestFile {
    path: String,
    source: PathBuf,
    sha256: Digest32,
}

struct Manifest {
    spare_bytes: u64,
    files: Vec<ManifestFile>,
}

/// Fields of the manifest (see `tests/compat/payloads/`). Anything else is
/// refused so that a misspelt key cannot silently change the result.
fn parse_manifest(bytes: &[u8], base: &Path) -> Result<Manifest, (&'static str, Option<u64>)> {
    use serde_json::Value;
    let value: Value = serde_json::from_slice(bytes).map_err(|_| ("manifest_json", None))?;
    let top = value.as_object().ok_or(("manifest_field", None))?;
    if top
        .keys()
        .any(|key| !["version", "comment", "spare_mib", "files"].contains(&key.as_str()))
        || top.get("version").and_then(Value::as_u64) != Some(1)
        || top.get("comment").is_some_and(|value| !value.is_string())
    {
        return Err(("manifest_field", None));
    }
    let spare_mib = match top.get("spare_mib") {
        None => DEFAULT_SPARE_MIB,
        Some(value) => value
            .as_u64()
            .filter(|mib| (1..=MAX_SPARE_MIB).contains(mib))
            .ok_or(("manifest_spare", None))?,
    };
    let rows = top
        .get("files")
        .and_then(Value::as_array)
        .ok_or(("manifest_field", None))?;
    if rows.len() > MAX_INPUT_FILES {
        return Err(("manifest_too_many_files", None));
    }
    let mut files = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let at = Some(index as u64);
        let row = row.as_object().ok_or(("manifest_field", at))?;
        if row
            .keys()
            .any(|key| !["path", "source", "sha256", "origin"].contains(&key.as_str()))
            || row.get("origin").is_some_and(|value| !value.is_string())
        {
            return Err(("manifest_field", at));
        }
        let text = |key: &str| row.get(key).and_then(Value::as_str);
        let path = text("path")
            .ok_or(("manifest_field", at))?
            .replace('\\', "/");
        let source = text("source")
            .filter(|source| !source.is_empty())
            .ok_or(("manifest_field", at))?;
        let sha256 = text("sha256")
            .and_then(parse_hex)
            .ok_or(("manifest_sha256", at))?;
        let inside = path
            .strip_prefix(PAYLOAD_DIRECTORY)
            .and_then(|rest| rest.strip_prefix('/'));
        if inside.is_none_or(str::is_empty) {
            return Err(("manifest_path", at));
        }
        // Every `/` ends a directory (`OD8` is the first), so the file lies in
        // a directory of that depth; the reader refuses anything deeper.
        if path.matches('/').count() > MAX_DIRECTORY_DEPTH {
            return Err(("manifest_depth", at));
        }
        files.push(ManifestFile {
            path,
            source: base.join(source),
            sha256,
        });
    }
    Ok(Manifest {
        spare_bytes: spare_mib * 1024 * 1024,
        files,
    })
}

/// Exactly 32 lowercase hexadecimal digits (a 128-bit random value).
fn valid_nonce(nonce: &str) -> bool {
    nonce.len() == 32
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn make_data_disk(manifest_path: &Path, target: &Path, nonce: Option<&str>) -> Run {
    const COMMAND: &str = "make-data-disk";
    match build_data_disk(manifest_path, target, nonce) {
        Ok(lines) => run_of(lines, Outcome::Ok),
        Err(refusal) => refusal.run(COMMAND),
    }
}

fn build_data_disk(
    manifest_path: &Path,
    target: &Path,
    nonce: Option<&str>,
) -> Result<Lines, Refusal> {
    let partial = with_suffix(target, ".partial");
    refuse_existing(target, &partial)?;
    if nonce.is_some_and(|nonce| !valid_nonce(nonce)) {
        return Err(Refusal::new(Outcome::Unsupported, "nonce_invalid"));
    }

    let manifest_bytes = read_manifest(manifest_path)?;
    let base = manifest_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let manifest = parse_manifest(&manifest_bytes, base).map_err(|(reason, at)| {
        let refusal = Refusal::new(Outcome::Unsupported, reason);
        match at {
            Some(at) => refusal.detail(at),
            None => refusal,
        }
    })?;

    // The tool's own files come first; the manifest cannot replace them.
    let nonce_bytes = nonce.unwrap_or("").as_bytes().to_vec();
    let mut source = ConcatSource::new();
    let mut added = vec![
        (
            format!("{PAYLOAD_DIRECTORY}/nonce.txt"),
            source.push(
                nonce_bytes.len() as u64,
                Segment::Bytes(nonce_bytes.clone()),
            ),
            nonce_bytes.len() as u64,
        ),
        (format!("{PAYLOAD_DIRECTORY}/markers.txt"), source.size, 0),
    ];
    let mut expected = Vec::new();
    for (index, file) in manifest.files.iter().enumerate() {
        let (lease, length) =
            open_input(&file.source).map_err(|refusal| refusal.detail(index as u64))?;
        let offset = source.push(length, Segment::File(lease));
        added.push((file.path.clone(), offset, length));
        expected.push((offset, length, file.sha256));
    }
    let source = CheckedStorage::new(source).map_err(|_| Refusal::io())?;

    // Nothing is created until every input matches its recorded hash.
    verify_inputs(&source, &expected)?;

    let files: Vec<AddedFile<'_>> = added
        .iter()
        .map(|(path, source_offset, length)| AddedFile {
            path,
            source_offset: *source_offset,
            length: *length,
        })
        .collect();
    let plan = InstallMedia::payload_disk(&source, PAYLOAD_LABEL, manifest.spare_bytes, &files)
        .map_err(|error| plan_refusal(&error))?;
    let written = write_and_publish(&plan, &source, target, super::fresh_uid(), true, || {
        verify_inputs(&source, &expected)
    })?;

    let mut lines = Lines::new("make-data-disk");
    lines
        .put("outcome", "ok")
        .put("reason", "ok")
        .put("logical_size", plan.disk_bytes())
        .put("files", plan.file_count())
        .put("input_files", manifest.files.len())
        .put("volume_label", PAYLOAD_LABEL)
        .put("partition_type", "ebd0a0a2-b9e5-4433-87c0-68b6b72699c7")
        .put(
            "manifest_sha256",
            hex(&Sha256::digest(&manifest_bytes).into()),
        )
        .put("nonce_present", nonce.is_some())
        .put(
            "output_sha256",
            hex(&written.output_sha256.unwrap_or_default()),
        )
        .put("verified", true)
        .put("file_durable", true)
        .put("published", true)
        .put("namespace_durable", false)
        .put("partial_removed", written.partial_removed);
    Ok(lines)
}

fn read_manifest(path: &Path) -> Result<Vec<u8>, Refusal> {
    let (lease, length) = open_input(path)?;
    if length > MAX_MANIFEST_BYTES {
        return Err(Refusal::new(Outcome::Unsupported, "manifest_size"));
    }
    let mut bytes = vec![0; length as usize];
    let source = CheckedStorage::new(lease).map_err(|_| Refusal::io())?;
    source
        .read_exact_at(0, &mut bytes)
        .map_err(|_| Refusal::io())?;
    Ok(bytes)
}

/// Compares every input with its manifest hash through the checked source.
fn verify_inputs<B: StorageBackend>(
    source: &CheckedStorage<B>,
    expected: &[(u64, u64, Digest32)],
) -> Result<(), Refusal> {
    for (index, (offset, length, digest)) in expected.iter().enumerate() {
        let actual = hash_range(source, *offset, *length).map_err(|_| Refusal::io())?;
        if actual != *digest {
            return Err(Refusal::new(Outcome::Corrupt, "input_hash_mismatch")
                .detail(index as u64)
                .with("expected_sha256", hex(digest))
                .with("actual_sha256", hex(&actual)));
        }
    }
    Ok(())
}

// --------------------------------------------------------- add-install-file

pub(super) fn add_install_file(input: &Path, target: &Path, media_path: &str, file: &Path) -> Run {
    const COMMAND: &str = "add-install-file";
    match build_extended_media(input, target, media_path, file) {
        Ok(lines) => run_of(lines, Outcome::Ok),
        Err(refusal) => refusal.run(COMMAND),
    }
}

fn build_extended_media(
    input: &Path,
    target: &Path,
    media_path: &str,
    file: &Path,
) -> Result<Lines, Refusal> {
    let partial = with_suffix(target, ".partial");
    refuse_existing(target, &partial)?;
    let media_path = media_path.replace('\\', "/");

    let (media, media_length) = open_input(input)?;
    let (added, added_length) = open_input(file).map_err(|refusal| refusal.detail(1))?;
    let mut source = ConcatSource::new();
    source.push(media_length, Segment::File(media));
    let added_offset = source.push(added_length, Segment::File(added));
    let source = CheckedStorage::new(source).map_err(|_| Refusal::io())?;

    let input_sha256 = hash_range(&source, 0, media_length).map_err(|_| Refusal::io())?;
    let file_sha256 = hash_range(&source, added_offset, added_length).map_err(|_| Refusal::io())?;
    let plan = InstallMedia::from_converted_disk(
        &source,
        media_length,
        &[AddedFile {
            path: &media_path,
            source_offset: added_offset,
            length: added_length,
        }],
    )
    .map_err(|error| plan_refusal(&error))?;
    let unchanged = || {
        let input_now = hash_range(&source, 0, media_length).map_err(|_| Refusal::io())?;
        let file_now =
            hash_range(&source, added_offset, added_length).map_err(|_| Refusal::io())?;
        if input_now == input_sha256 && file_now == file_sha256 {
            Ok(())
        } else {
            Err(Refusal::new(Outcome::Corrupt, "input_changed"))
        }
    };
    let written = write_and_publish(&plan, &source, target, super::fresh_uid(), true, unchanged)?;

    let mut lines = Lines::new("add-install-file");
    lines
        .put("outcome", "ok")
        .put("reason", "ok")
        .put("logical_size", plan.disk_bytes())
        .put("files", plan.file_count())
        .put("input_sha256", hex(&input_sha256))
        .put("added_file_sha256", hex(&file_sha256))
        .put("added_file_size", added_length)
        .put(
            "output_sha256",
            hex(&written.output_sha256.unwrap_or_default()),
        )
        .put("input_unchanged", true)
        .put("verified", true)
        .put("file_durable", true)
        .put("published", true)
        .put("namespace_durable", false)
        .put("partial_removed", written.partial_removed);
    Ok(lines)
}
