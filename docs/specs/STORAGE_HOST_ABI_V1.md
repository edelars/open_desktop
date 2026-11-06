# Storage host ABI v1: create and inspect a RAW image

**Status:** normative contract with a Rust implementation (S11-07, 2026-11-06); not reviewed, not accepted. The header
[`storage_abi_v1.h`](../../include/open_desktop/storage_abi_v1.h) is the sole source of C declarations and numeric
constants. The exported symbols are in [`storage_abi_v1.rs`](../../crates/od-host/src/storage_abi_v1.rs). Tests: the
`od-host` unit tests (`cargo test -p od-host`) and the C smoke
[`storage_abi_smoke.c`](../../tests/system/storage_abi_smoke.c). Nothing here is wired into VMHost or the app yet
(S11-03, S11-08). MUST and MUST NOT are normative.

The ABI serves one job: VMHost creates the RAW disk image of a new machine inside the bundle's `disks/` directory and
checks an existing image, without parsing formats in Swift. It is not a disk manager: no resize, convert, snapshot,
CoW image, delete or rename exists here. Only `od-vmhost` links `libod_host.a`; the app and the CLI MUST NOT call it
([ADR-0006](../decisions/ADR-0006-desktop-app-shell.md), [VM_BUNDLE.md](VM_BUNDLE.md) section 2.1).

The ABI exists on macOS only: the `openat`/`unlinkat` flag values and the `statvfs` layout used by the implementation
are the macOS SDK values, checked with a C probe on macOS 26.7.1. On other targets the symbols are not built.

## Durability: not claimed

A successful create is **completion only**. The ABI neither flushes the file data (`F_FULLFSYNC`) nor the directory
entry and promises nothing about power loss, kernel panic or drive failure. Directory-entry (namespace) durability is
unconfirmed until S9-06 closes [ISSUE-055](../KNOWN_ISSUES.md#issue-055--host-durability-guarantee-for-macosapfs-is-unconfirmed);
a caller MUST NOT present success as "saved reliably". Creating a disk is an operation on the bundle before the VM
identity exists ([VM_BUNDLE.md](VM_BUNDLE.md) sections 3.4 and 6), so it has no `unsupported_durability` status;
refusing `START` of a machine with a disk before S9-06 is the job of control v2, not of this ABI.

## Identification

`od_storage_abi_version()` returns `OD_STORAGE_ABI_VERSION_V1` (`0x00010000`): an exact-match identifier in its own
namespace, not a wire version. A client that sees another value or no symbol MUST NOT call the other functions.
There is no capability word: the ABI has no optional parts.

## Addressing and ownership

* An image is `(dir_fd, name)`. `dir_fd` is an open directory descriptor (VMHost opens `<bundle>/disks` with
  `O_DIRECTORY | O_NOFOLLOW` itself). It is **borrowed**: never closed, never retained after the call. The
  implementation duplicates it privately when it must outlive a step. An invalid descriptor or one that is not a
  directory is `INVALID_ARGUMENT` (`os_errno` `EBADF` or `ENOTDIR`).
* `name` is `name_len` bytes of caller-owned memory, UTF-8, **not** NUL-terminated, not retained.
* Because the file is created and opened with `openat` relative to `dir_fd`, no absolute path is involved: renaming or
  replacing the directory's path after it was opened does not redirect the operation, and a name cannot leave the
  directory.
* Name rules (this is the contract of [VM_BUNDLE.md](VM_BUNDLE.md) section 3.3, stricter than the plan text of S11-07,
  and this ABI adopts it): 1 to `OD_STORAGE_MAX_NAME_BYTES_V1` (249) bytes; valid UTF-8; not `.` and not `..`; no `/`;
  no U+0000..U+001F and no U+007F. Anything else is `INVALID_NAME` and nothing is touched. Leading dots such as
  `.hidden` and `..x`, spaces and non-ASCII letters are legal names. The 249 limit makes `disks/<name>` fit the
  255-byte `path` value; it is a hypothesis of VM_BUNDLE, not a measurement.
* Outputs are two caller-owned structs, `od_storage_error_v1` and `od_storage_image_info_v1` (32 bytes, alignment 8).
  Each has `struct_size` as its first field, which is an **input** and MUST equal the header constant (the functions
  take no separate size argument). A NULL, misaligned or wrongly sized struct returns `INVALID_ARGUMENT` and the struct
  is untouched. Once its inputs were accepted the struct is written completely on every return, with zero in fields
  that do not apply. Dangling non-NULL pointers are caller contract violations.
* No Rust object, callback or pointer to library memory crosses the boundary. Calls are synchronous and may be made
  from any thread; they share no state.

## `od_storage_create_raw_v1`

`od_storage_create_raw_v1(dir_fd, name, name_len, capacity_bytes, flags, out_error)`

Creates `<dir_fd>/<name>` as a regular file of exactly `capacity_bytes` zero bytes, mode `0600`, RAW format (headerless,
bytes 1:1, `STORAGE_FORMAT.md`). The call is the directory-relative form of `HostFileStorage::create_new_exclusive`
(`openat` with `O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC`, then the same exclusive `flock`, identity check
and sizing), followed by `RawImage::create`; there is no second `O_EXCL` implementation.

* `capacity_bytes` MUST be nonzero, a multiple of `OD_STORAGE_RAW_BLOCK_BYTES_V1` (512) and at most `i64::MAX`;
  otherwise `INVALID_ARGUMENT`. RAW stores no block size; 512 is the candidate guest LBA size of
  [STORAGE_FORMAT.md](STORAGE_FORMAT.md) section 4 (hypothesis until S9-13) and the minimum alignment enforced here.
  No upper limit is set by the profile; the free-space check and the file system bound the size.
* `flags` MUST be 0, otherwise `INVALID_ARGUMENT`.
* Precedence of failures: `out_error` (no write) and `flags`/null `name`, then name, capacity, `dir_fd`, free space,
  creation.
* **Free space** is checked first: the unprivileged available bytes of the volume holding `dir_fd`
  (`fstatvfs`, `f_bavail * f_frsize`). Less than `capacity_bytes` is `INSUFFICIENT_SPACE`, nothing is created, and
  `out_error.required_bytes` and `out_error.available_bytes` carry the two numbers. The image is a sparse file, so
  this is a conservative guard against creating a disk the volume could never hold in full; it is not a reservation
  (another writer can fill the volume afterwards) and not a promise that writes will not fail. macOS reports the block
  count as a 32-bit value, so available space saturates at `u32::MAX` fragments (about 16 TiB with 4 KiB fragments).
  A failing `fstatvfs` is `IO_ERROR` with its `errno`. Injection for tests is internal (the `od-host` unit tests pass
  a probe); the C smoke gets a real `INSUFFICIENT_SPACE` with a capacity no volume has.
* **Existing entry:** an entry of any kind, a symlink (dangling or not) included, is `ALREADY_EXISTS`
  (`os_errno` `EEXIST`). It is neither followed, opened nor modified; a file with this name is never overwritten.
* **Partial file:** if a step after the file was created fails, the entry is removed with `unlinkat` **only if** `name`
  still resolves (without following symlinks) to the object this call created (same `st_dev`/`st_ino`). An entry
  replaced meanwhile is never removed. The check and the removal are two calls, so a replacement inside that window is
  not detected. Rollback runs after the failed writer's lock was released and before `unlinkat`; in that window another
  process can open the same inode by path and take the lock. Its later write then ends with `WriterIdentityLost` once
  the entry is gone, so no foreign data is lost. Exceptions that leave the entry in place: `WRITER_BUSY` (another open
  file description took the lock on the new file first), and a failing `fstat` of the just created descriptor: the
  identity of the created object is then unknown, so nothing is removed and `IO_ERROR` is returned (residual risk: an
  empty file may stay). The directory is duplicated before the entry is created, so `EMFILE` there leaves nothing
  behind. A crash of the process mid-call can leave an empty or partly sized file; this is not handled.
* After success no lock is held: the descriptor and its `flock` are released before the call returns. Lock ownership
  of the running VM is VMHost's business and is not part of this ABI.

## `od_storage_inspect_v1`

`od_storage_inspect_v1(dir_fd, name, name_len, out_info)`

Reports the RAW image `<dir_fd>/<name>`. It opens the entry read-only with `O_NOFOLLOW | O_NONBLOCK` (a FIFO is never
waited on), checks with `fstat` that it is a regular file and reads **no image bytes**, only the size.

* A symlink (any target, dangling or not), directory, FIFO or device is `NOT_REGULAR_FILE`; a missing entry is
  `NOT_FOUND` (`os_errno` `ENOENT`). A hard link to a regular file is a regular file and is accepted.
* Input `expected_size_bytes`: 0 means none. Otherwise a smaller object is `TRUNCATED` and a larger one
  `SIZE_MISMATCH`, and `size_bytes` carries the actual size. Without an expected size only the geometry is checked:
  size zero or not a multiple of 512 is `INVALID_IMAGE` (`size_bytes` carries the size). **A RAW image has no header,
  so a file cut at a block boundary cannot be told from a smaller image without `expected_size_bytes`;** the caller
  keeps the expected capacity (the bundle config has no capacity field today, so the app compares with what the
  wizard created).
* On `OK`: `format = OD_STORAGE_FORMAT_RAW_V1`, `size_bytes` = object size. In every other case `format` is 0.
* Inspect takes **no lock** and is not a lease: a writer may be active, and the size can change right after the call.
  It does not fail with `WRITER_BUSY`. `reserved` in the struct is an input and MUST be 0.

## Statuses (closed list)

| Value | Name | Meaning |
|---:|---|---|
| 0 | `OK` | Done. For create: completion only, not durable. |
| 1 | `INVALID_ARGUMENT` | NULL or bad pointer/struct, nonzero `flags` or `reserved`, bad capacity, `dir_fd` not an open directory. |
| 2 | `INVALID_NAME` | Name rules above. |
| 3 | `ALREADY_EXISTS` | create: the entry exists (any kind). |
| 4 | `NOT_FOUND` | inspect: no such entry. |
| 5 | `NOT_REGULAR_FILE` | inspect: symlink, directory, FIFO, device. |
| 6 | `INSUFFICIENT_SPACE` | create: volume has less than `capacity_bytes` available; both numbers in `out_error`. |
| 7 | `WRITER_BUSY` | create: another open file description locked the new file first (the entry is kept). |
| 8 | `INVALID_IMAGE` | inspect: size zero or not a multiple of 512. |
| 9 | `TRUNCATED` | inspect: smaller than `expected_size_bytes`. |
| 10 | `SIZE_MISMATCH` | inspect: larger than `expected_size_bytes`. |
| 11 | `IO_ERROR` | Any other failure; `os_errno` carries the `errno` when there is one (0 otherwise, for example when the entry was replaced between creation and locking). |

A value outside this list is never returned. Mapping these to the control v2 error codes and to user texts belongs to
S11-03 and S11-09.

## Not covered

* The `lock` file of the bundle and the VMHost lock: [VM_BUNDLE.md](VM_BUNDLE.md) section 2.1 (S11-04, S11-06).
* Opening an existing image for the running VM (a writable, locked, no-symlink open by `dir_fd`): not part of this ABI.
* CoW images, snapshots, resize, convert, delete, rename.
* Behaviour on network and non-APFS volumes, power loss, and a volume that does not support sparse files: not
  checked.
* Known low-risk limits, not changed: the free-space figure uses the 32-bit `f_bavail` of macOS `statvfs` (saturates);
  inspect opens a character device `O_RDONLY | O_NONBLOCK` before `fstat` refuses it (side effects of opening some
  devices are not excluded).
