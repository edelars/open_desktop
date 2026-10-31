#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include "open_desktop/host_abi.h"

int main(void) {
    const uint8_t code[] = {0x48, 0x2b, 0xc1, 0x75, 0xfb};
    const od_cpu_state_v0 initial = {{3, 1, 0, 0}, 0, 2};
    od_fragment_handle handle = NULL;
    od_execution_result_v0 first = {0};
    od_execution_result_v0 second = {0};

    const uint32_t create_status = od_fragment_create(code, sizeof(code), &handle);
    if (create_status != OD_STATUS_OK || handle == NULL) {
        fprintf(stderr, "create status=%u handle=%p\n", create_status, handle);
        return 1;
    }
    const uint32_t first_status = od_fragment_execute(handle, &initial, 32, &first);
    const uint32_t second_status = od_fragment_execute(handle, &initial, 32, &second);
    const uint32_t destroy_status = od_fragment_destroy(handle);

    printf("{\"create_status\":%u,\"first_status\":%u,\"first_rax\":%llu,\"first_rip\":%llu,\"first_reason\":%u,\"first_instructions\":%llu,\"second_status\":%u,\"second_rax\":%llu,\"second_rip\":%llu,\"second_reason\":%u,\"second_instructions\":%llu,\"destroy_status\":%u}\n",
           create_status,
           first_status,
           (unsigned long long)first.state.gpr[0],
           (unsigned long long)first.state.rip,
           first.reason,
           (unsigned long long)first.instructions_executed,
           second_status,
           (unsigned long long)second.state.gpr[0],
           (unsigned long long)second.state.rip,
           second.reason,
           (unsigned long long)second.instructions_executed,
           destroy_status);

    if (first_status != OD_STATUS_OK || second_status != OD_STATUS_OK || destroy_status != OD_STATUS_OK) {
        return 2;
    }
    if (first.state.gpr[0] != 0 || first.state.rip != 5 || first.reason != OD_EXIT_CODE_END || first.instructions_executed != 6) {
        return 3;
    }
    if (second.state.gpr[0] != 0 || second.state.rip != 5 || second.reason != OD_EXIT_CODE_END || second.instructions_executed != 6) {
        return 4;
    }
    return 0;
}
