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

/* Exact-match identifier of the cursor extension (S10-14, DISPLAY_DEVICE.md
 * 15.10-15.11). Only the cursor structs below carry it; every v1 symbol keeps
 * V1. The header file keeps its name: the extension adds symbols and does not
 * change any v1 value. */
#define OD_DISPLAY_ABI_VERSION_V2 UINT32_C(0x00020000)

#define OD_DISPLAY_SESSION_CONFIG_SIZE_V1 16
#define OD_DISPLAY_MODE_SIZE_V1 48
#define OD_DISPLAY_FRAME_INFO_SIZE_V1 48
#define OD_DISPLAY_STEP_REPORT_SIZE_V1 24
#define OD_DISPLAY_CURSOR_STATE_SIZE_V2 64

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
    OD_DISPLAY_STATUS_INTERNAL_ERROR = 11,    /* host fault (e.g. a caught Rust panic); session unusable except destroy */
    OD_DISPLAY_STATUS_NO_CURSOR_SHAPE = 12,   /* (cursor) the guest has not published a shape since create/reset */
    OD_DISPLAY_STATUS_CURSOR_BUFFER_LENGTH = 13, /* (cursor) buffer_len != width * height * 4 */
    OD_DISPLAY_STATUS_NO_SUCH_SCREEN = 14     /* (screen index) screen >= screen count */
};

/* Session profiles. Existing values never change; new profiles are appended
 * with new values (S10-08 adds 2). A consumer rejects values it does not know. */
enum od_display_profile {
    /* TEST-ONLY. Machine with a display device and no guest code. create
     * programs mode 64x48 BGRX8888 (stride 256) and draws the `bars` seed 0
     * pattern (tests/fixtures/guest/stage10/bars_64x48_seed0.raw) through the
     * device's own registers and memory; generation is 1 right after create.
     * step has nothing to execute. This profile never changes. */
    OD_DISPLAY_PROFILE_TEST_PATTERN = 1,
    /* S10-08. Guest probe: own x86-64 code (tests/fixtures/guest/stage10/
     * display_probe_bars_64x48_seed0.img, built by tools/build/
     * build_stage10_display_probe.py) on the firmware board. create only loads
     * the image: there is no mode and no frame until od_display_session_step
     * runs the guest. The guest programs 64x48 BGRX8888 through the registers,
     * stores the `bars` seed 0 pattern into framebuffer memory and rings the
     * FRAME_GENERATION doorbell, then executes HLT (25 983 instructions);
     * generation is then 1 and the frame equals bars_64x48_seed0.raw. */
    OD_DISPLAY_PROFILE_STAGE10_PROBE = 2,
    /* TEST-ONLY. Two screens (DISPLAY_DEVICE.md 16.2). Screen 0 is the profile-1
     * bars frame; screen 1 is a 32x24 BGRX8888 checkerboard (8x8 cells, black
     * at cell (0,0)) programmed through its mirror registers at create. */
    OD_DISPLAY_PROFILE_TWO_SCREENS_TEST = 4
};

/* Pixel formats of od_display_mode.format / od_display_frame_info.format
 * (same numbering as DISPLAY_DEVICE.md FORMAT). */
enum od_display_format {
    OD_DISPLAY_FORMAT_BGRX8888 = 1 /* byte 0 B, 1 G, 2 R, 3 X (ignored, opaque) */
};

/* od_display_mode.flags bits; other bits are reserved and read as zero. */
#define OD_DISPLAY_MODE_FLAG_ACTIVE UINT32_C(0x1)    /* a mode is committed */
#define OD_DISPLAY_MODE_FLAG_HAS_FRAME UINT32_C(0x2) /* a frame exists for the current mode */
#define OD_DISPLAY_MODE_FLAG_DISABLED UINT32_C(0x4) /* screen 1 only: disabled by the guest (DISPLAY_DEVICE.md 16.6) */

/* od_display_step_report.outcome. Values are appended by profiles. */
enum od_display_step_outcome {
    OD_DISPLAY_STEP_NOTHING_TO_EXECUTE = 1, /* profile has no guest; budget unused */
    OD_DISPLAY_STEP_BUDGET_EXHAUSTED = 2,   /* (STAGE10_PROBE) the guest is still running; instructions_executed == budget, or 0 for budget 0 */
    OD_DISPLAY_STEP_HALTED = 3,             /* (STAGE10_PROBE) the guest executed HLT; final: later steps execute nothing */
    OD_DISPLAY_STEP_FAILED = 4              /* (STAGE10_PROBE) the guest ended abnormally (fault, unsupported opcode or access); final */
};

/* od_display_cursor_state.flags bits; other bits are reserved and read as zero. */
#define OD_DISPLAY_CURSOR_FLAG_ENABLED UINT32_C(0x1)   /* CURSOR_ENABLE = 1: the cursor is shown */
#define OD_DISPLAY_CURSOR_FLAG_HAS_SHAPE UINT32_C(0x2) /* a shape is published; shape_seq, size, format, hotspot are set */

/* od_display_cursor_state.format: same numbering as DISPLAY_DEVICE.md 15.5. */
enum od_display_cursor_format {
    OD_DISPLAY_CURSOR_FORMAT_BGRA8888_PREMUL = 2 /* byte 0 B, 1 G, 2 R, 3 A; colour premultiplied by alpha */
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

/* Cursor state (ABI V2), size 64, align 8: abi_version 0, struct_size 4,
 * cursor_seq 8, shape_seq 16, flags 24, width 28, height 32, format 36,
 * hot_x 40, hot_y 44, pos_x 48, pos_y 52, reserved0 56, reserved1 60.
 * abi_version is OD_DISPLAY_ABI_VERSION_V2, struct_size
 * OD_DISPLAY_CURSOR_STATE_SIZE_V2. Without a shape (HAS_SHAPE clear) shape_seq,
 * width, height, format, hot_x and hot_y are 0. pos_x/pos_y are the hotspot in
 * frame pixels. */
typedef struct od_display_cursor_state {
    uint32_t abi_version;
    uint32_t struct_size;
    uint64_t cursor_seq; /* changes exactly when shape, position or visibility changes; 0 on a fresh device */
    uint64_t shape_seq;  /* one value always means the same image bytes; 0 = no shape */
    uint32_t flags;
    uint32_t width;
    uint32_t height;
    uint32_t format; /* enum od_display_cursor_format */
    uint32_t hot_x;
    uint32_t hot_y;
    uint32_t pos_x;
    uint32_t pos_y;
    uint32_t reserved0;
    uint32_t reserved1;
} od_display_cursor_state;

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
 * instructions_executed 0 for any budget. Profile STAGE10_PROBE resumes where
 * the previous step stopped and returns OK with outcome BUDGET_EXHAUSTED,
 * HALTED or FAILED; a guest failure is an outcome, not a status. out_report is set up as above and is
 * written only on OK. Not concurrent with read_frame on the same handle. */
uint32_t od_display_session_step(od_display_session *session, uint64_t budget,
                                 od_display_step_report *out_report);

/* Cursor state (S10-14). The caller sets out_state->abi_version and
 * ->struct_size to the V2 constants before the call (other fields are
 * ignored); on any nonzero status the struct is untouched. Read-only: neither
 * registers nor counters of the device change. Same rules as step/read_frame:
 * between quanta, one caller per handle. Profile TEST_PATTERN reports no shape
 * and cursor_seq 0 until something writes the cursor registers. */
uint32_t od_display_session_cursor_state(od_display_session *session,
                                         od_display_cursor_state *out_state);

/* Copies the published cursor image, exactly width * height * 4 bytes of
 * BGRA8888_PREMUL, rows packed, into buffer (at most 16384 bytes for the
 * 64x64 limit), and fills out_state with the cursor state of the same moment
 * (shape_seq matches the bytes). buffer_len must equal the image size (read
 * the size with od_display_session_cursor_state first); otherwise
 * CURSOR_BUFFER_LENGTH and neither buffer nor out_state is touched. Without a
 * shape: NO_CURSOR_SHAPE, same. buffer MUST NOT overlap out_state. Validation
 * order: NULL or misaligned pointers and overlap (INVALID_ARGUMENT); out_state
 * header (ABI_MISMATCH); handle (INVALID_HANDLE, BUSY, INTERNAL_ERROR); shape;
 * length. After a nonzero status out_state is untouched and so is buffer,
 * except INTERNAL_ERROR after a panic inside the copy (buffer contents are then
 * unspecified, as for read_frame). */
uint32_t od_display_session_read_cursor_image(od_display_session *session,
                                              uint8_t *buffer, size_t buffer_len,
                                              od_display_cursor_state *out_state);

/* Screens (DISPLAY_DEVICE.md 16.10). Functions without a screen index above
 * describe screen 0 and are unchanged. Screen index 0 or 1; 1 only when the
 * machine has two screens. */

/* Mode, flags and counters of one screen. 48 bytes. flags: bit 0 ACTIVE,
 * bit 1 HAS_FRAME (current frame of the mode), bit 2 DISABLED (screen 1 only;
 * never set for screen 0). */
typedef struct od_display_screen_mode {
    uint32_t abi_version;
    uint32_t struct_size;
    uint32_t width;
    uint32_t height;
    uint32_t stride;
    uint32_t format;
    uint32_t flags;
    uint32_t reserved;
    uint64_t generation;
    uint64_t mode_change_count;
} od_display_screen_mode;

/* Number of screens: 1 or 2. */
uint32_t od_display_session_screen_count(od_display_session *session, uint32_t *out_count);

/* Mode and counters of screen `screen`. NO_SUCH_SCREEN for a missing screen. */
uint32_t od_display_session_screen_mode(od_display_session *session, uint32_t screen,
                                        od_display_screen_mode *out_mode);

/* Current frame of screen `screen`: same rules and statuses as
 * od_display_session_read_frame, with NO_SUCH_SCREEN added. */
uint32_t od_display_session_read_screen_frame(od_display_session *session, uint32_t screen,
                                              uint8_t *buffer, size_t buffer_len,
                                              od_display_frame_info *out_info);

#ifdef __cplusplus
}
#endif

#endif /* OPEN_DESKTOP_DISPLAY_ABI_V1_H */
