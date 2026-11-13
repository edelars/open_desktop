#define main stage5_f_program_main
#include "stage5_f.c"
#undef main

#include <assert.h>

int main(void) {
    assert(classify_connect_outcome(1, 0, 0) == CONNECT_SOCKET_SETUP_ERROR);
    assert(classify_connect_outcome(1, 1, 0) == CONNECT_SOCKET_SETUP_ERROR);
    assert(classify_connect_outcome(0, 1, 0) == CONNECT_NETWORK_FAILED);
    assert(classify_connect_outcome(0, 0, 1) == CONNECT_LOCAL_ADDRESS_REFUSED);
    assert(classify_connect_outcome(0, 0, 0) == CONNECT_SOCKET_SETUP_ERROR);

    assert(is_network_connect_error(ECONNREFUSED));
    assert(is_network_connect_error(ETIMEDOUT));
    assert(is_network_connect_error(ENETUNREACH));
    assert(is_network_connect_error(EHOSTUNREACH));
    assert(!is_network_connect_error(EMFILE));
    assert(!is_network_connect_error(EPERM));
    assert(!is_network_connect_error(EINVAL));
    assert(!is_network_connect_error(EINTR));

    assert(is_expected_drop_error(EPIPE));
    assert(is_expected_drop_error(ECONNRESET));
    assert(!is_expected_drop_error(ETIMEDOUT));
    assert(!is_expected_drop_error(EIO));
    assert(is_expected_drop_response(0, 0));
    assert(is_expected_drop_response(-1, ECONNRESET));
    assert(is_expected_drop_response(-1, EPIPE));
    assert(!is_expected_drop_response(-1, ETIMEDOUT));
    assert(!is_expected_drop_response(1, 0));
    return 0;
}
