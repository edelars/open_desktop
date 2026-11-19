#define _POSIX_C_SOURCE 200809L

#include "open_desktop/session_abi_v1.h"

#include <assert.h>
#include <fcntl.h>
#include <signal.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#define LAYOUT(type, size_constant, size, alignment)                   \
    _Static_assert(sizeof(type) == (size), #type " size changed");      \
    _Static_assert((size_constant) == (size), #size_constant " changed"); \
    _Static_assert(_Alignof(type) == (alignment), #type " alignment changed")
#define OFFSET(type, field, offset) \
    _Static_assert(offsetof(type, field) == (offset), #type "." #field " offset changed")

_Static_assert(sizeof(od_session_handle_v1) == 8, "session handle size changed");
LAYOUT(od_session_cpu_state_v1, OD_SESSION_CPU_STATE_SIZE_V1, 48, 8);
OFFSET(od_session_cpu_state_v1, gpr, 0);
OFFSET(od_session_cpu_state_v1, rip, 32);
OFFSET(od_session_cpu_state_v1, rflags, 40);
LAYOUT(od_session_create_options_v1, OD_SESSION_CREATE_OPTIONS_SIZE_V1, 4112, 8);
OFFSET(od_session_create_options_v1, struct_size, 0);
OFFSET(od_session_create_options_v1, code_length, 4);
OFFSET(od_session_create_options_v1, first_entry_rip, 8);
OFFSET(od_session_create_options_v1, code, 16);
LAYOUT(od_session_run_request_v1, OD_SESSION_RUN_REQUEST_SIZE_V1, 4168, 8);
OFFSET(od_session_run_request_v1, struct_size, 0);
OFFSET(od_session_run_request_v1, request_id, 4);
OFFSET(od_session_run_request_v1, instruction_budget, 8);
OFFSET(od_session_run_request_v1, initial_state, 16);
OFFSET(od_session_run_request_v1, code_length, 64);
OFFSET(od_session_run_request_v1, reserved, 68);
OFFSET(od_session_run_request_v1, code, 72);
LAYOUT(od_session_run_token_v1, OD_SESSION_RUN_TOKEN_SIZE_V1, 16, 4);
OFFSET(od_session_run_token_v1, struct_size, 0);
OFFSET(od_session_run_token_v1, request_id, 4);
OFFSET(od_session_run_token_v1, completion_read_fd, 8);
OFFSET(od_session_run_token_v1, reserved, 12);
LAYOUT(od_session_run_result_v1, OD_SESSION_RUN_RESULT_SIZE_V1, 72, 8);
OFFSET(od_session_run_result_v1, struct_size, 0);
OFFSET(od_session_run_result_v1, request_id, 4);
OFFSET(od_session_run_result_v1, state, 8);
OFFSET(od_session_run_result_v1, reason, 56);
OFFSET(od_session_run_result_v1, reserved, 60);
OFFSET(od_session_run_result_v1, instructions_executed, 64);

static const uint8_t code[] = {0x75, 0xFE}; /* JNZ -2 */

static od_session_handle_v1 create_session(void) {
    od_session_create_options_v1 options = {
        .struct_size = OD_SESSION_CREATE_OPTIONS_SIZE_V1,
        .code_length = sizeof(code),
    };
    memcpy(options.code, code, sizeof(code));
    od_session_handle_v1 handle = NULL;
    assert(od_session_create_v1(&options, sizeof(options), &handle) == OD_SESSION_STATUS_OK_V1);
    assert(handle != NULL);
    return handle;
}

static od_session_run_request_v1 make_request(uint32_t id, uint64_t budget, uint64_t flags) {
    od_session_run_request_v1 request = {
        .struct_size = OD_SESSION_RUN_REQUEST_SIZE_V1,
        .request_id = id,
        .instruction_budget = budget,
        .initial_state = {.gpr = {7, 11, 13, 17}, .rflags = flags},
        .code_length = sizeof(code),
    };
    memcpy(request.code, code, sizeof(code));
    return request;
}

static int begin(od_session_handle_v1 handle, const od_session_run_request_v1 *request) {
    od_session_run_token_v1 token;
    memset(&token, 0xA5, sizeof(token));
    assert(od_session_begin_run_v1(handle, request, sizeof(*request), &token, sizeof(token)) ==
           OD_SESSION_STATUS_OK_V1);
    assert(token.struct_size == OD_SESSION_RUN_TOKEN_SIZE_V1);
    assert(token.request_id == request->request_id && token.reserved == 0);
    int flags = fcntl(token.completion_read_fd, F_GETFD);
    assert(flags >= 0 && (flags & FD_CLOEXEC) != 0);
    return token.completion_read_fd;
}

static void assert_sentinel(const void *output, size_t size) {
    const unsigned char *bytes = output;
    for (size_t i = 0; i < size; ++i) {
        assert(bytes[i] == 0xA5);
    }
}

static void require_normal_notification(od_session_handle_v1 handle) {
    /* ZF set, so the branch falls through to the end after one instruction. */
    od_session_run_request_v1 request = make_request(1, 8, 0x42);
    int fd = begin(handle, &request);
    uint8_t notification[2];
    assert(read(fd, notification, sizeof(notification)) == 1);
    assert(notification[0] == OD_SESSION_COMPLETION_BYTE_V1);
    assert(read(fd, notification, sizeof(notification)) == 0);

    od_session_run_result_v1 result;
    memset(&result, 0xA5, sizeof(result));
    assert(od_session_wait_run_v1(handle, request.request_id, &result, sizeof(result)) == OD_SESSION_STATUS_OK_V1);
    assert(result.struct_size == OD_SESSION_RUN_RESULT_SIZE_V1);
    assert(result.request_id == request.request_id && result.reserved == 0);
    assert(result.reason == OD_SESSION_EXIT_CODE_END_V1 && result.instructions_executed == 1);
    assert(result.state.rip == sizeof(code) && result.state.rflags == request.initial_state.rflags);
    assert(memcmp(result.state.gpr, request.initial_state.gpr, sizeof(result.state.gpr)) == 0);
    assert(close(fd) == 0);
}

static void require_notification_failure(od_session_handle_v1 handle) {
    /* ZF clear and effectively unlimited budget, so STOP drives completion. */
    od_session_run_request_v1 request = make_request(2, UINT64_MAX, 2);
    int fd = begin(handle, &request);
    /* Fault injection only. A real caller keeps this descriptor open through wait. */
    assert(close(fd) == 0);
    puts("injecting lost notification reader before STOP");
    fflush(stdout);
    assert(od_session_request_stop_v1(handle, request.request_id) == OD_SESSION_STATUS_OK_V1);
    assert(od_session_destroy_v1(handle) == OD_SESSION_STATUS_BUSY_V1);

    od_session_run_result_v1 result;
    memset(&result, 0xA5, sizeof(result));
    assert(od_session_wait_run_v1(handle, request.request_id, &result, sizeof(result)) ==
           OD_SESSION_STATUS_INVALID_STATE_V1);
    assert_sentinel(&result, sizeof(result));

    request.request_id = 3;
    od_session_run_token_v1 token;
    memset(&token, 0xA5, sizeof(token));
    assert(od_session_begin_run_v1(handle, &request, sizeof(request), &token, sizeof(token)) ==
           OD_SESSION_STATUS_INVALID_STATE_V1);
    assert_sentinel(&token, sizeof(token));
}

int main(void) {
    alarm(5);
    struct sigaction action = {.sa_handler = SIG_DFL};
    assert(sigemptyset(&action.sa_mask) == 0);
    assert(sigaction(SIGPIPE, &action, NULL) == 0);
    sigset_t signals;
    assert(sigemptyset(&signals) == 0 && sigaddset(&signals, SIGPIPE) == 0);
    assert(sigprocmask(SIG_UNBLOCK, &signals, NULL) == 0);

    assert(od_session_abi_version() == OD_SESSION_ABI_VERSION_V1);
    assert((od_session_abi_capabilities() & OD_SESSION_CAP_S0_09_SAME_CODE_V1) != 0);
    od_session_handle_v1 handle = create_session();
    require_normal_notification(handle);
    puts("normal notification: one byte, EOF, then valid result");
    require_notification_failure(handle);
    assert(od_session_destroy_v1(handle) == OD_SESSION_STATUS_OK_V1);

    handle = create_session();
    require_normal_notification(handle);
    assert(od_session_destroy_v1(handle) == OD_SESSION_STATUS_OK_V1);
    assert(sigaction(SIGPIPE, NULL, &action) == 0 && action.sa_handler == SIG_DFL);
    alarm(0);
    puts("V1 native ABI smoke passed: layout, notification failure, destroy and fresh session");
    return 0;
}
