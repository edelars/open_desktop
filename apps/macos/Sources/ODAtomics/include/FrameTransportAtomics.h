#ifndef OPEN_DESKTOP_VMHOST_FRAME_TRANSPORT_ATOMICS_H
#define OPEN_DESKTOP_VMHOST_FRAME_TRANSPORT_ATOMICS_H

#include <stdint.h>

/* Atomic C shim of protocols/control/FRAME_TRANSPORT_V1.md section 5.3.
 * Internal header of the apps (not a public ABI of include/). It is shared by
 * the VMHost writer and, later, by the UI reader: do not copy it. Swift does
 * not import _Atomic types, so these functions take plain pointers to words in
 * the shared mapping and use the clang __atomic builtins. Expected ARM64
 * code generation is listed in section 5; it is confirmed by disassembly, not
 * by the names. */

static inline uint32_t odft_load_relaxed_u32(uint32_t *p) {
    return __atomic_load_n(p, __ATOMIC_RELAXED);
}

static inline void odft_store_relaxed_u32(uint32_t *p, uint32_t v) {
    __atomic_store_n(p, v, __ATOMIC_RELAXED);
}

static inline uint64_t odft_load_relaxed_u64(uint64_t *p) {
    return __atomic_load_n(p, __ATOMIC_RELAXED);
}

static inline void odft_store_relaxed_u64(uint64_t *p, uint64_t v) {
    __atomic_store_n(p, v, __ATOMIC_RELAXED);
}

static inline uint64_t odft_load_acquire_u64(uint64_t *p) {
    return __atomic_load_n(p, __ATOMIC_ACQUIRE);
}

static inline void odft_store_release_u64(uint64_t *p, uint64_t v) {
    __atomic_store_n(p, v, __ATOMIC_RELEASE);
}

static inline void odft_fence_release(void) {
    __atomic_thread_fence(__ATOMIC_RELEASE);
}

static inline void odft_fence_acquire(void) {
    __atomic_thread_fence(__ATOMIC_ACQUIRE);
}

#endif /* OPEN_DESKTOP_VMHOST_FRAME_TRANSPORT_ATOMICS_H */
