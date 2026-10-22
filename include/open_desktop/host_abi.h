#ifndef OPEN_DESKTOP_HOST_ABI_H
#define OPEN_DESKTOP_HOST_ABI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define OD_HOST_ABI_VERSION_V0 0x00000001
#define OD_FRAGMENT_MAX_LENGTH_V0 4096

typedef void *od_fragment_handle;

typedef enum od_status_v0 {
    OD_STATUS_OK = 0,
    OD_STATUS_INVALID_ARGUMENT = 1,
    OD_STATUS_INVALID_CODE = 2,
    OD_STATUS_BUSY = 3,
    OD_STATUS_UNSUPPORTED_HOST = 4,
    OD_STATUS_ALLOCATION_FAILED = 5,
    OD_STATUS_INVALID_STATE = 6
} od_status_v0;

typedef enum od_exit_reason_v0 {
    OD_EXIT_CODE_END = 0,
    OD_EXIT_INSTRUCTION_BUDGET = 1,
    OD_EXIT_STOP_REQUESTED = 2
} od_exit_reason_v0;

typedef struct od_cpu_state_v0 {
    uint64_t gpr[4]; /* RAX, RCX, RDX, RBX */
    uint64_t rip;    /* byte offset in the immutable code fragment */
    uint64_t rflags; /* only CF/PF/AF/ZF/SF/OF are modeled */
} od_cpu_state_v0;

typedef struct od_execution_result_v0 {
    od_cpu_state_v0 state;
    uint32_t reason;
    uint32_t reserved;
    uint64_t instructions_executed;
} od_execution_result_v0;

/* Monotonic identifier for this ABI draft; currently v0 id 1. */
uint32_t od_host_abi_version(void);

/* Copies and validates code. Caller owns *out_handle after OD_STATUS_OK. */
uint32_t od_fragment_create(const uint8_t *code,
                            size_t code_length,
                            od_fragment_handle *out_handle);

/* Synchronous run. od_fragment_request_stop may run concurrently on another
 * thread and is observed at guest instruction boundaries. */
uint32_t od_fragment_execute(od_fragment_handle handle,
                             const od_cpu_state_v0 *initial_state,
                             uint64_t instruction_budget,
                             od_execution_result_v0 *out_result);

/* Requests cooperative stop. A pre-run request is retained for the next run;
 * an active run observes it at a guest instruction boundary. */
uint32_t od_fragment_request_stop(od_fragment_handle handle);

/* Call only after preventing new calls using this handle. Returns BUSY while
 * execution is active. A successfully destroyed handle must not be reused. */
uint32_t od_fragment_destroy(od_fragment_handle handle);

#ifdef __cplusplus
}
#endif

#endif /* OPEN_DESKTOP_HOST_ABI_H */
