#include "../../../include/open_desktop/host_abi.h"
#include "../../../include/open_desktop/session_abi_v1.h"
#include "../../../include/open_desktop/display_abi_v1.h"
#include "FrameTransportAtomics.h"
#include <fcntl.h>
#include <sys/mman.h>

static inline uint32_t vmhost_session_abi_version_v1(void) {
    return OD_SESSION_ABI_VERSION_V1;
}

static inline uint64_t vmhost_session_required_capabilities_v1(void) {
    return OD_SESSION_CAP_S0_09_SAME_CODE_V1;
}

/* Swift does not import function-like UINT32_C macros: expose them as functions. */
static inline uint32_t vmhost_display_abi_version_v1(void) {
    return OD_DISPLAY_ABI_VERSION_V1;
}

static inline uint32_t vmhost_display_abi_version_v2(void) {
    return OD_DISPLAY_ABI_VERSION_V2;
}

static inline uint32_t vmhost_display_cursor_flag_enabled(void) {
    return OD_DISPLAY_CURSOR_FLAG_ENABLED;
}

static inline uint32_t vmhost_display_mode_flag_active(void) {
    return OD_DISPLAY_MODE_FLAG_ACTIVE;
}

static inline uint32_t vmhost_display_mode_flag_has_frame(void) {
    return OD_DISPLAY_MODE_FLAG_HAS_FRAME;
}

/* shm_open is variadic and unavailable to Swift: exclusive create, mode 0600 (FRAME_TRANSPORT_V1.md 2.2). */
static inline int vmhost_shm_create_excl(const char *name) {
    return shm_open(name, O_RDWR | O_CREAT | O_EXCL, 0600);
}
