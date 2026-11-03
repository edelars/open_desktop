#ifndef OPEN_DESKTOP_SESSION_ABI_V1_H
#define OPEN_DESKTOP_SESSION_ABI_V1_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opt-in design contract; declarations do not imply runtime availability.
 * See docs/specs/HOST_SESSION_ABI_V1.md. This does not extend host_abi.h. */
#define OD_SESSION_ABI_VERSION_V1 UINT32_C(0x00010000)
#define OD_SESSION_CAP_S0_09_SAME_CODE_V1 UINT64_C(1)
#define OD_SESSION_MAX_CODE_BYTES_V1 4096
#define OD_SESSION_COMPLETION_BYTE_V1 1

#define OD_SESSION_STATUS_OK_V1 0
#define OD_SESSION_STATUS_INVALID_ARGUMENT_V1 1
#define OD_SESSION_STATUS_INVALID_CODE_V1 2
#define OD_SESSION_STATUS_BUSY_V1 3
#define OD_SESSION_STATUS_UNSUPPORTED_HOST_V1 4
#define OD_SESSION_STATUS_ALLOCATION_FAILED_V1 5
#define OD_SESSION_STATUS_INVALID_STATE_V1 6
#define OD_SESSION_STATUS_INVALID_REQUEST_ID_V1 7
#define OD_SESSION_STATUS_CODE_MISMATCH_V1 8
#define OD_SESSION_STATUS_NO_ACTIVE_RUN_V1 9

#define OD_SESSION_EXIT_CODE_END_V1 0
#define OD_SESSION_EXIT_INSTRUCTION_BUDGET_V1 1
#define OD_SESSION_EXIT_STOP_REQUESTED_V1 2

#define OD_SESSION_CPU_STATE_SIZE_V1 48
#define OD_SESSION_CREATE_OPTIONS_SIZE_V1 4112
#define OD_SESSION_RUN_REQUEST_SIZE_V1 4168
#define OD_SESSION_RUN_TOKEN_SIZE_V1 16
#define OD_SESSION_RUN_RESULT_SIZE_V1 72

typedef struct od_session_proxy_v1 *od_session_handle_v1;

/* All layouts use native C layout on the supported 64-bit host ABI.
 * CPU/options/request/result alignment is 8; token alignment is 4.
 * Reserved fields and unused code array bytes MUST be zero on input. */
typedef struct od_session_cpu_state_v1 {
    uint64_t gpr[4]; /* RAX, RCX, RDX, RBX */
    uint64_t rip;    /* byte offset in immutable code */
    uint64_t rflags; /* raw; CF/PF/AF/ZF/SF/OF are modeled */
} od_session_cpu_state_v1;

typedef struct od_session_create_options_v1 {
    uint32_t struct_size;
    uint32_t code_length;
    uint64_t first_entry_rip; /* validation only; no CPU state is retained */
    uint8_t code[OD_SESSION_MAX_CODE_BYTES_V1];
} od_session_create_options_v1;

typedef struct od_session_run_request_v1 {
    uint32_t struct_size;
    uint32_t request_id;
    uint64_t instruction_budget;
    od_session_cpu_state_v1 initial_state;
    uint32_t code_length;
    uint32_t reserved;
    uint8_t code[OD_SESSION_MAX_CODE_BYTES_V1];
} od_session_run_request_v1;

typedef struct od_session_run_token_v1 {
    uint32_t struct_size;
    uint32_t request_id;
    int32_t completion_read_fd; /* caller-owned fresh CLOEXEC pipe read end */
    uint32_t reserved;
} od_session_run_token_v1;

typedef struct od_session_run_result_v1 {
    uint32_t struct_size;
    uint32_t request_id;
    od_session_cpu_state_v1 state;
    uint32_t reason;
    uint32_t reserved;
    uint64_t instructions_executed;
} od_session_run_result_v1;

/* Separate namespace from od_host_abi_version() and control protocol version. */
uint32_t od_session_abi_version(void);
uint64_t od_session_abi_capabilities(void);

/* Validates code and first RIP before acquiring a lease; starts and initializes
 * the dedicated owner. Caller initializes *out_handle to NULL. On failure it
 * remains NULL. Success transfers the proxy handle, never the Rust Session. */
uint32_t od_session_create_v1(const od_session_create_options_v1 *options,
                              uint32_t options_size,
                              od_session_handle_v1 *out_handle);

/* Copies request, reserves fresh control/STOP and enqueues one invocation.
 * Success transfers a fresh pipe read fd in out_token. No future-RUN queue.
 * Nonzero status leaves all out_token bytes untouched. */
uint32_t od_session_begin_run_v1(od_session_handle_v1 handle,
                                 const od_session_run_request_v1 *request,
                                 uint32_t request_size,
                                 od_session_run_token_v1 *out_token,
                                 uint32_t out_token_size);

/* Single consumer, synchronous until execution AND notification have finished.
 * May be called before readiness; it does not read or close the caller's fd.
 * Consumes a matching invocation even on terminal status 1..6; output is written
 * only on OK. A second concurrent waiter returns BUSY without consuming it. */
uint32_t od_session_wait_run_v1(od_session_handle_v1 handle,
                                uint32_t request_id,
                                od_session_run_result_v1 *out_result,
                                uint32_t out_result_size);

/* Concurrent with begin/wait. Matching active ID latches invocation-local STOP;
 * idle/completed returns NO_ACTIVE_RUN. No STOP is retained for a future RUN. */
uint32_t od_session_request_stop_v1(od_session_handle_v1 handle,
                                    uint32_t request_id);

/* Exclude new calls and synchronize other callers first. BUSY until the run is
 * consumed and wait has returned. Success drops Session on its owner, joins
 * that thread and invalidates the handle. Caller still owns returned read fds. */
uint32_t od_session_destroy_v1(od_session_handle_v1 handle);

#ifdef __cplusplus
}
#endif

#endif /* OPEN_DESKTOP_SESSION_ABI_V1_H */
