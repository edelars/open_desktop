//! Shared-folder host service: file operations confined to one directory
//! descriptor (S11-26).
//!
//! The semantic contract (permissions, symlinks, case, Unicode, locking,
//! concurrency, limits, transport) is `docs/specs/SHARED_FOLDERS.md`; this module is its
//! host side. It has no guest-facing wire code and no C ABI: the transport to
//! the guest and the VMHost call boundary belong to later tasks (S11-22,
//! S11-29) and are recorded as open in the contract.
//!
//! # Confinement
//!
//! A [`SharedFolder`] owns exactly one open directory descriptor (the root,
//! received by VMHost through `ATTACH_RESOURCE` with the role
//! `shared_folder_root`). Every operation resolves a guest path **component by
//! component starting from that descriptor** with `openat`; `..`, `.`,
//! empty components and absolute paths are refused before any system call, so
//! the walk can only descend. Each component is opened with `O_NOFOLLOW`
//! (intermediate directories also with `O_DIRECTORY`); a symbolic link at any
//! position, including the last one, is refused and never followed, even when
//! it points inside the root. The final entry is operated on with the
//! `*at` family relative to its parent descriptor, so no host path string is ever
//! built or passed to the kernel.
//!
//! Sources (macOS, `man 2 open`, `man 2 rename`, SDK headers; numeric flag, `errno`,
//! `dirent` layout and `_PC_*` values verified with a C probe on macOS arm64):
//! `O_NOFOLLOW` "do not follow symlinks" (`ELOOP` at the last component),
//! `O_SYMLINK` "allow open of symlinks" (used to inspect an entry without following
//! it), `O_DIRECTORY`, `O_NONBLOCK` (a FIFO planted in the folder cannot block
//! the open), `renameatx_np` with `RENAME_EXCL` (fails with `EEXIST` instead of
//! replacing), `fpathconf` `_PC_CASE_SENSITIVE`/`_PC_CASE_PRESERVING`
//! (`<unistd.h>`). `O_NOFOLLOW_ANY` and `O_RESOLVE_BENEATH` exist in newer macOS
//! releases but are not used: the component walk does not depend on them.

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char};
use std::fmt;
use std::fs::{File, Metadata};
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd};
use std::os::unix::fs::{FileExt, MetadataExt};

#[cfg(test)]
mod tests;

/// Longest single name, in UTF-8 bytes (macOS `NAME_MAX`).
pub const MAX_NAME_BYTES: usize = 255;
/// Longest guest path, in UTF-8 bytes (macOS `PATH_MAX`).
pub const MAX_PATH_BYTES: usize = 1024;
/// Most components in one path (decision of the contract, not a measurement).
pub const MAX_PATH_DEPTH: usize = 32;
/// Largest read or write in one call, in bytes; leaves room for the frame
/// header in the agent protocol's 61 440-byte payload (decision of the
/// contract, not a measurement).
pub const MAX_IO_BYTES: usize = 32 * 1024;
/// Most files open at once in one service.
pub const MAX_OPEN_FILES: usize = 64;
/// Most entries one directory listing examines.
pub const MAX_DIR_ENTRIES: usize = 65_536;
/// Most entries in one `readdir` page.
pub const MAX_READDIR_PAGE: usize = 64;

// macOS SDK values (`<sys/fcntl.h>`, `<sys/errno.h>`, `<unistd.h>`, `<sys/stdio.h>`,
// `<dirent.h>`), verified with a C probe on macOS 26 arm64.
const O_RDONLY: i32 = 0x0000;
const O_RDWR: i32 = 0x0002;
const O_NONBLOCK: i32 = 0x0004;
const O_NOFOLLOW: i32 = 0x0100;
const O_CREAT: i32 = 0x0200;
const O_EXCL: i32 = 0x0800;
const O_DIRECTORY: i32 = 0x0010_0000;
const O_SYMLINK: i32 = 0x0020_0000;
const O_CLOEXEC: i32 = 0x0100_0000;
const F_GETFL: i32 = 3;
const F_SETFD: i32 = 2;
const O_ACCMODE: i32 = 0x0003;
const FD_CLOEXEC: i32 = 1;
const RENAME_EXCL: u32 = 0x4;
const PC_CASE_SENSITIVE: i32 = 11;
const PC_CASE_PRESERVING: i32 = 12;
const DT_DIR: u8 = 4;
const DT_REG: u8 = 8;
const DT_LNK: u8 = 10;
const EPERM: i32 = 1;
const ENOENT: i32 = 2;
const ENXIO: i32 = 6;
const EACCES: i32 = 13;
const EEXIST: i32 = 17;
const ENOTDIR: i32 = 20;
const EISDIR: i32 = 21;
const EINVAL: i32 = 22;
const EFBIG: i32 = 27;
const ENOSPC: i32 = 28;
const EROFS: i32 = 30;
const ELOOP: i32 = 62;
const ENAMETOOLONG: i32 = 63;
const EDQUOT: i32 = 69;
/// Mode of a created file before the process umask (host user is the owner).
const CREATE_MODE: u32 = 0o644;

/// `struct dirent` of macOS (64-bit inodes); offsets verified with a C probe:
/// `d_ino` 0, `d_seekoff` 8, `d_reclen` 16, `d_namlen` 18, `d_type` 20,
/// `d_name` 21, size 1048.
#[repr(C)]
#[allow(dead_code)] // fields exist to give the C layout; only the last three are read
struct Dirent {
    d_ino: u64,
    d_seekoff: u64,
    d_reclen: u16,
    d_namlen: u16,
    d_type: u8,
    d_name: [u8; 1024],
}

/// Offset of `d_name` in `Dirent` (verified with the C probe) and the capacity
/// of that field.
const D_NAME_OFFSET: usize = 21;
const DIRENT_NAME_CAPACITY: usize = 1024;

/// Opaque `DIR`.
#[repr(C)]
struct Dir {
    _private: [u8; 0],
}

unsafe extern "C" {
    // Variadic in C (`int openat(int, const char *, int, ...)`); the optional
    // mode is passed as `c_uint`. SAFETY contract: `fd` is an open directory
    // descriptor and `path` a NUL-terminated string that outlives the call.
    fn openat(fd: i32, path: *const c_char, oflag: i32, ...) -> i32;
    // SAFETY contract: as for `openat`; `flag` is 0 (a non-directory).
    fn unlinkat(fd: i32, path: *const c_char, flag: i32) -> i32;
    // SAFETY contract: both descriptors open directories, both names
    // NUL-terminated strings that outlive the call.
    fn renameatx_np(
        from_fd: i32,
        from: *const c_char,
        to_fd: i32,
        to: *const c_char,
        flags: u32,
    ) -> i32;
    // Variadic in C; on Apple arm64 variadic arguments go on the stack, so it
    // must be declared variadic. SAFETY contract: `fd` is open.
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    // SAFETY contract: `fd` is open; reads no caller memory.
    fn fpathconf(fd: i32, name: i32) -> isize;
    // SAFETY contract: `fd` is an open directory descriptor; on success the
    // returned stream owns it, on failure the caller still owns it.
    fn fdopendir(fd: i32) -> *mut Dir;
    // SAFETY contract: `dir` is a live stream; the returned entry is valid until
    // the next `readdir`/`closedir` on the same stream.
    fn readdir(dir: *mut Dir) -> *mut Dirent;
    // SAFETY contract: `dir` is a live stream; closes its descriptor.
    fn closedir(dir: *mut Dir) -> i32;
    // Thread-local `errno` location on macOS.
    fn __error() -> *mut i32;
}

/// Closed list of results of a refused or failed operation. Not a wire
/// encoding: numeric codes are assigned together with the guest transport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SharedFolderError {
    /// Malformed or forbidden path: absolute, `.`/`..`, empty component,
    /// `\`, `:`, NUL or control character, invalid UTF-8, too deep.
    InvalidPath,
    /// A name longer than [`MAX_NAME_BYTES`] (also reported when the host refuses a
    /// name as too long).
    NameTooLong,
    /// A path longer than [`MAX_PATH_BYTES`].
    PathTooLong,
    NotFound,
    AlreadyExists,
    NotADirectory,
    IsADirectory,
    /// A symbolic link was met; links are never followed (contract section 5).
    SymlinkRefused,
    /// The entry is neither a regular file, a directory nor a symlink (FIFO,
    /// socket, device), or a file operation was asked of such an entry.
    UnsupportedKind,
    /// The folder is read-only (policy or host volume).
    ReadOnly,
    /// The host refused for lack of permission.
    PermissionDenied,
    /// Out of space, quota or file-size limit on the host.
    NoSpace,
    /// A length, offset or page size outside the limits.
    TooLarge,
    /// The handle is not open in this service.
    BadHandle,
    TooManyOpenFiles,
    /// A directory holds more than [`MAX_DIR_ENTRIES`] entries.
    TooManyEntries,
    /// Any other host error, with its macOS `errno`.
    Io(i32),
}

impl fmt::Display for SharedFolderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "shared folder: {self:?}")
    }
}

impl std::error::Error for SharedFolderError {}

type Result<T> = std::result::Result<T, SharedFolderError>;

fn from_io(error: io::Error) -> SharedFolderError {
    match error.raw_os_error() {
        Some(ENOENT) => SharedFolderError::NotFound,
        Some(EEXIST) => SharedFolderError::AlreadyExists,
        Some(ENOTDIR) => SharedFolderError::NotADirectory,
        Some(EISDIR) => SharedFolderError::IsADirectory,
        Some(ELOOP) => SharedFolderError::SymlinkRefused,
        Some(EACCES | EPERM) => SharedFolderError::PermissionDenied,
        Some(EROFS) => SharedFolderError::ReadOnly,
        Some(ENAMETOOLONG) => SharedFolderError::NameTooLong,
        Some(ENOSPC | EDQUOT | EFBIG) => SharedFolderError::NoSpace,
        Some(EINVAL) => SharedFolderError::InvalidPath,
        Some(ENXIO) => SharedFolderError::UnsupportedKind,
        Some(code) => SharedFolderError::Io(code),
        None => SharedFolderError::Io(0),
    }
}

/// Kind of a directory entry; a symlink is reported as such, never resolved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

impl EntryKind {
    fn of(metadata: &Metadata) -> Self {
        let kind = metadata.file_type();
        if kind.is_file() {
            Self::File
        } else if kind.is_dir() {
            Self::Directory
        } else if kind.is_symlink() {
            Self::Symlink
        } else {
            Self::Other
        }
    }
}

/// Result of `stat` (the entry itself, links not followed).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileStat {
    pub kind: EntryKind,
    /// Size in bytes; meaningful for regular files.
    pub size: u64,
    /// Modification time, nanoseconds since the Unix epoch (saturating).
    pub modified_ns: i64,
}

/// One listed name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirEntry {
    pub name: String,
    pub kind: EntryKind,
}

/// One page of a directory listing, ordered by the bytes of the name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirPage {
    pub entries: Vec<DirEntry>,
    /// Cookie for the next page; `None` when the listing is complete.
    pub next: Option<u32>,
    /// Names of the whole directory that were left out because they cannot be
    /// represented to the guest (invalid UTF-8 or a forbidden character).
    pub skipped: u32,
}

/// How `open` treats the file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenMode {
    /// Existing regular file, read only.
    Read,
    /// Existing regular file, read and write, content kept.
    Write,
    /// Existing regular file, read and write, truncated to zero.
    Truncate,
    /// New regular file, read and write; fails if the name exists.
    CreateNew,
}

/// Handle of a file open in one service; never reused while open.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FileHandle(pub u32);

/// Properties of the attached folder, for the guest driver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FolderInfo {
    /// Guest writes are refused (policy flag of the folder).
    pub read_only: bool,
    /// The host volume distinguishes names that differ only in case.
    pub case_sensitive: bool,
    /// The host volume keeps the case a name was created with.
    pub case_preserving: bool,
}

struct OpenFile {
    file: File,
    writable: bool,
}

/// One shared folder: a root directory descriptor plus the open files.
pub struct SharedFolder {
    root: OwnedFd,
    info: FolderInfo,
    files: HashMap<u32, OpenFile>,
    next_handle: u32,
}

/// A directory descriptor on the way down, or the root itself.
fn dir_fd<'a>(current: &'a Option<OwnedFd>, root: &'a OwnedFd) -> BorrowedFd<'a> {
    match current {
        Some(fd) => fd.as_fd(),
        None => root.as_fd(),
    }
}

fn cstring(name: &str) -> Result<CString> {
    CString::new(name).map_err(|_| SharedFolderError::InvalidPath)
}

fn openat_fd(dir: BorrowedFd<'_>, name: &CStr, flags: i32, mode: u32) -> io::Result<OwnedFd> {
    loop {
        // SAFETY: `dir` is live for the call and `name` NUL-terminated; on
        // success the new descriptor has no other owner and moves into the
        // returned `OwnedFd`.
        let fd = unsafe { openat(dir.as_raw_fd(), name.as_ptr(), flags, mode) };
        if fd >= 0 {
            // SAFETY: just returned by `openat`, not shared.
            return Ok(unsafe { OwnedFd::from_raw_fd(fd) });
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// Opens the entry itself without following a symlink and without blocking
/// (a FIFO opens immediately); the caller inspects the descriptor.
fn open_entry(dir: BorrowedFd<'_>, name: &CStr) -> io::Result<File> {
    openat_fd(dir, name, O_RDONLY | O_SYMLINK | O_NONBLOCK | O_CLOEXEC, 0).map(File::from)
}

/// Opens a sub-directory without following links. A symlink is reported as
/// [`SharedFolderError::SymlinkRefused`], any other non-directory as
/// [`SharedFolderError::NotADirectory`].
fn open_dir(dir: BorrowedFd<'_>, name: &CStr) -> Result<OwnedFd> {
    match openat_fd(
        dir,
        name,
        O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC,
        0,
    ) {
        Ok(fd) => Ok(fd),
        Err(error) if matches!(error.raw_os_error(), Some(ELOOP | ENOTDIR)) => {
            let metadata = open_entry(dir, name)
                .and_then(|file| file.metadata())
                .map_err(from_io)?;
            if metadata.file_type().is_symlink() {
                Err(SharedFolderError::SymlinkRefused)
            } else {
                Err(SharedFolderError::NotADirectory)
            }
        }
        Err(error) => Err(from_io(error)),
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." {
        return Err(SharedFolderError::InvalidPath);
    }
    if name.len() > MAX_NAME_BYTES {
        return Err(SharedFolderError::NameTooLong);
    }
    if name
        .chars()
        .any(|c| c == '\\' || c == ':' || c == '/' || c.is_ascii_control())
    {
        return Err(SharedFolderError::InvalidPath);
    }
    Ok(())
}

/// Splits a guest path into validated components. The empty path is the root
/// (no components). Nothing here touches the file system.
fn parse_path(path: &[u8]) -> Result<Vec<&str>> {
    if path.len() > MAX_PATH_BYTES {
        return Err(SharedFolderError::PathTooLong);
    }
    let text = std::str::from_utf8(path).map_err(|_| SharedFolderError::InvalidPath)?;
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let components: Vec<&str> = text.split('/').collect();
    if components.len() > MAX_PATH_DEPTH {
        return Err(SharedFolderError::InvalidPath);
    }
    for component in &components {
        validate_name(component)?;
    }
    Ok(components)
}

/// Like [`parse_path`] but the path must name an entry (not the root).
fn parse_entry_path(path: &[u8]) -> Result<Vec<&str>> {
    let components = parse_path(path)?;
    if components.is_empty() {
        Err(SharedFolderError::InvalidPath)
    } else {
        Ok(components)
    }
}

fn probe_case(fd: BorrowedFd<'_>, name: i32) -> Option<bool> {
    // SAFETY: `fd` is open for the call; `fpathconf` reads no memory.
    let value = unsafe { fpathconf(fd.as_raw_fd(), name) };
    (value >= 0).then_some(value != 0)
}

impl SharedFolder {
    /// Takes ownership of the root directory descriptor. Verifies it is a
    /// directory (defence in depth: control v2 already checked `fstat`),
    /// marks it close-on-exec and probes the volume's case behaviour. A volume
    /// that does not answer `_PC_CASE_SENSITIVE`/`_PC_CASE_PRESERVING` is reported
    /// case-sensitive and case-preserving (no folding assumed).
    pub fn new(root: OwnedFd, read_only: bool) -> Result<Self> {
        let metadata = File::from(root.try_clone().map_err(from_io)?)
            .metadata()
            .map_err(from_io)?;
        if !metadata.file_type().is_dir() {
            return Err(SharedFolderError::NotADirectory);
        }
        // The root descriptor must be read-only (control v2 section 8).
        // SAFETY: `root` is open; `F_GETFL` reads no memory.
        let status_flags = unsafe { fcntl(root.as_raw_fd(), F_GETFL) };
        if status_flags < 0 {
            return Err(from_io(io::Error::last_os_error()));
        }
        if status_flags & O_ACCMODE != O_RDONLY {
            return Err(SharedFolderError::PermissionDenied);
        }
        // SAFETY: `root` is open; `F_SETFD` takes an int and reads no memory.
        if unsafe { fcntl(root.as_raw_fd(), F_SETFD, FD_CLOEXEC) } != 0 {
            return Err(from_io(io::Error::last_os_error()));
        }
        let info = FolderInfo {
            read_only,
            case_sensitive: probe_case(root.as_fd(), PC_CASE_SENSITIVE).unwrap_or(true),
            case_preserving: probe_case(root.as_fd(), PC_CASE_PRESERVING).unwrap_or(true),
        };
        Ok(Self {
            root,
            info,
            files: HashMap::new(),
            next_handle: 1,
        })
    }

    pub fn info(&self) -> FolderInfo {
        self.info
    }

    /// Opens the parent directory of the last component. Returns it (`None`
    /// is the root) and the last name.
    fn walk_to_parent<'a>(&self, components: &[&'a str]) -> Result<(Option<OwnedFd>, &'a str)> {
        let (last, parents) = components
            .split_last()
            .ok_or(SharedFolderError::InvalidPath)?;
        let mut current: Option<OwnedFd> = None;
        for component in parents {
            let next = open_dir(dir_fd(&current, &self.root), &cstring(component)?)?;
            current = Some(next);
        }
        Ok((current, last))
    }

    /// Opens the directory a path names (the root for the empty path) as a
    /// descriptor with its own open file description.
    fn open_directory(&self, components: &[&str]) -> Result<OwnedFd> {
        if components.is_empty() {
            // "." opened relative to the root: a fresh open file description,
            // so listing never shares an offset with the root descriptor.
            return open_dir(self.root.as_fd(), c".");
        }
        let (parent, last) = self.walk_to_parent(components)?;
        open_dir(dir_fd(&parent, &self.root), &cstring(last)?)
    }

    /// Metadata of the entry a path names, without following a symlink.
    pub fn stat(&self, path: impl AsRef<[u8]>) -> Result<FileStat> {
        let components = parse_path(path.as_ref())?;
        let metadata = if components.is_empty() {
            File::from(self.root.try_clone().map_err(from_io)?)
                .metadata()
                .map_err(from_io)?
        } else {
            let (parent, last) = self.walk_to_parent(&components)?;
            open_entry(dir_fd(&parent, &self.root), &cstring(last)?)
                .and_then(|file| file.metadata())
                .map_err(from_io)?
        };
        let modified_ns = metadata
            .mtime()
            .saturating_mul(1_000_000_000)
            .saturating_add(metadata.mtime_nsec());
        Ok(FileStat {
            kind: EntryKind::of(&metadata),
            size: metadata.len(),
            modified_ns,
        })
    }

    /// Opens a regular file. Every mode but [`OpenMode::Read`] is refused in a
    /// read-only folder before anything is resolved.
    pub fn open(&mut self, path: impl AsRef<[u8]>, mode: OpenMode) -> Result<FileHandle> {
        if mode != OpenMode::Read && self.info.read_only {
            return Err(SharedFolderError::ReadOnly);
        }
        let components = parse_path(path.as_ref())?;
        if components.is_empty() {
            return Err(SharedFolderError::IsADirectory);
        }
        if self.files.len() >= MAX_OPEN_FILES {
            return Err(SharedFolderError::TooManyOpenFiles);
        }
        let (parent, last) = self.walk_to_parent(&components)?;
        let access = match mode {
            OpenMode::Read => O_RDONLY,
            OpenMode::Write | OpenMode::Truncate => O_RDWR,
            OpenMode::CreateNew => O_RDWR | O_CREAT | O_EXCL,
        };
        let fd = openat_fd(
            dir_fd(&parent, &self.root),
            &cstring(last)?,
            access | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC,
            CREATE_MODE,
        )
        .map_err(from_io)?;
        let file = File::from(fd);
        // The kind is checked on the descriptor that was opened, not on a
        // name that may have changed since. Truncation waits for this check
        // so that `O_TRUNC` never touches a non-regular object.
        let metadata = file.metadata().map_err(from_io)?;
        match EntryKind::of(&metadata) {
            EntryKind::File => {}
            EntryKind::Directory => return Err(SharedFolderError::IsADirectory),
            _ => return Err(SharedFolderError::UnsupportedKind),
        }
        if mode == OpenMode::Truncate {
            file.set_len(0).map_err(from_io)?;
        }
        let id = self.allocate_handle();
        self.files.insert(
            id,
            OpenFile {
                file,
                writable: mode != OpenMode::Read,
            },
        );
        Ok(FileHandle(id))
    }

    fn allocate_handle(&mut self) -> u32 {
        loop {
            let id = self.next_handle;
            self.next_handle = self.next_handle.checked_add(1).unwrap_or(1);
            if !self.files.contains_key(&id) {
                return id;
            }
        }
    }

    /// Reads up to `len` bytes at `offset`; fewer only at end of file.
    pub fn read(&self, handle: FileHandle, offset: u64, len: usize) -> Result<Vec<u8>> {
        if len > MAX_IO_BYTES {
            return Err(SharedFolderError::TooLarge);
        }
        let open = self
            .files
            .get(&handle.0)
            .ok_or(SharedFolderError::BadHandle)?;
        let mut buffer = vec![0u8; len];
        let mut filled = 0;
        while filled < len {
            let position = offset
                .checked_add(filled as u64)
                .filter(|p| *p <= i64::MAX as u64)
                .ok_or(SharedFolderError::TooLarge)?;
            match open.file.read_at(&mut buffer[filled..], position) {
                Ok(0) => break,
                Ok(count) => filled += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(from_io(error)),
            }
        }
        buffer.truncate(filled);
        Ok(buffer)
    }

    /// Writes all of `data` at `offset`. No locking: concurrent writers, in this
    /// service or another process, are not coordinated (contract section 8).
    pub fn write(&self, handle: FileHandle, offset: u64, data: &[u8]) -> Result<()> {
        let open = self
            .files
            .get(&handle.0)
            .ok_or(SharedFolderError::BadHandle)?;
        if !open.writable {
            return Err(SharedFolderError::ReadOnly);
        }
        if data.len() > MAX_IO_BYTES {
            return Err(SharedFolderError::TooLarge);
        }
        offset
            .checked_add(data.len() as u64)
            .filter(|end| *end <= i64::MAX as u64)
            .ok_or(SharedFolderError::TooLarge)?;
        open.file.write_all_at(data, offset).map_err(from_io)
    }

    pub fn close(&mut self, handle: FileHandle) -> Result<()> {
        self.files
            .remove(&handle.0)
            .map(drop)
            .ok_or(SharedFolderError::BadHandle)
    }

    /// Closes every open file; used when the agent restarts (the guest lost
    /// its handles).
    pub fn close_all(&mut self) {
        self.files.clear();
    }

    pub fn open_file_count(&self) -> usize {
        self.files.len()
    }

    /// Lists a directory: names ordered by their bytes, `.` and `..` omitted.
    /// `start` is 0 or the `next` of the previous page; `max_entries` is
    /// `1..=MAX_READDIR_PAGE`. Every call lists the directory again, so a directory
    /// changed between pages can make a name appear twice or be missed.
    pub fn readdir(
        &self,
        path: impl AsRef<[u8]>,
        start: u32,
        max_entries: usize,
    ) -> Result<DirPage> {
        if max_entries == 0 || max_entries > MAX_READDIR_PAGE {
            return Err(SharedFolderError::TooLarge);
        }
        let components = parse_path(path.as_ref())?;
        let dir = self.open_directory(&components)?;
        let (mut all, skipped) = list_directory(dir)?;
        all.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
        let start = start as usize;
        let end = start.saturating_add(max_entries).min(all.len());
        let entries = if start >= all.len() {
            Vec::new()
        } else {
            all[start..end].to_vec()
        };
        let next = (end < all.len()).then_some(end as u32);
        Ok(DirPage {
            entries,
            next,
            skipped,
        })
    }

    /// Renames `from` to `to`; never replaces an existing entry
    /// (`RENAME_EXCL`, `AlreadyExists`), also when both names are hard links
    /// to one file. Whether a change of case only is accepted on a
    /// case-insensitive volume is decided by `renameatx_np` (it is on APFS).
    pub fn rename(&self, from: impl AsRef<[u8]>, to: impl AsRef<[u8]>) -> Result<()> {
        if self.info.read_only {
            return Err(SharedFolderError::ReadOnly);
        }
        let from_components = parse_entry_path(from.as_ref())?;
        let to_components = parse_entry_path(to.as_ref())?;
        let (from_parent, from_name) = self.walk_to_parent(&from_components)?;
        let (to_parent, to_name) = self.walk_to_parent(&to_components)?;
        let from_name = cstring(from_name)?;
        let to_name = cstring(to_name)?;
        let from_dir = dir_fd(&from_parent, &self.root);
        let to_dir = dir_fd(&to_parent, &self.root);
        // SAFETY: both descriptors are live for the call, both names are
        // NUL-terminated strings that outlive it.
        let rc = unsafe {
            renameatx_np(
                from_dir.as_raw_fd(),
                from_name.as_ptr(),
                to_dir.as_raw_fd(),
                to_name.as_ptr(),
                RENAME_EXCL,
            )
        };
        if rc == 0 {
            return Ok(());
        }
        // EEXIST (the target exists) maps to `AlreadyExists` like every other
        // error; there is deliberately no fallback to a plain `renameat`. A
        // case-only rename is handled by `renameatx_np` itself on APFS, and
        // two hard links to one inode must stay refused: POSIX `rename`
        // returns 0 for them without doing anything.
        Err(from_io(io::Error::last_os_error()))
    }

    /// Removes a file or a symlink (the link itself). A directory is refused;
    /// v1 has no directory removal.
    pub fn unlink(&self, path: impl AsRef<[u8]>) -> Result<()> {
        if self.info.read_only {
            return Err(SharedFolderError::ReadOnly);
        }
        let components = parse_entry_path(path.as_ref())?;
        let (parent, last) = self.walk_to_parent(&components)?;
        let name = cstring(last)?;
        let dir = dir_fd(&parent, &self.root);
        // SAFETY: `dir` is live and `name` NUL-terminated for the call; flag 0
        // removes a non-directory (a symlink is removed, not followed).
        if unsafe { unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if matches!(error.raw_os_error(), Some(EPERM | EISDIR)) {
            // macOS reports a directory as EPERM; tell it from a real refusal.
            let is_dir = open_entry(dir, &name)
                .and_then(|file| file.metadata())
                .map(|metadata| metadata.file_type().is_dir())
                .unwrap_or(false);
            if is_dir {
                return Err(SharedFolderError::IsADirectory);
            }
        }
        Err(from_io(error))
    }
}

/// Reads every entry of an open directory (consumes the descriptor).
fn list_directory(dir: OwnedFd) -> Result<(Vec<DirEntry>, u32)> {
    let raw = dir.as_raw_fd();
    // SAFETY: `raw` is an open directory descriptor. On success the stream
    // owns it, so ownership is released from `dir` below; on failure `dir` still
    // owns and closes it.
    let stream = unsafe { fdopendir(raw) };
    if stream.is_null() {
        return Err(from_io(io::Error::last_os_error()));
    }
    let _ = dir.into_raw_fd();
    let mut entries = Vec::new();
    let mut skipped = 0u32;
    let mut seen = 0usize;
    let result = loop {
        // SAFETY: `__error` returns this thread's `errno` location; clearing
        // it tells the end of the directory from a failure after a null return.
        unsafe { *__error() = 0 };
        // SAFETY: `stream` is live until `closedir` below.
        let entry = unsafe { readdir(stream) };
        if entry.is_null() {
            // SAFETY: as above.
            let errno = unsafe { *__error() };
            break if errno == 0 {
                Ok(())
            } else {
                Err(from_io(io::Error::from_raw_os_error(errno)))
            };
        }
        // SAFETY: a non-null `readdir` result points to a `dirent` of
        // `d_reclen` bytes that stays valid until the next `readdir` or
        // `closedir`; the entry is variable length (shorter than
        // `size_of::<Dirent>()`), so no `&Dirent` is created: the fixed header
        // fields are read through raw pointers (they lie within the first 21
        // bytes, which every entry has) and the name only up to
        // `min(d_namlen, d_reclen - D_NAME_OFFSET, size of d_name)` bytes, copied
        // out before the next call.
        let (name_bytes, d_type) = unsafe {
            let reclen = std::ptr::addr_of!((*entry).d_reclen).read_unaligned() as usize;
            let namlen = std::ptr::addr_of!((*entry).d_namlen).read_unaligned() as usize;
            let d_type = std::ptr::addr_of!((*entry).d_type).read();
            let len = namlen
                .min(reclen.saturating_sub(D_NAME_OFFSET))
                .min(DIRENT_NAME_CAPACITY);
            let name =
                std::slice::from_raw_parts(std::ptr::addr_of!((*entry).d_name).cast::<u8>(), len);
            (name.to_vec(), d_type)
        };
        if name_bytes == b"." || name_bytes == b".." {
            continue;
        }
        seen += 1;
        if seen > MAX_DIR_ENTRIES {
            break Err(SharedFolderError::TooManyEntries);
        }
        let name = match String::from_utf8(name_bytes) {
            Ok(name) if validate_name(&name).is_ok() => name,
            _ => {
                skipped += 1;
                continue;
            }
        };
        let kind = match d_type {
            DT_REG => Some(EntryKind::File),
            DT_DIR => Some(EntryKind::Directory),
            DT_LNK => Some(EntryKind::Symlink),
            _ => None,
        };
        entries.push((name, kind));
    };
    // SAFETY: `stream` is live; this closes it and its descriptor.
    unsafe { closedir(stream) };
    result?;
    // The directory descriptor is gone; entries whose type the volume did not
    // report are listed as `Other` rather than guessed.
    let entries = entries
        .into_iter()
        .map(|(name, kind)| DirEntry {
            name,
            kind: kind.unwrap_or(EntryKind::Other),
        })
        .collect();
    Ok((entries, skipped))
}
