//! C ABI for the agent channel of one session (S11-21, ADR-0007).
//!
//! Contract: `docs/specs/AGENT_HOST_ABI_V1.md`; declarations:
//! `include/open_desktop/agent_abi_v1.h`; the device: `docs/specs/AGENT_CHANNEL_DEVICE.md`.
//!
//! A handle is an opaque, never-reused identifier registered in a process-wide table, like the
//! display ABI: a stale, forged or released handle gives `INVALID_HANDLE`, not undefined
//! behaviour. A handle is a host reference to an [`AgentChannel`] (the channel itself is owned by
//! the session that built the machine): `release` drops the reference, a call already running on
//! another thread keeps its own reference and finishes. The channel serializes calls with its own
//! lock, so any thread may use a handle and no `BUSY` status exists.
//!
//! The session that runs guest code with bus and devices (S10-09) does not exist yet. Until it
//! does, the ABI is exercised by [`od_agent_channel_test_create_v1`] (a detached device) and by
//! the unit tests below, which bind the device on a bus without a CPU and drive the guest side
//! through port accesses.

use od_machine::{AgentChannel, AgentChannelStats};
use std::collections::BTreeMap;
use std::mem::{align_of, size_of};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, PoisonError};

const ABI_VERSION: u32 = 0x0001_0000;

const STATUS_OK: u32 = 0;
const STATUS_INVALID_ARGUMENT: u32 = 1;
const STATUS_ABI_MISMATCH: u32 = 2;
const STATUS_INVALID_HANDLE: u32 = 3;
const STATUS_INTERNAL_ERROR: u32 = 4;

/// Opaque type behind `od_agent_channel *`; never dereferenced.
#[repr(C)]
pub struct OdAgentChannel {
    _private: [u8; 0],
}

/// Mirror of `od_agent_channel_stats` in `agent_abi_v1.h`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OdAgentChannelStats {
    pub abi_version: u32,
    pub struct_size: u32,
    pub rx_capacity: u64,
    pub tx_capacity: u64,
    pub rx_queued: u64,
    pub tx_queued: u64,
    pub rx_bytes_accepted: u64,
    pub rx_bytes_delivered: u64,
    pub rx_overflow_bytes: u64,
    pub tx_bytes_accepted: u64,
    pub tx_bytes_read: u64,
    pub tx_overflow_bytes: u64,
}

const STATS_SIZE: usize = 88;
const _: () = assert!(size_of::<OdAgentChannelStats>() == STATS_SIZE);
const _: () = assert!(align_of::<OdAgentChannelStats>() == 8);

struct Registry {
    /// Identifiers are never reused; zero is never issued (NULL).
    next_id: usize,
    entries: BTreeMap<usize, AgentChannel>,
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    next_id: 1,
    entries: BTreeMap::new(),
});

fn registry() -> std::sync::MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Registers `channel` and returns its handle (null when the identifiers are exhausted). The
/// session that owns the machine calls this to hand the channel to the host (S10-09, S11-22); the
/// test constructor below uses it too.
pub(crate) fn register_agent_channel(channel: AgentChannel) -> *mut OdAgentChannel {
    let mut registry = registry();
    let id = registry.next_id;
    let Some(next) = id.checked_add(1) else {
        return std::ptr::null_mut();
    };
    registry.next_id = next;
    registry.entries.insert(id, channel);
    std::ptr::without_provenance_mut(id)
}

fn lookup(handle: *mut OdAgentChannel) -> Option<AgentChannel> {
    registry().entries.get(&handle.addr()).cloned()
}

/// Runs `body` so that a Rust panic never crosses the C boundary.
fn guarded(body: impl FnOnce() -> u32) -> u32 {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or(STATUS_INTERNAL_ERROR)
}

fn with_channel(handle: *mut OdAgentChannel, body: impl FnOnce(&AgentChannel) -> u32) -> u32 {
    if handle.is_null() {
        return STATUS_INVALID_ARGUMENT;
    }
    match lookup(handle) {
        Some(channel) => body(&channel),
        None => STATUS_INVALID_HANDLE,
    }
}

fn valid_ptr<T>(pointer: *const T) -> bool {
    !pointer.is_null() && pointer.is_aligned()
}

fn overlaps(a: *const u8, a_len: usize, b: *const u8, b_len: usize) -> bool {
    let (a, b) = (a.addr(), b.addr());
    a < b.saturating_add(b_len) && b < a.saturating_add(a_len)
}

/// Shape of a (buffer, len, out count) triple shared by the three data calls.
fn data_args_valid(buf: *const u8, len: usize, out: *const usize) -> bool {
    valid_ptr(out)
        && len <= isize::MAX as usize
        && (len == 0 || !buf.is_null())
        && !(len > 0 && overlaps(buf, len, out.cast(), size_of::<usize>()))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_agent_channel_read_v1(
    channel: *mut OdAgentChannel,
    buf: *mut u8,
    len: usize,
    out_read: *mut usize,
) -> u32 {
    guarded(|| {
        if !data_args_valid(buf, len, out_read) {
            return STATUS_INVALID_ARGUMENT;
        }
        with_channel(channel, |channel| {
            // A zero-length call still pumps the device.
            let count = if len == 0 {
                channel.read_from_guest(&mut [])
            } else {
                // SAFETY: non-null, length bounded; validity for `len` bytes is a caller
                // obligation of the ABI.
                let slice = unsafe { std::slice::from_raw_parts_mut(buf, len) };
                channel.read_from_guest(slice)
            };
            // SAFETY: shape checked above.
            unsafe { out_read.write(count) };
            STATUS_OK
        })
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_agent_channel_write_v1(
    channel: *mut OdAgentChannel,
    buf: *const u8,
    len: usize,
    out_written: *mut usize,
) -> u32 {
    guarded(|| {
        if !data_args_valid(buf, len, out_written) {
            return STATUS_INVALID_ARGUMENT;
        }
        with_channel(channel, |channel| {
            let count = if len == 0 {
                channel.write_to_guest(&[])
            } else {
                // SAFETY: as in `read`.
                let slice = unsafe { std::slice::from_raw_parts(buf, len) };
                channel.write_to_guest(slice)
            };
            // SAFETY: shape checked above.
            unsafe { out_written.write(count) };
            STATUS_OK
        })
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_agent_channel_stats_v1(
    channel: *mut OdAgentChannel,
    out_stats: *mut OdAgentChannelStats,
) -> u32 {
    guarded(|| {
        if !valid_ptr(out_stats) {
            return STATUS_INVALID_ARGUMENT;
        }
        // SAFETY: pointer shape checked; the two header fields are caller input.
        let (version, size) = unsafe { ((*out_stats).abi_version, (*out_stats).struct_size) };
        if version != ABI_VERSION || size as usize != STATS_SIZE {
            return STATUS_ABI_MISMATCH;
        }
        with_channel(channel, |channel| {
            let stats = channel.stats();
            // SAFETY: as above.
            unsafe { out_stats.write(abi_stats(stats)) };
            STATUS_OK
        })
    })
}

fn abi_stats(stats: AgentChannelStats) -> OdAgentChannelStats {
    OdAgentChannelStats {
        abi_version: ABI_VERSION,
        struct_size: STATS_SIZE as u32,
        rx_capacity: stats.rx_capacity,
        tx_capacity: stats.tx_capacity,
        rx_queued: stats.rx_queued,
        tx_queued: stats.tx_queued,
        rx_bytes_accepted: stats.rx_bytes_accepted,
        rx_bytes_delivered: stats.rx_bytes_delivered,
        rx_overflow_bytes: stats.rx_overflow_bytes,
        tx_bytes_accepted: stats.tx_bytes_accepted,
        tx_bytes_read: stats.tx_bytes_read,
        tx_overflow_bytes: stats.tx_overflow_bytes,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_agent_channel_release_v1(channel: *mut OdAgentChannel) -> u32 {
    guarded(|| {
        if channel.is_null() {
            return STATUS_INVALID_ARGUMENT;
        }
        let removed = registry().entries.remove(&channel.addr());
        match removed {
            Some(_) => STATUS_OK,
            None => STATUS_INVALID_HANDLE,
        }
    })
}

/// TEST-ONLY (see the header): a channel without a session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_agent_channel_test_create_v1(
    out_channel: *mut *mut OdAgentChannel,
) -> u32 {
    guarded(|| {
        if !valid_ptr(out_channel) {
            return STATUS_INVALID_ARGUMENT;
        }
        // SAFETY: pointer shape checked; the slot is read, not written, until success.
        if unsafe { !(*out_channel).is_null() } {
            return STATUS_INVALID_ARGUMENT;
        }
        // The bus device is dropped: the channel keeps the register state alive and the host
        // queues work without a bus (the guest side is `test_inject`).
        let (_device, channel) = AgentChannel::new();
        let handle = register_agent_channel(channel);
        if handle.is_null() {
            return STATUS_INTERNAL_ERROR;
        }
        // SAFETY: as above; success transfers the handle.
        unsafe { out_channel.write(handle) };
        STATUS_OK
    })
}

/// TEST-ONLY (see the header): bytes "from the guest" without a CPU.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_agent_channel_test_inject_v1(
    channel: *mut OdAgentChannel,
    buf: *const u8,
    len: usize,
    out_injected: *mut usize,
) -> u32 {
    guarded(|| {
        if !data_args_valid(buf, len, out_injected) {
            return STATUS_INVALID_ARGUMENT;
        }
        with_channel(channel, |channel| {
            let count = if len == 0 {
                channel.test_inject_from_guest(&[])
            } else {
                // SAFETY: as in `read`.
                let slice = unsafe { std::slice::from_raw_parts(buf, len) };
                channel.test_inject_from_guest(slice)
            };
            // SAFETY: shape checked above.
            unsafe { out_injected.write(count) };
            STATUS_OK
        })
    })
}

#[cfg(test)]
mod tests;
