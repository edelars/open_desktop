/* The target ODAtomics is the header include/FrameTransportAtomics.h (static inline functions). SwiftPM builds a C target
 * only from at least one source file, so this unit compiles the header as plain C and checks the property the shim relies
 * on: 32- and 64-bit atomic words of the shared mapping are lock-free (FRAME_TRANSPORT_V1.md section 5.3). */
#include "FrameTransportAtomics.h"

_Static_assert(__atomic_always_lock_free(sizeof(uint32_t), 0), "32-bit atomics must be lock-free");
_Static_assert(__atomic_always_lock_free(sizeof(uint64_t), 0), "64-bit atomics must be lock-free");
