#ifndef OPEN_DESKTOP_AGENT_ABI_V1_H
#define OPEN_DESKTOP_AGENT_ABI_V1_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opt-in agent channel ABI, version 1 (S11-21, ADR-0007). Independent of
 * host_abi.h, session_abi_v1.h and display_abi_v1.h, which it does not extend.
 * The contract (ownership, threads, queue and overflow rules, extension rules)
 * is docs/specs/AGENT_HOST_ABI_V1.md; the device behind it is
 * docs/specs/AGENT_CHANNEL_DEVICE.md. This header holds the C declarations and
 * numeric constants; crates/od-host/src/agent_abi_v1.rs repeats the codes by
 * hand and checks them against this file in a test. */

/* Exact-match identifier of this ABI; not an arithmetic encoding. The stats
 * struct starts with abi_version, then struct_size. */
#define OD_AGENT_ABI_VERSION_V1 UINT32_C(0x00010000)

#define OD_AGENT_CHANNEL_STATS_SIZE_V1 88

/* Functions return one of these as uint32_t (not as a C enum). Values are
 * fixed; new codes are appended, existing ones are never changed or reused. */
enum od_agent_status {
    OD_AGENT_STATUS_OK = 0,
    OD_AGENT_STATUS_INVALID_ARGUMENT = 1, /* NULL handle, NULL/misaligned out pointer, NULL buffer with len > 0, len > PTRDIFF_MAX, overlapping buffer and out pointer */
    OD_AGENT_STATUS_ABI_MISMATCH = 2,     /* abi_version or struct_size of the stats struct differs from this header */
    OD_AGENT_STATUS_INVALID_HANDLE = 3,   /* unknown, forged or released handle */
    OD_AGENT_STATUS_INTERNAL_ERROR = 4    /* host fault (a caught Rust panic) */
};

/* Opaque handle on the agent channel of one session. A handle is a host
 * reference: od_agent_channel_release_v1 drops it, not the device. */
typedef struct od_agent_channel od_agent_channel;

/* Native C layout on the supported 64-bit macOS host ABI; fixed-width fields,
 * no implicit padding. Not a serialized format.
 *
 * size 88, align 8: abi_version 0, struct_size 4, rx_capacity 8, tx_capacity 16,
 * rx_queued 24, tx_queued 32, rx_bytes_accepted 40, rx_bytes_delivered 48,
 * rx_overflow_bytes 56, tx_bytes_accepted 64, tx_bytes_read 72,
 * tx_overflow_bytes 80. The caller sets abi_version and struct_size before the
 * call; the other fields are output. rx = host to guest, tx = guest to host.
 * Totals only grow; queued values are the fill levels at the call. */
typedef struct od_agent_channel_stats {
    uint32_t abi_version;
    uint32_t struct_size;
    uint64_t rx_capacity;
    uint64_t tx_capacity;
    uint64_t rx_queued;
    uint64_t tx_queued;
    uint64_t rx_bytes_accepted;
    uint64_t rx_bytes_delivered;
    uint64_t rx_overflow_bytes;
    uint64_t tx_bytes_accepted;
    uint64_t tx_bytes_read;
    uint64_t tx_overflow_bytes;
} od_agent_channel_stats;

/* Guest to host, non-blocking. Copies up to len queued bytes into buf and
 * stores the number in *out_read (0: nothing queued, status OK). Pumps the
 * device first. len == 0 is valid (buf may then be NULL). */
uint32_t od_agent_channel_read_v1(od_agent_channel *channel, uint8_t *buf, size_t len, size_t *out_read);

/* Host to guest, non-blocking. Queues as many of the len bytes as fit and stores
 * that number in *out_written; the rest is refused and counted in
 * rx_overflow_bytes (status is still OK: a short count is not an error). Order
 * is kept. len == 0 is valid (buf may then be NULL). */
uint32_t od_agent_channel_write_v1(od_agent_channel *channel, const uint8_t *buf, size_t len, size_t *out_written);

/* Fill levels, capacities and counters. */
uint32_t od_agent_channel_stats_v1(od_agent_channel *channel, od_agent_channel_stats *out_stats);

/* Drops the host reference. The handle is invalid afterwards (INVALID_HANDLE);
 * calls already running on other threads finish normally. */
uint32_t od_agent_channel_release_v1(od_agent_channel *channel);

/* TEST-ONLY. Not part of the product contract; absent from a future release
 * build is allowed. Creates a channel without a session (a detached COM2
 * device with default queue capacities); *out_channel must be NULL. */
uint32_t od_agent_channel_test_create_v1(od_agent_channel **out_channel);

/* TEST-ONLY. Queues len bytes as if the guest had sent them, without a CPU or
 * register access; *out_injected is the number that fitted (the rest is dropped
 * and counted in tx_overflow_bytes). */
uint32_t od_agent_channel_test_inject_v1(od_agent_channel *channel, const uint8_t *buf, size_t len, size_t *out_injected);

#ifdef __cplusplus
}
#endif

#endif
