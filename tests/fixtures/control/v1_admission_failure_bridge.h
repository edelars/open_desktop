#ifndef OPEN_DESKTOP_TEST_V1_ADMISSION_FAILURE_BRIDGE_H
#define OPEN_DESKTOP_TEST_V1_ADMISSION_FAILURE_BRIDGE_H

#include <stdio.h>

#define od_session_create_v1 od_session_create_v1_test_hidden
#define od_session_begin_run_v1 od_session_begin_run_v1_test_hidden
#include "../../../apps/vmhost/Sources/ODHostABI/include/VMHostABI.h"
#undef od_session_create_v1
#undef od_session_begin_run_v1

extern uint32_t real_session_create(const od_session_create_options_v1 *, uint32_t,
                                    od_session_handle_v1 *) __asm("_od_session_create_v1");

// A distinct Mach-O symbol lets the wrapper call the real Rust create entry.
static inline uint32_t od_session_create_v1(const od_session_create_options_v1 *, uint32_t,
                                           od_session_handle_v1 *) __asm("_fixture_od_session_create_v1");

static inline uint32_t od_session_create_v1(const od_session_create_options_v1 *options,
                                           uint32_t options_size,
                                           od_session_handle_v1 *out_handle) {
    // The single ADD fixture fails at create. The two-ADD fixture reaches begin.
    if (options->code_length == 3) {
        fputs("fixture_invalid_state=create\n", stderr);
        return OD_SESSION_STATUS_INVALID_STATE_V1;
    }
    return real_session_create(options, options_size, out_handle);
}

static inline uint32_t od_session_begin_run_v1(od_session_handle_v1 handle,
                                              const od_session_run_request_v1 *request,
                                              uint32_t request_size,
                                              od_session_run_token_v1 *out_token,
                                              uint32_t out_token_size) {
    fputs("fixture_invalid_state=begin\n", stderr);
    return OD_SESSION_STATUS_INVALID_STATE_V1;
}

#endif
