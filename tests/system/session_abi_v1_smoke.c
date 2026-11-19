#define _POSIX_C_SOURCE 200809L

#include "open_desktop/session_abi_v1.h"

#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>
#include <sys/resource.h>
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

static void require_normal_notification(od_session_handle_v1 handle, uint32_t id) {
    /* ZF set, so the branch falls through to the end after one instruction. */
    od_session_run_request_v1 request = make_request(id, 8, 0x42);
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

static void require_pipe_creation_failure(od_session_handle_v1 handle, uint32_t id) {
    enum { DESCRIPTOR_CAP = 32 };
    struct rlimit saved;
    assert(getrlimit(RLIMIT_NOFILE, &saved) == 0);
    struct rlimit temporary = saved;
    if (temporary.rlim_cur > DESCRIPTOR_CAP) {
        temporary.rlim_cur = DESCRIPTOR_CAP;
    }
    assert(setrlimit(RLIMIT_NOFILE, &temporary) == 0);

    int held[DESCRIPTOR_CAP];
    size_t count = 0;
    for (;;) {
        int fd = open("/dev/null", O_RDONLY | O_CLOEXEC);
        if (fd < 0) {
            assert(errno == EMFILE);
            break;
        }
        assert(count < DESCRIPTOR_CAP);
        held[count++] = fd;
    }
    assert(count > 0);
    /* Leave one slot. pipe() needs two, so it must undo any partial allocation. */
    int available_fd = held[--count];
    assert(close(available_fd) == 0);
    int probe[2];
    assert(pipe(probe) == -1 && errno == EMFILE);

    od_session_run_request_v1 request = make_request(id, 8, 0x42);
    od_session_run_token_v1 token;
    memset(&token, 0xA5, sizeof(token));
    uint32_t begin_status = od_session_begin_run_v1(handle, &request, sizeof(request), &token, sizeof(token));
    assert(begin_status == OD_SESSION_STATUS_ALLOCATION_FAILED_V1);
    assert_sentinel(&token, sizeof(token));
    assert(od_session_request_stop_v1(handle, id) == OD_SESSION_STATUS_NO_ACTIVE_RUN_V1);
    od_session_run_result_v1 result;
    memset(&result, 0xA5, sizeof(result));
    assert(od_session_wait_run_v1(handle, id, &result, sizeof(result)) == OD_SESSION_STATUS_NO_ACTIVE_RUN_V1);
    assert_sentinel(&result, sizeof(result));

    int reopened = open("/dev/null", O_RDONLY | O_CLOEXEC);
    assert(reopened == available_fd);
    assert(close(reopened) == 0);
    while (count > 0) {
        assert(close(held[--count]) == 0);
    }
    assert(setrlimit(RLIMIT_NOFILE, &saved) == 0);

    assert(od_session_begin_run_v1(handle, &request, sizeof(request), &token, sizeof(token)) ==
           OD_SESSION_STATUS_INVALID_REQUEST_ID_V1);
    assert_sentinel(&token, sizeof(token));
    require_normal_notification(handle, id + 1);
    printf("pipe EMFILE: begin status=%u, no active run, retained high-water mark, recovered on greater ID\n",
           begin_status);
}

struct wait_race {
    od_session_handle_v1 handle;
    uint32_t id;
    pthread_mutex_t mutex;
    pthread_cond_t changed;
    struct waiter *first;
};

struct waiter {
    struct wait_race *race;
    uint32_t status;
    od_session_run_result_v1 result;
};

static void *wait_once(void *argument) {
    struct waiter *waiter = argument;
    struct wait_race *race = waiter->race;
    waiter->status = od_session_wait_run_v1(race->handle, race->id, &waiter->result, sizeof(waiter->result));
    assert(pthread_mutex_lock(&race->mutex) == 0);
    if (race->first == NULL) {
        race->first = waiter;
        assert(pthread_cond_signal(&race->changed) == 0);
    }
    assert(pthread_mutex_unlock(&race->mutex) == 0);
    return NULL;
}

static void require_single_waiter(od_session_handle_v1 handle, uint32_t id) {
    od_session_run_request_v1 request = make_request(id, UINT64_MAX, 2);
    int fd = begin(handle, &request);
    struct wait_race race = {.handle = handle, .id = id};
    assert(pthread_mutex_init(&race.mutex, NULL) == 0);
    assert(pthread_cond_init(&race.changed, NULL) == 0);
    struct waiter waiters[] = {{.race = &race}, {.race = &race}};
    pthread_t threads[2];
    for (size_t i = 0; i < 2; ++i) {
        memset(&waiters[i].result, 0xA5, sizeof(waiters[i].result));
        assert(pthread_create(&threads[i], NULL, wait_once, &waiters[i]) == 0);
    }

    /* The guest loops until main issues STOP. The first return must be BUSY. */
    assert(pthread_mutex_lock(&race.mutex) == 0);
    while (race.first == NULL) {
        assert(pthread_cond_wait(&race.changed, &race.mutex) == 0);
    }
    struct waiter *rejected = race.first;
    assert(rejected->status == OD_SESSION_STATUS_BUSY_V1);
    assert_sentinel(&rejected->result, sizeof(rejected->result));
    assert(pthread_mutex_unlock(&race.mutex) == 0);

    od_session_run_request_v1 next = make_request(id + 1, 8, 0x42);
    od_session_run_token_v1 token;
    memset(&token, 0xA5, sizeof(token));
    assert(od_session_begin_run_v1(handle, &next, sizeof(next), &token, sizeof(token)) == OD_SESSION_STATUS_BUSY_V1);
    assert_sentinel(&token, sizeof(token));
    assert(od_session_request_stop_v1(handle, id) == OD_SESSION_STATUS_OK_V1);
    for (size_t i = 0; i < 2; ++i) {
        assert(pthread_join(threads[i], NULL) == 0);
    }

    const struct waiter *winner = rejected == &waiters[0] ? &waiters[1] : &waiters[0];
    assert(winner->status == OD_SESSION_STATUS_OK_V1);
    const od_session_run_result_v1 *result = &winner->result;
    assert(result->struct_size == OD_SESSION_RUN_RESULT_SIZE_V1);
    assert(result->request_id == id && result->reason == OD_SESSION_EXIT_STOP_REQUESTED_V1 && result->reserved == 0);
    assert(result->state.rip == request.initial_state.rip && result->state.rflags == request.initial_state.rflags);
    assert(memcmp(result->state.gpr, request.initial_state.gpr, sizeof(result->state.gpr)) == 0);
    uint8_t notification[2];
    assert(read(fd, notification, sizeof(notification)) == 1 && notification[0] == OD_SESSION_COMPLETION_BYTE_V1);
    assert(read(fd, notification, sizeof(notification)) == 0);
    assert(close(fd) == 0);
    assert(pthread_cond_destroy(&race.changed) == 0);
    assert(pthread_mutex_destroy(&race.mutex) == 0);

    assert(od_session_wait_run_v1(handle, id, &rejected->result, sizeof(rejected->result)) ==
           OD_SESSION_STATUS_NO_ACTIVE_RUN_V1);
    assert_sentinel(&rejected->result, sizeof(rejected->result));
    assert(od_session_begin_run_v1(handle, &next, sizeof(next), &token, sizeof(token)) ==
           OD_SESSION_STATUS_INVALID_REQUEST_ID_V1);
    assert_sentinel(&token, sizeof(token));
    require_normal_notification(handle, id + 2);
    puts("concurrent wait: one BUSY, one STOP result, one notification, later greater ID completed");
}

static void require_notification_failure(od_session_handle_v1 handle, uint32_t id) {
    /* ZF clear and effectively unlimited budget, so STOP drives completion. */
    od_session_run_request_v1 request = make_request(id, UINT64_MAX, 2);
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

    request.request_id = id + 1;
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
    require_normal_notification(handle, 1);
    puts("normal notification: one byte, EOF, then valid result");
    require_pipe_creation_failure(handle, 2);
    require_single_waiter(handle, 4);
    require_notification_failure(handle, 7);
    assert(od_session_destroy_v1(handle) == OD_SESSION_STATUS_OK_V1);

    handle = create_session();
    require_normal_notification(handle, 1);
    assert(od_session_destroy_v1(handle) == OD_SESSION_STATUS_OK_V1);
    assert(sigaction(SIGPIPE, NULL, &action) == 0 && action.sa_handler == SIG_DFL);
    alarm(0);
    puts("V1 native ABI smoke passed: layout, notification failure, destroy and fresh session");
    return 0;
}
