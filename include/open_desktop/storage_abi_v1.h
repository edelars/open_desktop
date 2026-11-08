#ifndef OPEN_DESKTOP_STORAGE_ABI_V1_H
#define OPEN_DESKTOP_STORAGE_ABI_V1_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Create and inspect one RAW disk image inside an open directory (macOS only).
 * Contract: docs/specs/STORAGE_HOST_ABI_V1.md. Linked only by VMHost.
 * Creation is completion only: no durability of the file or of its directory
 * entry is claimed or requested (ISSUE-055, S9-06). */
#define OD_STORAGE_ABI_VERSION_V1 UINT32_C(0x00010000)

/* Entry name: 1..249 bytes (disks/<name> fits the 255-byte config value). */
#define OD_STORAGE_MAX_NAME_BYTES_V1 249
/* Capacity must be a nonzero multiple of this (RAW stores no block size). */
#define OD_STORAGE_RAW_BLOCK_BYTES_V1 512

#define OD_STORAGE_FORMAT_RAW_V1 1

#define OD_STORAGE_STATUS_OK_V1 0
#define OD_STORAGE_STATUS_INVALID_ARGUMENT_V1 1
#define OD_STORAGE_STATUS_INVALID_NAME_V1 2
#define OD_STORAGE_STATUS_ALREADY_EXISTS_V1 3
#define OD_STORAGE_STATUS_NOT_FOUND_V1 4
#define OD_STORAGE_STATUS_NOT_REGULAR_FILE_V1 5
#define OD_STORAGE_STATUS_INSUFFICIENT_SPACE_V1 6
#define OD_STORAGE_STATUS_WRITER_BUSY_V1 7
#define OD_STORAGE_STATUS_INVALID_IMAGE_V1 8
#define OD_STORAGE_STATUS_TRUNCATED_V1 9
#define OD_STORAGE_STATUS_SIZE_MISMATCH_V1 10
#define OD_STORAGE_STATUS_IO_ERROR_V1 11

#define OD_STORAGE_ERROR_SIZE_V1 32
#define OD_STORAGE_IMAGE_INFO_SIZE_V1 32

/* Native C layout on the supported 64-bit macOS ABI; both structs are
 * 32 bytes, alignment 8. Not a serialized format. */
typedef struct od_storage_error_v1 {
    uint32_t struct_size;      /* in: MUST be OD_STORAGE_ERROR_SIZE_V1 */
    int32_t os_errno;          /* out: errno of the failing call, else 0 */
    uint64_t required_bytes;   /* out: INSUFFICIENT_SPACE only, else 0 */
    uint64_t available_bytes;  /* out: INSUFFICIENT_SPACE only, else 0 */
    uint64_t reserved;         /* out: 0 */
} od_storage_error_v1;

typedef struct od_storage_image_info_v1 {
    uint32_t struct_size;          /* in: MUST be OD_STORAGE_IMAGE_INFO_SIZE_V1 */
    uint32_t format;               /* out: OD_STORAGE_FORMAT_RAW_V1 on OK, else 0 */
    uint64_t expected_size_bytes;  /* in: 0 = none; else the exact size required */
    uint64_t size_bytes;           /* out: object size when determined, else 0 */
    int32_t os_errno;              /* out: errno of the failing call, else 0 */
    uint32_t reserved;             /* in: MUST be 0; not written */
} od_storage_image_info_v1;

/* Exact-match identifier, separate from every other ABI version. */
uint32_t od_storage_abi_version(void);

/* Creates <dir_fd>/<name>: a regular file of exactly capacity_bytes zero bytes,
 * mode 0600, that did not exist (existing entry of any kind, symlinks
 * included, is ALREADY_EXISTS and is never touched or followed). Checks the
 * volume's available bytes first: less than capacity_bytes is
 * INSUFFICIENT_SPACE with both numbers in out_error. On a failure after the
 * entry was created, the entry is removed only if it still is the object this
 * call created. name: name_len bytes of UTF-8, not NUL-terminated, 1..249
 * bytes, not "." or "..", no '/', no U+0000..U+001F, no U+007F, otherwise
 * INVALID_NAME. flags MUST be 0. dir_fd is borrowed (not closed, not
 * retained); it must be an open directory. out_error MUST be non-NULL with
 * struct_size set; it is fully written on every return once struct_size was
 * accepted, and untouched otherwise. */
uint32_t od_storage_create_raw_v1(int32_t dir_fd,
                                  const uint8_t *name,
                                  uint32_t name_len,
                                  uint64_t capacity_bytes,
                                  uint32_t flags,
                                  od_storage_error_v1 *out_error);

/* Reports the RAW image <dir_fd>/<name> without following a symlink and
 * without taking a lock (not a lease: a writer may be active). Reads no image
 * bytes. A non-regular entry (symlink, directory, FIFO, device) is
 * NOT_REGULAR_FILE. With expected_size_bytes != 0 a smaller object is
 * TRUNCATED and a larger one SIZE_MISMATCH; otherwise a size that is zero or
 * not a multiple of OD_STORAGE_RAW_BLOCK_BYTES_V1 is INVALID_IMAGE. Name rules
 * and dir_fd as for create. out_info MUST be non-NULL with struct_size set and
 * reserved zero; it is written (outputs only) on every return once its inputs
 * were accepted, and untouched otherwise. */
uint32_t od_storage_inspect_v1(int32_t dir_fd,
                               const uint8_t *name,
                               uint32_t name_len,
                               od_storage_image_info_v1 *out_info);

#ifdef __cplusplus
}
#endif

#endif
