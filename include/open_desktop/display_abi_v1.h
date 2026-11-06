#ifndef OPEN_DESKTOP_DISPLAY_ABI_V1_H
#define OPEN_DESKTOP_DISPLAY_ABI_V1_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opt-in display session ABI, version 1. Independent of host_abi.h and
 * session_abi_v1.h, which it does not extend. The contract (ownership,
 * threads, status precedence, extension rules) is
 * docs/specs/DISPLAY_HOST_ABI_V1.md. This header holds the C declarations and
 * numeric constants; crates/od-host/src/display.rs repeats the codes by hand
 * and checks only struct sizes, alignments and field offsets against it
 * automatically. */

/* Exact-match identifier of this ABI; not an arithmetic encoding. Every struct
 * below starts with abi_version, then struct_size. */
#define OD_DISPLAY_ABI_VERSION_V1 UINT32_C(0x00010000)

#define OD_DISPLAY_SESSION_CONFIG_SIZE_V1 16
#define OD_DISPLAY_MODE_SIZE_V1 48
#define OD_DISPLAY_FRAME_INFO_SIZE_V1 48
#define OD_DISPLAY_STEP_REPORT_SIZE_V1 24

/* Functions return one of these as uint32_t (not as a C enum). Values are
 * fixed; new codes are appended, existing ones are never changed or reused. */
enum od_display_status {
    OD_DISPLAY_STATUS_OK = 0,
    OD_DISPLAY_STATUS_INVALID_ARGUMENT = 1,   /* NULL/misaligned pointer, nonzero reserved, non-NULL out handle slot, NULL handle */
    OD_DISPLAY_STATUS_ABI_MISMATCH = 2,       /* abi_version or struct_size differs from this header */
    OD_DISPLAY_STATUS_UNSUPPORTED_PROFILE = 3,
    OD_DISPLAY_STATUS_INVALID_HANDLE = 4,     /* unknown, forged or destroyed handle */
    OD_DISPLAY_STATUS_BUSY = 5,               /* another call on this handle is in progress */
    OD_DISPLAY_STATUS_NO_ACTIVE_MODE = 6,     /* the guest has not committed a mode */
    OD_DISPLAY_STATUS_NO_FRAME = 7,           /* mode active, but no frame since the last commit */
    OD_DISPLAY_STATUS_BUFFER_LENGTH = 8,      /* buffer_len != stride * height */
    OD_DISPLAY_STATUS_MODE_CHANGED = 9,       /* the mode changed while the frame was copied */
    OD_DISPLAY_STATUS_MEMORY_ERROR = 10,      /* framebuffer memory could not be read */
    OD_DISPLAY_STATUS_INTERNAL_ERROR = 11     /* host fault (e.g. a caught Rust panic); session unusable except destroy */
};

/* Session profiles. Existing values never change; new profiles are appended
 * with new values (S10-08 adds 2). A consumer rejects values it does not know. */
enum od_display_profile {
    /* TEST-ONLY. Machine with a display device and no guest code. create
     * programs mode 64x48 BGRX8888 (stride 256) and draws the `bars` seed 0
     * pattern (tests/fixtures/guest/stage10/bars_64x48_seed0.raw) through the
     * device's own registers and memory; generation is 1 right after create.
     * step has nothing to execute. This profile never changes. */
    OD_DISPLAY_PROFILE_TEST_PATTERN = 1
};

/* Pixel formats of od_display_mode.format / od_display_frame_info.format
 * (same numbering as DISPLAY_DEVICE.md FORMAT). */
enum od_display_format {
    OD_DISPLAY_FORMAT_BGRX8888 = 1 /* byte 0 B, 1 G, 2 R, 3 X (ignored, opaque) */
};

/* od_display_mode.flags bits; other bits are reserved and read as zero. */
#define OD_DISPLAY_MODE_FLAG_ACTIVE UINT32_C(0x1)    /* a mode is committed */
#define OD_DISPLAY_MODE_FLAG_HAS_FRAME UINT32_C(0x2) /* a frame exists for the current mode */

/* od_display_step_report.outcome. Values are appended by profiles. */
enum od_display_step_outcome {
    OD_DISPLAY_STEP_NOTHING_TO_EXECUTE = 1 /* profile has no guest; budget unused */
};

typedef struct od_display_session od_display_session; /* opaque */

/* Native C layout on the supported 64-bit macOS host ABI; all fields have
 * fixed width and there is no implicit padding. Reserved fields MUST be zero
 * on input and are written zero on output. This is not a serialized format. */

/* size 16, align 4: abi_version 0, struct_size 4, profile 8, reserved 12 */
typedef struct od_display_session_config {
    uint32_t abi_version;
    uint32_t struct_size;
    uint32_t profile; /* enum od_display_profile */
    uint32_t reserved;
} od_display_session_config;

/* size 48, align 8: abi_version 0, struct_size 4, width 8, height 12,
 * stride 16, format 20, flags 24, reserved 28, generation 32,
 * mode_change_count 40. All zero (flags 0) while no mode is active. */
typedef struct od_display_mode {
    uint32_t abi_version;
    uint32_t struct_size;
    uint32_t width;
    uint32_t height;
    uint32_t stride; /* bytes per scanline; width * 4, no padding in v1 */
    uint32_t format; /* enum od_display_format */
    uint32_t flags;
    uint32_t reserved;
    uint64_t generation;
    uint64_t mode_change_count;
} od_display_mode;

/* size 48, align 8: abi_version 0, struct_size 4, width 8, height 12,
 * stride 16, format 20, reserved0 24, reserved1 28, generation 32,
 * mode_change_count 40. Describes the frame just copied. */
typedef struct od_display_frame_info {
    uint32_t abi_version;
    uint32_t struct_size;
    uint32_t width;
    uint32_t height;
    uint32_t stride;
    uint32_t format;
    uint32_t reserved0;
    uint32_t reserved1;
    uint64_t generation;
    uint64_t mode_change_count;
} od_display_frame_info;

/* size 24, align 8: abi_version 0, struct_size 4, outcome 8, reserved 12,
 * instructions_executed 16. */
typedef struct od_display_step_report {
    uint32_t abi_version;
    uint32_t struct_size;
    uint32_t outcome; /* enum od_display_step_outcome */
    uint32_t reserved;
    uint64_t instructions_executed;
} od_display_step_report;

/* Creates a session. *out_session MUST be NULL on entry (otherwise
 * INVALID_ARGUMENT, untouched); it is written only on OK. Success transfers one
 * handle that the caller must destroy exactly once. Validation order: NULL or
 * misaligned pointers; *out_session not NULL; config overlapping out_session
 * (all INVALID_ARGUMENT); ABI header (ABI_MISMATCH); reserved != 0
 * (INVALID_ARGUMENT); profile (UNSUPPORTED_PROFILE). */
uint32_t od_display_session_create(const od_display_session_config *config,
                                   od_display_session **out_session);

/* Releases the session and everything it owns. After OK the handle is invalid;
 * a repeated destroy returns INVALID_HANDLE, a NULL handle INVALID_ARGUMENT.
 * Returns BUSY, leaving the session alive, if another call is in progress. */
uint32_t od_display_session_destroy(od_display_session *session);

/* Copies the current mode and counters. The caller sets out_mode->abi_version
 * and ->struct_size to the V1 constants before the call (other fields are
 * ignored); on any nonzero status the struct is untouched. */
uint32_t od_display_session_mode(od_display_session *session,
                                 od_display_mode *out_mode);

/* Copies the current frame, exactly stride * height bytes, into buffer. The
 * caller owns buffer for the duration of the call only; it MUST NOT overlap
 * out_info. buffer_len must equal stride * height of the current mode
 * (read it with od_display_session_mode first); otherwise BUFFER_LENGTH and
 * buffer is not touched. After other nonzero statuses buffer contents are
 * unspecified. out_info is set up as for od_display_session_mode and is
 * written only on OK. Allowed only while no step is executing. */
uint32_t od_display_session_read_frame(od_display_session *session,
                                       uint8_t *buffer, size_t buffer_len,
                                       od_display_frame_info *out_info);

/* Executes at most `budget` guest instructions of the profile. Profile
 * TEST_PATTERN has no guest: returns OK with outcome NOTHING_TO_EXECUTE and
 * instructions_executed 0 for any budget. out_report is set up as above and is
 * written only on OK. Not concurrent with read_frame on the same handle. */
uint32_t od_display_session_step(od_display_session *session, uint64_t budget,
                                 od_display_step_report *out_report);

#ifdef __cplusplus
}
#endif

#endif /* OPEN_DESKTOP_DISPLAY_ABI_V1_H */
