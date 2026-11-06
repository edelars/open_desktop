/* Header of the target ODHostConstants (apps/Package.swift): only the engine host ABI constants and types of
 * include/open_desktop/host_abi.h (OD_HOST_ABI_VERSION_V0, OD_FRAGMENT_MAX_LENGTH_V0). It does not expose the session or
 * display ABI headers nor the VMHost helpers (ADR-0006): the launcher smoke depends on this target, not on ODHostABI. */
#include "../../../../../include/open_desktop/host_abi.h"
