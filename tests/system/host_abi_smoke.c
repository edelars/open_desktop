#include "open_desktop/host_abi.h"

#include <assert.h>

_Static_assert(sizeof(od_cpu_state_v0) == 48, "od_cpu_state_v0 layout changed");
_Static_assert(sizeof(od_execution_result_v0) == 64, "od_execution_result_v0 layout changed");

int main(void) {
    const uint8_t loop[] = {0x48, 0x2B, 0xC1, 0x75, 0xFB}; /* sub rax,rcx; jnz -5 */
    const uint8_t invalid[] = {0x90};
    od_fragment_handle handle = NULL;
    od_cpu_state_v0 initial = {{3, 1, 0, 0}, 0, 2};
    od_execution_result_v0 result = {0};

    assert(od_host_abi_version() == OD_HOST_ABI_VERSION_V0);
    assert(od_fragment_create(invalid, sizeof(invalid), &handle) == OD_STATUS_INVALID_CODE);
    assert(handle == NULL);
    assert(od_fragment_create(loop, sizeof(loop), &handle) == OD_STATUS_OK);
    assert(handle != NULL);
    assert(od_fragment_execute(handle, &initial, 32, &result) == OD_STATUS_OK);
    assert(result.reason == OD_EXIT_CODE_END);
    assert(result.instructions_executed == 6);
    assert(result.state.rip == sizeof(loop));
    assert(result.state.gpr[0] == 0);
    assert(result.state.gpr[1] == 1);
    assert(od_fragment_destroy(handle) == OD_STATUS_OK);
    return 0;
}
