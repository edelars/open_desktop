#include "../../../include/open_desktop/host_abi.h"
#include "../../../include/open_desktop/session_abi_v1.h"

static inline uint32_t vmhost_session_abi_version_v1(void) {
    return OD_SESSION_ABI_VERSION_V1;
}

static inline uint64_t vmhost_session_required_capabilities_v1(void) {
    return OD_SESSION_CAP_S0_09_SAME_CODE_V1;
}
