//! C ABI to create and inspect one RAW disk image inside an open directory.
//!
//! Contract: `docs/specs/STORAGE_HOST_ABI_V1.md`; declarations:
//! `include/open_desktop/storage_abi_v1.h`. The image is addressed by an open
//! directory descriptor plus a single entry name, never by a path. Creation
//! goes through `HostFileStorage::create_new_exclusive_at` (the directory
//! relative form of `create_new_exclusive`) and `od_storage::raw::RawImage`;
//! this module adds only argument validation, the free-space check and the
//! status mapping. Creation is completion only: nothing here flushes data or
//! the directory entry, and no durability is claimed (ISSUE-055).

use crate::storage_io::{HostFileStorage, WriterOpenError, unlink_at_if_identity};
use od_storage::raw::{RawError, RawImage};
use std::ffi::CString;
use std::fs::File;
use std::io;
use std::mem::{align_of, offset_of, size_of};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

const ABI_VERSION: u32 = 0x0001_0000;

/// Largest entry name in bytes: the `disks/<name>` config value is limited to
/// 255 bytes and `disks/` takes 6 (`docs/specs/VM_BUNDLE.md` section 3.3).
const MAX_NAME_BYTES: usize = 249;
/// Capacity granularity. RAW has no header and does not store a block size;
/// 512 is the candidate guest LBA size of `STORAGE_FORMAT.md` section 4
/// (hypothesis until S9-13) and the minimum alignment this ABI enforces.
const RAW_BLOCK_BYTES: u64 = 512;
const FORMAT_RAW: u32 = 1;

const OK: u32 = 0;
const INVALID_ARGUMENT: u32 = 1;
const INVALID_NAME: u32 = 2;
const ALREADY_EXISTS: u32 = 3;
const NOT_FOUND: u32 = 4;
const NOT_REGULAR_FILE: u32 = 5;
const INSUFFICIENT_SPACE: u32 = 6;
const WRITER_BUSY: u32 = 7;
const INVALID_IMAGE: u32 = 8;
const TRUNCATED: u32 = 9;
const SIZE_MISMATCH: u32 = 10;
const IO_ERROR: u32 = 11;

const EBADF: i32 = 9;
#[cfg(test)]
const EEXIST: i32 = 17;
const ENOTDIR: i32 = 20;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ErrorV1 {
    struct_size: u32,
    os_errno: i32,
    required_bytes: u64,
    available_bytes: u64,
    reserved: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ImageInfoV1 {
    struct_size: u32,
    format: u32,
    expected_size_bytes: u64,
    size_bytes: u64,
    os_errno: i32,
    reserved: u32,
}

const _: [(); 32] = [(); size_of::<ErrorV1>()];
const _: [(); 8] = [(); align_of::<ErrorV1>()];
const _: [(); 4] = [(); offset_of!(ErrorV1, os_errno)];
const _: [(); 8] = [(); offset_of!(ErrorV1, required_bytes)];
const _: [(); 16] = [(); offset_of!(ErrorV1, available_bytes)];
const _: [(); 24] = [(); offset_of!(ErrorV1, reserved)];
const _: [(); 32] = [(); size_of::<ImageInfoV1>()];
const _: [(); 8] = [(); align_of::<ImageInfoV1>()];
const _: [(); 4] = [(); offset_of!(ImageInfoV1, format)];
const _: [(); 8] = [(); offset_of!(ImageInfoV1, expected_size_bytes)];
const _: [(); 16] = [(); offset_of!(ImageInfoV1, size_bytes)];
const _: [(); 24] = [(); offset_of!(ImageInfoV1, os_errno)];
const _: [(); 28] = [(); offset_of!(ImageInfoV1, reserved)];

/// Details of a non-OK outcome. Fields that do not apply stay zero.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Failure {
    status: u32,
    os_errno: i32,
    required: u64,
    available: u64,
    size: u64,
}

impl Failure {
    fn status(status: u32) -> Self {
        Self {
            status,
            ..Self::default()
        }
    }

    fn io(status: u32, error: &io::Error) -> Self {
        Self {
            status,
            os_errno: error.raw_os_error().unwrap_or(0),
            ..Self::default()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn od_storage_abi_version() -> u32 {
    ABI_VERSION
}

/// Narrow Rust entry point for `od-image-tool create-raw`.
///
/// The creation policy remains private in this module and is shared with the
/// C ABI entry point below. This is not a second disk-format implementation;
/// it lets the repository build tool use the same create-new, free-space and
/// identity-checked rollback path without loading a C ABI library dynamically.
#[doc(hidden)]
pub fn create_raw_for_image_tool(
    directory: &Path,
    name: &[u8],
    capacity_bytes: u64,
) -> Result<(), u32> {
    // O_DIRECTORY makes this atomic with path resolution and fails on FIFOs
    // without waiting for a writer; O_NOFOLLOW rejects a symlink at the final
    // component. Do not metadata-check then open, which would race replacement.
    const O_NONBLOCK_MACOS: i32 = 0x0004;
    const O_NOFOLLOW_MACOS: i32 = 0x0100;
    const O_DIRECTORY_MACOS: i32 = 0x0010_0000;
    const O_CLOEXEC_MACOS: i32 = 0x0100_0000;
    let directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_NONBLOCK_MACOS | O_NOFOLLOW_MACOS | O_DIRECTORY_MACOS | O_CLOEXEC_MACOS)
        .open(directory)
        .map_err(|_| IO_ERROR)?;
    create_raw(
        directory.as_raw_fd(),
        name,
        capacity_bytes,
        &host_available_bytes,
    )
    .map_err(|failure| failure.status)
}

/// # Safety
///
/// `name` must point to `name_len` readable bytes and `out_error` to a
/// writable, aligned `od_storage_error_v1`; `dir_fd` must be a descriptor the
/// caller owns for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_storage_create_raw_v1(
    dir_fd: i32,
    name: *const u8,
    name_len: u32,
    capacity_bytes: u64,
    flags: u32,
    out_error: *mut std::ffi::c_void,
) -> u32 {
    let out = out_error.cast::<ErrorV1>();
    // SAFETY: a non-null, aligned pointer to the documented caller-owned
    // output struct; its `struct_size` field is read before anything else.
    if out.is_null()
        || !out.is_aligned()
        || unsafe { (*out).struct_size } != size_of::<ErrorV1>() as u32
    {
        return INVALID_ARGUMENT;
    }
    let outcome = if flags != 0 || name.is_null() {
        Err(Failure::status(INVALID_ARGUMENT))
    } else {
        // SAFETY: the caller promises `name_len` readable bytes at `name`.
        let name = unsafe { std::slice::from_raw_parts(name, name_len as usize) };
        create_raw(dir_fd, name, capacity_bytes, &host_available_bytes)
    };
    let failure = outcome.err().unwrap_or_default();
    // SAFETY: validated above; the struct is exclusive to this call.
    unsafe {
        out.write(ErrorV1 {
            struct_size: size_of::<ErrorV1>() as u32,
            os_errno: failure.os_errno,
            required_bytes: failure.required,
            available_bytes: failure.available,
            reserved: 0,
        });
    }
    failure.status
}

/// Input fields of `od_storage_image_info_v1`, copied out of the caller's
/// struct before anything is written to it.
struct InspectInput {
    struct_size: u32,
    expected_size_bytes: u64,
    reserved: u32,
}

/// # Safety
///
/// `name` must point to `name_len` readable bytes and `out_info` to a
/// writable, aligned `od_storage_image_info_v1`; `dir_fd` must be a
/// descriptor the caller owns for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_storage_inspect_v1(
    dir_fd: i32,
    name: *const u8,
    name_len: u32,
    out_info: *mut std::ffi::c_void,
) -> u32 {
    let out = out_info.cast::<ImageInfoV1>();
    // SAFETY: as in `od_storage_create_raw_v1`; only the three input fields
    // are read, each once, field by field. The output fields (`format`,
    // `size_bytes`, `os_errno`) are not read before they are written.
    if out.is_null() || !out.is_aligned() {
        return INVALID_ARGUMENT;
    }
    let input = unsafe {
        InspectInput {
            struct_size: std::ptr::addr_of!((*out).struct_size).read(),
            expected_size_bytes: std::ptr::addr_of!((*out).expected_size_bytes).read(),
            reserved: std::ptr::addr_of!((*out).reserved).read(),
        }
    };
    if input.struct_size != size_of::<ImageInfoV1>() as u32 || input.reserved != 0 {
        return INVALID_ARGUMENT;
    }
    let outcome = if name.is_null() {
        Err(Failure::status(INVALID_ARGUMENT))
    } else {
        // SAFETY: the caller promises `name_len` readable bytes at `name`.
        let name = unsafe { std::slice::from_raw_parts(name, name_len as usize) };
        inspect(dir_fd, name, input.expected_size_bytes)
    };
    let (status, os_errno, size) = match outcome {
        Ok(size) => (OK, 0, size),
        Err(failure) => (failure.status, failure.os_errno, failure.size),
    };
    // SAFETY: validated above; the struct is exclusive to this call.
    unsafe {
        out.write(ImageInfoV1 {
            struct_size: input.struct_size,
            format: if status == OK { FORMAT_RAW } else { 0 },
            expected_size_bytes: input.expected_size_bytes,
            size_bytes: size,
            os_errno,
            reserved: 0,
        });
    }
    status
}

/// Name rules of `VM_BUNDLE.md` section 3.3 (stricter than the plan text):
/// 1 to 249 bytes of UTF-8, not `.` or `..`, no `/`, no U+0000..U+001F and no
/// U+007F. Returns the name as `&str`.
fn validate_name(name: &[u8]) -> Result<&str, Failure> {
    let invalid = || Failure::status(INVALID_NAME);
    if name.is_empty() || name.len() > MAX_NAME_BYTES {
        return Err(invalid());
    }
    let name = std::str::from_utf8(name).map_err(|_| invalid())?;
    if name == "." || name == ".." {
        return Err(invalid());
    }
    if name
        .chars()
        .any(|c| c == '/' || c <= '\u{1f}' || c == '\u{7f}')
    {
        return Err(invalid());
    }
    Ok(name)
}

/// Checks `dir_fd` is an open directory; the returned file is a private
/// duplicate used only for that check and dropped by the caller.
fn check_directory(dir_fd: i32) -> Result<(), Failure> {
    if dir_fd < 0 {
        return Err(Failure {
            status: INVALID_ARGUMENT,
            os_errno: EBADF,
            ..Failure::default()
        });
    }
    // SAFETY: the caller guarantees `dir_fd` stays open for the call; it is
    // only duplicated, never closed here.
    let borrowed = unsafe { BorrowedFd::borrow_raw(dir_fd) };
    let duplicate = borrowed
        .try_clone_to_owned()
        .map_err(|e| Failure::io(INVALID_ARGUMENT, &e))?;
    let metadata = File::from(duplicate)
        .metadata()
        .map_err(|e| Failure::io(INVALID_ARGUMENT, &e))?;
    if metadata.is_dir() {
        Ok(())
    } else {
        Err(Failure {
            status: INVALID_ARGUMENT,
            os_errno: ENOTDIR,
            ..Failure::default()
        })
    }
}

/// Unprivileged available space of the volume holding `dir`, in bytes.
///
/// `fstatvfs` on macOS reports block counts as 32-bit values, so a volume
/// with more than `u32::MAX` free fragments saturates at that count.
fn host_available_bytes(dir: BorrowedFd<'_>) -> io::Result<u64> {
    /// macOS `struct statvfs` (`<sys/statvfs.h>`, 64 bytes, verified with a C
    /// probe on macOS 26.7.1). `fsblkcnt_t`/`fsfilcnt_t` are `unsigned int`.
    #[repr(C)]
    struct Statvfs {
        f_bsize: u64,
        f_frsize: u64,
        f_blocks: u32,
        f_bfree: u32,
        f_bavail: u32,
        f_files: u32,
        f_ffree: u32,
        f_favail: u32,
        f_fsid: u64,
        f_flag: u64,
        f_namemax: u64,
    }
    const _: [(); 64] = [(); size_of::<Statvfs>()];
    const _: [(); 8] = [(); offset_of!(Statvfs, f_frsize)];
    const _: [(); 24] = [(); offset_of!(Statvfs, f_bavail)];
    unsafe extern "C" {
        // SAFETY contract: `fd` is open and `buf` points to a writable
        // `struct statvfs`.
        fn fstatvfs(fd: i32, buf: *mut Statvfs) -> i32;
    }
    let mut stat = std::mem::MaybeUninit::<Statvfs>::zeroed();
    // SAFETY: `dir` is live for the call and `stat` is a writable buffer of
    // the exact C layout (asserted above).
    if unsafe { fstatvfs(dir.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fstatvfs` succeeded and wrote the whole struct.
    let stat = unsafe { stat.assume_init() };
    let unit = if stat.f_frsize != 0 {
        stat.f_frsize
    } else {
        stat.f_bsize
    };
    Ok(u64::from(stat.f_bavail).saturating_mul(unit))
}

/// Creation logic with the free-space probe injectable. Precedence of
/// failures: name, capacity, directory descriptor, free space, creation.
fn create_raw(
    dir_fd: i32,
    name: &[u8],
    capacity: u64,
    available: &dyn Fn(BorrowedFd<'_>) -> io::Result<u64>,
) -> Result<(), Failure> {
    let name = validate_name(name)?;
    if capacity == 0
        || !capacity.is_multiple_of(RAW_BLOCK_BYTES)
        || capacity > crate::storage_io::HOST_MAX_LEN
    {
        return Err(Failure::status(INVALID_ARGUMENT));
    }
    check_directory(dir_fd)?;
    // SAFETY: checked open above; the caller keeps it open for the call.
    let dir = unsafe { BorrowedFd::borrow_raw(dir_fd) };
    let free = available(dir).map_err(|e| Failure::io(IO_ERROR, &e))?;
    if free < capacity {
        return Err(Failure {
            status: INSUFFICIENT_SPACE,
            required: capacity,
            available: free,
            ..Failure::default()
        });
    }
    let storage = HostFileStorage::create_new_exclusive_at(dir, name, capacity)
        .map_err(|error| map_create_error(&error))?;
    let identity = storage.file_identity();
    match RawImage::create(storage, capacity, RAW_BLOCK_BYTES) {
        // The image value and with it the writer lock are dropped here.
        Ok(_image) => Ok(()),
        Err(error) => {
            // Cannot happen for validated arguments; if it does, remove what
            // this call created (identity-checked) and report an I/O failure.
            if let (Some(identity), Ok(c_name)) = (identity, CString::new(name)) {
                let _ = unlink_at_if_identity(dir, &c_name, identity);
            }
            Err(Failure {
                status: IO_ERROR,
                os_errno: raw_errno(&error),
                ..Failure::default()
            })
        }
    }
}

fn map_create_error(error: &WriterOpenError) -> Failure {
    match error {
        WriterOpenError::WriterBusy => Failure::status(WRITER_BUSY),
        // The entry was replaced between create and lock; no errno exists.
        WriterOpenError::IdentityMismatch => Failure::status(IO_ERROR),
        WriterOpenError::Io(error) => match error.kind() {
            io::ErrorKind::AlreadyExists => Failure::io(ALREADY_EXISTS, error),
            _ => Failure::io(IO_ERROR, error),
        },
    }
}

fn raw_errno(error: &RawError) -> i32 {
    match error {
        RawError::Storage(od_storage::io::StorageError::Io {
            os_error: Some(errno),
            ..
        }) => *errno,
        _ => 0,
    }
}

/// Inspection logic; `Ok` carries the image size. `expected` zero means
/// "not given".
fn inspect(dir_fd: i32, name: &[u8], expected: u64) -> Result<u64, Failure> {
    let name = validate_name(name)?;
    check_directory(dir_fd)?;
    // SAFETY: checked open above; the caller keeps it open for the call.
    let dir = unsafe { BorrowedFd::borrow_raw(dir_fd) };
    let storage =
        HostFileStorage::open_read_only_at(dir.as_fd(), name).map_err(|error| {
            match error.kind() {
                io::ErrorKind::NotFound => Failure::io(NOT_FOUND, &error),
                io::ErrorKind::InvalidInput => Failure::io(NOT_REGULAR_FILE, &error),
                _ => Failure::io(IO_ERROR, &error),
            }
        })?;
    let expected = (expected != 0).then_some(expected);
    match RawImage::open_read_only(storage, expected, RAW_BLOCK_BYTES) {
        Ok(image) => Ok(image.logical_size()),
        Err(RawError::Truncated { actual, .. }) => Err(Failure {
            status: TRUNCATED,
            size: actual,
            ..Failure::default()
        }),
        Err(RawError::SizeMismatch { actual, .. }) => Err(Failure {
            status: SIZE_MISMATCH,
            size: actual,
            ..Failure::default()
        }),
        Err(RawError::ZeroSize) => Err(Failure::status(INVALID_IMAGE)),
        Err(RawError::SizeNotBlockMultiple { size, .. }) => Err(Failure {
            status: INVALID_IMAGE,
            size,
            ..Failure::default()
        }),
        Err(other) => Err(Failure {
            status: IO_ERROR,
            os_errno: raw_errno(&other),
            ..Failure::default()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage_io::process_spawn_guard;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    /// `base/` holds `disks/` (the directory under test) and `outside/`.
    struct Fixture {
        base: PathBuf,
        disks: PathBuf,
        outside: PathBuf,
        dir: File,
    }

    impl Fixture {
        fn new() -> Self {
            let base = std::env::temp_dir().join(format!(
                "od-host-storage-abi-{}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            let disks = base.join("disks");
            let outside = base.join("outside");
            std::fs::create_dir_all(&disks).expect("create disks");
            std::fs::create_dir(&outside).expect("create outside");
            let dir = File::open(&disks).expect("open disks");
            Self {
                base,
                disks,
                outside,
                dir,
            }
        }

        fn fd(&self) -> i32 {
            self.dir.as_raw_fd()
        }

        fn entries(&self) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(&self.disks)
                .expect("read disks")
                .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }

        fn outside_entries(&self) -> usize {
            std::fs::read_dir(&self.outside)
                .expect("read outside")
                .count()
        }

        fn base_entries(&self) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(&self.base)
                .expect("read base")
                .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn plenty(_: BorrowedFd<'_>) -> io::Result<u64> {
        Ok(u64::MAX)
    }

    fn create(fx: &Fixture, name: &[u8], capacity: u64) -> Result<(), Failure> {
        create_raw(fx.fd(), name, capacity, &plenty)
    }

    fn status_of(result: Result<(), Failure>) -> u32 {
        result.err().map_or(OK, |f| f.status)
    }

    /// Calls the exported `od_storage_create_raw_v1`.
    fn abi_create(fx: &Fixture, name: &[u8], capacity: u64, flags: u32) -> (u32, ErrorV1) {
        let mut error = ErrorV1 {
            struct_size: size_of::<ErrorV1>() as u32,
            ..ErrorV1::default()
        };
        // SAFETY: valid pointers and a live descriptor owned by `fx`.
        let status = unsafe {
            od_storage_create_raw_v1(
                fx.fd(),
                name.as_ptr(),
                name.len() as u32,
                capacity,
                flags,
                (&raw mut error).cast(),
            )
        };
        (status, error)
    }

    /// Calls the exported `od_storage_inspect_v1`.
    fn abi_inspect(fx: &Fixture, name: &[u8], expected: u64) -> (u32, ImageInfoV1) {
        let mut info = ImageInfoV1 {
            struct_size: size_of::<ImageInfoV1>() as u32,
            expected_size_bytes: expected,
            ..ImageInfoV1::default()
        };
        // SAFETY: valid pointers and a live descriptor owned by `fx`.
        let status = unsafe {
            od_storage_inspect_v1(
                fx.fd(),
                name.as_ptr(),
                name.len() as u32,
                (&raw mut info).cast(),
            )
        };
        (status, info)
    }

    #[test]
    fn version_is_exact_and_documented_value() {
        assert_eq!(od_storage_abi_version(), 0x0001_0000);
    }

    #[test]
    fn creates_zeroed_private_regular_file_of_exact_size_and_releases_lock() {
        let _serial = process_spawn_guard();
        let fx = Fixture::new();
        let (status, error) = abi_create(&fx, b"disk0.img", 8192, 0);
        assert_eq!(status, OK);
        assert_eq!(error.struct_size, 32);
        assert_eq!(
            (error.os_errno, error.required_bytes, error.available_bytes),
            (0, 0, 0)
        );
        let path = fx.disks.join("disk0.img");
        let meta = std::fs::symlink_metadata(&path).expect("stat");
        assert!(meta.file_type().is_file());
        assert_eq!(meta.len(), 8192);
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert!(std::fs::read(&path).expect("read").iter().all(|b| *b == 0));
        assert_eq!(fx.entries(), ["disk0.img"]);
        // The creation lock is gone: a writer can take the file at once.
        drop(HostFileStorage::open_read_write_exclusive(&path).expect("lock free"));
    }

    #[test]
    fn existing_name_is_refused_and_the_file_is_not_touched() {
        let _serial = process_spawn_guard();
        let fx = Fixture::new();
        assert_eq!(abi_create(&fx, b"a.img", 4096, 0).0, OK);
        let path = fx.disks.join("a.img");
        std::fs::write(&path, vec![7u8; 4096]).expect("seed bytes");
        let (status, error) = abi_create(&fx, b"a.img", 8192, 0);
        assert_eq!(status, ALREADY_EXISTS);
        assert_eq!(error.os_errno, 17);
        assert_eq!(std::fs::read(&path).expect("read"), vec![7u8; 4096]);
        assert_eq!(fx.entries(), ["a.img"]);
        // A foreign file (not made by this ABI) is refused the same way.
        std::fs::write(fx.disks.join("foreign"), b"keep").expect("seed");
        assert_eq!(abi_create(&fx, b"foreign", 4096, 0).0, ALREADY_EXISTS);
        assert_eq!(std::fs::read(fx.disks.join("foreign")).unwrap(), b"keep");
    }

    #[test]
    fn symlink_in_place_of_the_image_is_refused_and_not_followed() {
        let _serial = process_spawn_guard();
        let fx = Fixture::new();
        // Dangling link: creation must not create the target.
        let target = fx.outside.join("created-through-link");
        symlink(&target, fx.disks.join("dangling")).expect("symlink");
        assert_eq!(abi_create(&fx, b"dangling", 4096, 0).0, ALREADY_EXISTS);
        assert!(!target.exists());
        assert_eq!(fx.outside_entries(), 0);
        // Link to an existing regular file: the file must keep its bytes.
        let real = fx.outside.join("real.img");
        std::fs::write(&real, vec![3u8; 4096]).expect("seed");
        symlink(&real, fx.disks.join("link.img")).expect("symlink");
        assert_eq!(abi_create(&fx, b"link.img", 4096, 0).0, ALREADY_EXISTS);
        assert_eq!(std::fs::read(&real).unwrap(), vec![3u8; 4096]);
        // Inspect refuses both links, whatever they point at.
        assert_eq!(abi_inspect(&fx, b"link.img", 0).0, NOT_REGULAR_FILE);
        assert_eq!(abi_inspect(&fx, b"dangling", 0).0, NOT_REGULAR_FILE);
        // The links are still links.
        for name in ["dangling", "link.img"] {
            let meta = std::fs::symlink_metadata(fx.disks.join(name)).unwrap();
            assert!(meta.file_type().is_symlink());
        }
    }

    #[test]
    fn names_that_could_leave_the_directory_or_break_bundle_rules_are_refused() {
        let _serial = process_spawn_guard();
        let fx = Fixture::new();
        let long = vec![b'a'; MAX_NAME_BYTES + 1];
        let bad: Vec<&[u8]> = vec![
            b"",
            b".",
            b"..",
            b"a/b",
            b"../escape.img",
            b"/abs.img",
            b"dir/",
            b"a\0b",
            b"line\nbreak",
            b"tab\t",
            b"\x1fcontrol",
            b"del\x7f",
            b"\xff\xfe",
            b"\xc0\xaf",
            &long,
        ];
        for name in bad {
            let (status, _) = abi_create(&fx, name, 4096, 0);
            assert_eq!(status, INVALID_NAME, "name {name:?}");
            assert_eq!(abi_inspect(&fx, name, 0).0, INVALID_NAME, "name {name:?}");
        }
        assert!(fx.entries().is_empty());
        assert_eq!(fx.outside_entries(), 0);
        assert_eq!(fx.base_entries(), ["disks", "outside"]);
    }

    #[test]
    fn longest_and_multibyte_names_are_accepted() {
        let _serial = process_spawn_guard();
        let fx = Fixture::new();
        let longest = "a".repeat(MAX_NAME_BYTES);
        assert_eq!(abi_create(&fx, longest.as_bytes(), 4096, 0).0, OK);
        let multibyte = "диск 1.raw";
        assert_eq!(abi_create(&fx, multibyte.as_bytes(), 4096, 0).0, OK);
        assert_eq!(abi_inspect(&fx, multibyte.as_bytes(), 0).0, OK);
        // Leading dot and spaces are legal entry names (not `.` or `..`).
        assert_eq!(abi_create(&fx, b".hidden", 4096, 0).0, OK);
        assert_eq!(abi_create(&fx, b"..x", 4096, 0).0, OK);
        assert_eq!(fx.entries().len(), 4);
    }

    #[test]
    fn symlinked_name_cannot_reach_outside_the_directory() {
        let _serial = process_spawn_guard();
        let fx = Fixture::new();
        std::fs::write(fx.outside.join("victim"), vec![9u8; 4096]).unwrap();
        symlink("../outside/victim", fx.disks.join("escape")).expect("symlink");
        symlink("../outside", fx.disks.join("escape-dir")).expect("symlink");
        assert_eq!(abi_create(&fx, b"escape", 4096, 0).0, ALREADY_EXISTS);
        assert_eq!(abi_create(&fx, b"escape-dir", 4096, 0).0, ALREADY_EXISTS);
        assert_eq!(abi_inspect(&fx, b"escape", 0).0, NOT_REGULAR_FILE);
        assert_eq!(abi_inspect(&fx, b"escape-dir", 0).0, NOT_REGULAR_FILE);
        // A component path through the link is not a single name at all.
        assert_eq!(abi_create(&fx, b"escape-dir/new", 4096, 0).0, INVALID_NAME);
        assert_eq!(
            std::fs::read(fx.outside.join("victim")).unwrap(),
            vec![9u8; 4096]
        );
        assert_eq!(fx.outside_entries(), 1);
    }

    #[test]
    fn the_descriptor_not_the_path_decides_where_the_image_goes() {
        let _serial = process_spawn_guard();
        let fx = Fixture::new();
        let moved = fx.base.join("moved");
        std::fs::rename(&fx.disks, &moved).expect("rename directory");
        std::fs::create_dir(&fx.disks).expect("decoy at the old path");
        assert_eq!(abi_create(&fx, b"pinned.img", 4096, 0).0, OK);
        assert!(moved.join("pinned.img").is_file());
        assert!(std::fs::read_dir(&fx.disks).unwrap().next().is_none());
    }

    #[test]
    fn invalid_capacity_flags_and_pointers_are_invalid_argument() {
        let _serial = process_spawn_guard();
        let fx = Fixture::new();
        let aligned_huge = (i64::MAX as u64) & !(RAW_BLOCK_BYTES - 1);
        for capacity in [0, 1, 511, 513, 4097, u64::MAX, (i64::MAX as u64) + 1] {
            assert_eq!(
                abi_create(&fx, b"c.img", capacity, 0).0,
                INVALID_ARGUMENT,
                "{capacity}"
            );
        }
        assert_eq!(abi_create(&fx, b"c.img", 4096, 1).0, INVALID_ARGUMENT);
        assert!(fx.entries().is_empty());
        // Exactly the largest aligned size is not an argument error; it only
        // fails on space (real probe, no injection).
        let (status, error) = abi_create(&fx, b"huge.img", aligned_huge, 0);
        assert_eq!(status, INSUFFICIENT_SPACE);
        assert_eq!(error.required_bytes, aligned_huge);
        assert!(error.available_bytes < aligned_huge);
        assert!(fx.entries().is_empty());

        let mut error = ErrorV1 {
            struct_size: 32,
            os_errno: 77,
            ..ErrorV1::default()
        };
        // SAFETY: valid pointers; the null cases are the point of the test.
        unsafe {
            let e = (&raw mut error).cast();
            let n = b"p.img";
            assert_eq!(
                od_storage_create_raw_v1(fx.fd(), std::ptr::null(), 5, 4096, 0, e),
                INVALID_ARGUMENT
            );
            assert_eq!(error.os_errno, 0, "struct valid: written on failure");
            assert_eq!(
                od_storage_create_raw_v1(fx.fd(), n.as_ptr(), 5, 4096, 0, std::ptr::null_mut()),
                INVALID_ARGUMENT
            );
            error.struct_size = 31;
            error.os_errno = 77;
            assert_eq!(
                od_storage_create_raw_v1(fx.fd(), n.as_ptr(), 5, 4096, 0, e),
                INVALID_ARGUMENT
            );
            assert_eq!(error.os_errno, 77, "bad struct_size: output untouched");
        }
        assert!(fx.entries().is_empty());
    }

    #[test]
    fn bad_directory_descriptors_are_invalid_argument() {
        let _serial = process_spawn_guard();
        let fx = Fixture::new();
        let file = File::create(fx.base.join("regular")).expect("file");
        let mut error = ErrorV1 {
            struct_size: 32,
            ..ErrorV1::default()
        };
        for (fd, errno) in [(-1, EBADF), (file.as_raw_fd(), ENOTDIR)] {
            // SAFETY: valid pointers; the descriptors are the point of the test.
            let status = unsafe {
                od_storage_create_raw_v1(fd, b"x.img".as_ptr(), 5, 4096, 0, (&raw mut error).cast())
            };
            assert_eq!(status, INVALID_ARGUMENT);
            assert_eq!(error.os_errno, errno);
        }
        assert!(!fx.disks.join("x.img").exists());
    }

    #[test]
    fn free_space_is_checked_before_creation_with_both_numbers() {
        let _serial = process_spawn_guard();
        let fx = Fixture::new();
        let short = |_: BorrowedFd<'_>| Ok(4095);
        let failure = create_raw(fx.fd(), b"s.img", 4096, &short).unwrap_err();
        assert_eq!(
            failure,
            Failure {
                status: INSUFFICIENT_SPACE,
                os_errno: 0,
                required: 4096,
                available: 4095,
                size: 0
            }
        );
        assert!(fx.entries().is_empty());
        // Exactly enough is enough.
        let exact = |_: BorrowedFd<'_>| Ok(4096);
        assert!(create_raw(fx.fd(), b"s.img", 4096, &exact).is_ok());
        // A failing probe is an I/O error, nothing is created.
        let broken = |_: BorrowedFd<'_>| Err(io::Error::from_raw_os_error(5));
        let failure = create_raw(fx.fd(), b"t.img", 4096, &broken).unwrap_err();
        assert_eq!((failure.status, failure.os_errno), (IO_ERROR, 5));
        assert_eq!(fx.entries(), ["s.img"]);
    }

    #[test]
    fn real_free_space_probe_reports_a_plausible_number() {
        let fx = Fixture::new();
        let free = host_available_bytes(fx.dir.as_fd()).expect("fstatvfs");
        assert!(free > 0);
    }

    #[test]
    fn inspect_reports_size_and_format_of_a_created_image() {
        let _serial = process_spawn_guard();
        let fx = Fixture::new();
        assert_eq!(status_of(create(&fx, b"i.img", 16384)), OK);
        let (status, info) = abi_inspect(&fx, b"i.img", 0);
        assert_eq!(status, OK);
        assert_eq!((info.format, info.size_bytes, info.os_errno), (1, 16384, 0));
        let (status, info) = abi_inspect(&fx, b"i.img", 16384);
        assert_eq!((status, info.size_bytes), (OK, 16384));
        assert_eq!(info.expected_size_bytes, 16384, "input is preserved");
    }

    #[test]
    fn inspect_distinguishes_truncated_oversized_and_malformed_files() {
        let _serial = process_spawn_guard();
        let fx = Fixture::new();
        let path = fx.disks.join("t.img");
        assert_eq!(status_of(create(&fx, b"t.img", 8192)), OK);
        // Cut to half: valid geometry, but shorter than the creator promised.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(4096)
            .unwrap();
        let (status, info) = abi_inspect(&fx, b"t.img", 8192);
        assert_eq!((status, info.size_bytes, info.format), (TRUNCATED, 4096, 0));
        // Without an expectation a block-multiple cut cannot be told apart.
        assert_eq!(abi_inspect(&fx, b"t.img", 0).0, OK);
        // Larger than expected.
        let (status, info) = abi_inspect(&fx, b"t.img", 2048);
        assert_eq!((status, info.size_bytes), (SIZE_MISMATCH, 4096));
        // Cut inside a block: malformed even without an expectation.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(4000)
            .unwrap();
        let (status, info) = abi_inspect(&fx, b"t.img", 0);
        assert_eq!((status, info.size_bytes), (INVALID_IMAGE, 4000));
        assert_eq!(abi_inspect(&fx, b"t.img", 8192).0, TRUNCATED);
        // Empty file.
        std::fs::write(fx.disks.join("empty.img"), b"").unwrap();
        let (status, info) = abi_inspect(&fx, b"empty.img", 0);
        assert_eq!((status, info.size_bytes), (INVALID_IMAGE, 0));
        assert_eq!(abi_inspect(&fx, b"empty.img", 4096).0, TRUNCATED);
    }

    #[test]
    fn inspect_refuses_missing_and_non_regular_entries_without_blocking() {
        let _serial = process_spawn_guard();
        let fx = Fixture::new();
        let (status, info) = abi_inspect(&fx, b"missing.img", 0);
        assert_eq!((status, info.os_errno), (NOT_FOUND, 2));
        std::fs::create_dir(fx.disks.join("subdir")).unwrap();
        assert_eq!(abi_inspect(&fx, b"subdir", 0).0, NOT_REGULAR_FILE);
        let status = std::process::Command::new("mkfifo")
            .arg(fx.disks.join("pipe"))
            .status()
            .expect("run mkfifo");
        assert!(status.success());
        // Would block forever if the FIFO were opened without O_NONBLOCK.
        assert_eq!(abi_inspect(&fx, b"pipe", 0).0, NOT_REGULAR_FILE);
    }

    #[test]
    fn inspect_changes_nothing_and_is_not_blocked_by_a_writer() {
        let _serial = process_spawn_guard();
        let fx = Fixture::new();
        assert_eq!(status_of(create(&fx, b"w.img", 4096)), OK);
        let path = fx.disks.join("w.img");
        let before = std::fs::metadata(&path).unwrap();
        let writer = HostFileStorage::open_read_write_exclusive(&path).expect("writer");
        assert_eq!(abi_inspect(&fx, b"w.img", 0).0, OK, "inspect takes no lock");
        // And creation never displaces the holder.
        assert_eq!(abi_create(&fx, b"w.img", 4096, 0).0, ALREADY_EXISTS);
        drop(writer);
        let after = std::fs::metadata(&path).unwrap();
        assert_eq!(
            (before.len(), before.mtime(), before.mtime_nsec()),
            (after.len(), after.mtime(), after.mtime_nsec())
        );
    }

    #[test]
    fn inspect_input_struct_is_validated() {
        let _serial = process_spawn_guard();
        let fx = Fixture::new();
        assert_eq!(status_of(create(&fx, b"v.img", 4096)), OK);
        let mut info = ImageInfoV1 {
            struct_size: 32,
            reserved: 1,
            size_bytes: 55,
            ..ImageInfoV1::default()
        };
        let n = b"v.img";
        // SAFETY: valid pointers; the field values are the point of the test.
        unsafe {
            let p = (&raw mut info).cast();
            assert_eq!(
                od_storage_inspect_v1(fx.fd(), n.as_ptr(), 5, p),
                INVALID_ARGUMENT
            );
            assert_eq!(info.size_bytes, 55, "reserved nonzero: output untouched");
            info.reserved = 0;
            info.struct_size = 24;
            assert_eq!(
                od_storage_inspect_v1(fx.fd(), n.as_ptr(), 5, p),
                INVALID_ARGUMENT
            );
            assert_eq!(info.size_bytes, 55, "bad struct_size: output untouched");
            assert_eq!(
                od_storage_inspect_v1(fx.fd(), n.as_ptr(), 5, std::ptr::null_mut()),
                INVALID_ARGUMENT
            );
            let mut valid = ImageInfoV1 {
                struct_size: 32,
                ..ImageInfoV1::default()
            };
            assert_eq!(
                od_storage_inspect_v1(fx.fd(), std::ptr::null(), 5, (&raw mut valid).cast()),
                INVALID_ARGUMENT
            );
        }
    }

    #[test]
    fn create_errors_map_to_closed_statuses() {
        assert_eq!(
            map_create_error(&WriterOpenError::WriterBusy),
            Failure::status(WRITER_BUSY)
        );
        // No errno exists for a replaced entry.
        assert_eq!(
            map_create_error(&WriterOpenError::IdentityMismatch),
            Failure::status(IO_ERROR)
        );
        let exists = map_create_error(&WriterOpenError::Io(io::Error::from_raw_os_error(EEXIST)));
        assert_eq!((exists.status, exists.os_errno), (ALREADY_EXISTS, EEXIST));
        let other = map_create_error(&WriterOpenError::Io(io::Error::from_raw_os_error(EBADF)));
        assert_eq!((other.status, other.os_errno), (IO_ERROR, EBADF));
        let no_errno = map_create_error(&WriterOpenError::Io(io::Error::other("x")));
        assert_eq!((no_errno.status, no_errno.os_errno), (IO_ERROR, 0));
    }
}
