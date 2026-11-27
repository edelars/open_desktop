//! S7-32 diagnostic: the log of guest device accesses the board refused (`boot-firmware
//! --device-errors log`).
//!
//! By default (`stop`) a guest PIO or MMIO access that a device refuses ([`DeviceError`]), or a
//! port no device is bound to, ends the run (`pio_error`, `backend_error`). With the log installed
//! on the bus ([`crate::MachineBus::enable_device_error_log`]) the general step of the set loop
//! completes such an access as an open bus would: a write is dropped, a read returns all ones of
//! its width, and the instruction retires. Each absorbed access is counted here, grouped by
//! (space, device, address, direction, width, kind). The device itself is not changed: it refused
//! the access, so by the [`crate::BusDevice`] contract it committed nothing.
//!
//! A run with the log is a diagnostic, not a compatibility result: from the first absorbed access
//! on, the guest sees a machine the model does not claim to be, and what it does next may differ
//! from what it does on real hardware.
//!
//! Limits: after `event_limit` absorbed accesses, or when a new group would exceed `group_limit`,
//! the next refusal is not absorbed and the run stops on it as without the log.

use crate::{AccessWidth, DeviceError, IoSpace};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

/// The default number of distinct groups the log keeps.
pub const DEVICE_ERROR_LOG_GROUP_LIMIT: usize = 4096;

/// Whether the refused access was a read or a write.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DeviceErrorDirection {
    Read,
    Write,
}

/// What refused the access.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum DeviceErrorKind {
    /// The device bound at the address returned this error.
    Device(DeviceError),
    /// No device is bound at the port (`BusAccessError::UnmappedPort`).
    UnmappedPort,
    /// The access straddles the end of a port binding (`BusAccessError::CrossesRegion`).
    CrossesRegion,
    /// The write half of a read-modify-write whose read the log absorbed in the same
    /// instruction: the value derives from the all-ones the guest was given, so it is dropped
    /// without reaching the device (which may well have accepted it).
    SuppressedAfterAbsorbedRead,
}

/// The identity of a group: every absorbed access with the same key adds to one group.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DeviceErrorKey {
    pub space: IoSpace,
    /// The bus index of the device that refused; `None` when no device was reached.
    pub device_index: Option<usize>,
    /// The port (PIO) or the guest-physical address (MMIO) of the access.
    pub address: u64,
    pub direction: DeviceErrorDirection,
    pub width: AccessWidth,
    pub kind: DeviceErrorKind,
}

/// One group of absorbed accesses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceErrorGroup {
    pub key: DeviceErrorKey,
    /// The first port or the guest-physical start of the binding the access hit; `None` when no
    /// device was reached.
    pub region_base: Option<u64>,
    /// How many accesses were absorbed in this group.
    pub count: u64,
    /// Instructions retired before the instruction of the first and of the last access.
    pub first_retired: u64,
    pub last_retired: u64,
    /// RIP and CPL of the instruction of the first access.
    pub first_rip: u64,
    pub first_cpl: u8,
    /// The value of the first write (`None` for a read).
    pub first_value: Option<u64>,
}

/// Why the log stopped absorbing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceErrorLogStop {
    /// `event_limit` accesses were absorbed.
    EventLimit,
    /// A new group would have exceeded `group_limit`.
    GroupLimit,
}

/// What the log holds at the end of a run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceErrorLogReport {
    pub event_limit: u64,
    pub group_limit: usize,
    /// Absorbed accesses in all groups.
    pub events: u64,
    /// The groups in the order of their first access.
    pub groups: Vec<DeviceErrorGroup>,
    /// Set when a refusal was not absorbed because of a limit (the run stopped on it).
    pub stopped: Option<DeviceErrorLogStop>,
}

/// The instruction a refused access belongs to, given by the general step of the set loop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DeviceErrorContext {
    pub(crate) rip: u64,
    pub(crate) cpl: u8,
    /// Instructions retired in this set-loop call before the instruction (the board adds the
    /// retired count of the run before the call, [`DeviceErrorLog::set_retired_base`]).
    pub(crate) retired_in_call: u64,
}

/// A refused access as the bus saw it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DeviceErrorAccess {
    pub(crate) key: DeviceErrorKey,
    pub(crate) region_base: Option<u64>,
    pub(crate) value: Option<u64>,
}

/// The log on the bus. Only the error path of a device access reaches it.
#[derive(Debug)]
pub struct DeviceErrorLog {
    retired_base: AtomicU64,
    state: Mutex<LogState>,
}

#[derive(Debug)]
struct LogState {
    event_limit: u64,
    group_limit: usize,
    events: u64,
    groups: Vec<DeviceErrorGroup>,
    index: HashMap<DeviceErrorKey, usize>,
    stopped: Option<DeviceErrorLogStop>,
}

impl DeviceErrorLog {
    /// A log that absorbs at most `event_limit` accesses in at most `group_limit` groups.
    pub fn new(event_limit: u64, group_limit: usize) -> Self {
        Self {
            retired_base: AtomicU64::new(0),
            state: Mutex::new(LogState {
                event_limit,
                group_limit,
                events: 0,
                groups: Vec::new(),
                index: HashMap::new(),
                stopped: None,
            }),
        }
    }

    /// The instructions the run retired before the current set-loop call (the board sets it before
    /// each call and at each merged quantum boundary of the turbo engine).
    pub(crate) fn set_retired_base(&self, retired: u64) {
        self.retired_base.store(retired, Ordering::Relaxed);
    }

    /// Records the refused `access` of the instruction `context`. `true`: absorbed (the caller
    /// completes the access as an open bus); `false`: a limit is reached, the caller returns the
    /// refusal as without the log (and the log stays stopped).
    pub(crate) fn absorb(&self, access: DeviceErrorAccess, context: DeviceErrorContext) -> bool {
        let retired = self
            .retired_base
            .load(Ordering::Relaxed)
            .saturating_add(context.retired_in_call);
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.stopped.is_some() {
            return false;
        }
        if state.events >= state.event_limit {
            state.stopped = Some(DeviceErrorLogStop::EventLimit);
            return false;
        }
        match state.index.get(&access.key).copied() {
            Some(position) => {
                let group = &mut state.groups[position];
                group.count += 1;
                group.last_retired = retired;
            }
            None => {
                if state.groups.len() >= state.group_limit {
                    state.stopped = Some(DeviceErrorLogStop::GroupLimit);
                    return false;
                }
                let position = state.groups.len();
                state.index.insert(access.key.clone(), position);
                state.groups.push(DeviceErrorGroup {
                    key: access.key,
                    region_base: access.region_base,
                    count: 1,
                    first_retired: retired,
                    last_retired: retired,
                    first_rip: context.rip,
                    first_cpl: context.cpl,
                    first_value: access.value,
                });
            }
        }
        state.events += 1;
        true
    }

    /// Absorbed accesses so far, in all groups.
    pub fn events(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .events
    }

    /// A copy of what the log holds now.
    pub fn report(&self) -> DeviceErrorLogReport {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        DeviceErrorLogReport {
            event_limit: state.event_limit,
            group_limit: state.group_limit,
            events: state.events,
            groups: state.groups.clone(),
            stopped: state.stopped,
        }
    }
}

/// Whether the log may absorb a refusal of this kind. A clock overflow is not a gap of a device
/// model but the end of virtual time: it stops the run as without the log.
pub(crate) fn absorbable(error: &DeviceError) -> bool {
    !matches!(error, DeviceError::ClockOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access(
        address: u64,
        direction: DeviceErrorDirection,
        value: Option<u64>,
    ) -> DeviceErrorAccess {
        DeviceErrorAccess {
            key: DeviceErrorKey {
                space: IoSpace::Pio,
                device_index: Some(3),
                address,
                direction,
                width: AccessWidth::Byte,
                kind: DeviceErrorKind::Device(DeviceError::UnsupportedConfiguration),
            },
            region_base: Some(0x20),
            value,
        }
    }

    fn at(rip: u64, retired_in_call: u64) -> DeviceErrorContext {
        DeviceErrorContext {
            rip,
            cpl: 0,
            retired_in_call,
        }
    }

    #[test]
    fn accesses_with_one_key_form_one_group_with_first_and_last_and_the_first_case() {
        let log = DeviceErrorLog::new(10, 4);
        log.set_retired_base(1000);
        assert!(log.absorb(
            access(0x21, DeviceErrorDirection::Write, Some(0xd8)),
            at(0x10, 5)
        ));
        assert!(log.absorb(
            access(0x21, DeviceErrorDirection::Write, Some(0x04)),
            at(0x20, 7)
        ));
        log.set_retired_base(2000);
        assert!(log.absorb(
            access(0x21, DeviceErrorDirection::Write, Some(0x01)),
            at(0x30, 1)
        ));
        // Another direction is another group, in the order of its first access.
        assert!(log.absorb(access(0x21, DeviceErrorDirection::Read, None), at(0x40, 2)));
        let report = log.report();
        assert_eq!(report.events, 4);
        assert_eq!(report.stopped, None);
        assert_eq!(report.groups.len(), 2);
        let write = &report.groups[0];
        assert_eq!(write.count, 3);
        assert_eq!((write.first_retired, write.last_retired), (1005, 2001));
        assert_eq!((write.first_rip, write.first_value), (0x10, Some(0xd8)));
        assert_eq!(write.region_base, Some(0x20));
        let read = &report.groups[1];
        assert_eq!(read.key.direction, DeviceErrorDirection::Read);
        assert_eq!(
            (read.count, read.first_retired, read.first_value),
            (1, 2002, None)
        );
    }

    #[test]
    fn the_event_limit_stops_absorbing_and_stays_stopped() {
        let log = DeviceErrorLog::new(2, 4);
        assert!(log.absorb(access(0x21, DeviceErrorDirection::Write, Some(1)), at(1, 0)));
        assert!(log.absorb(access(0x21, DeviceErrorDirection::Write, Some(2)), at(1, 1)));
        assert!(!log.absorb(access(0x21, DeviceErrorDirection::Write, Some(3)), at(1, 2)));
        assert!(!log.absorb(access(0x22, DeviceErrorDirection::Write, Some(3)), at(1, 3)));
        let report = log.report();
        assert_eq!(report.events, 2);
        assert_eq!(report.groups[0].count, 2);
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.stopped, Some(DeviceErrorLogStop::EventLimit));
    }

    #[test]
    fn a_full_group_table_stops_absorbing_but_not_before_a_new_group_is_needed() {
        let log = DeviceErrorLog::new(100, 1);
        assert!(log.absorb(access(0x21, DeviceErrorDirection::Write, Some(1)), at(1, 0)));
        assert!(log.absorb(access(0x21, DeviceErrorDirection::Write, Some(1)), at(1, 1)));
        assert!(!log.absorb(access(0xa1, DeviceErrorDirection::Write, Some(1)), at(1, 2)));
        // Once stopped, even a known group is not absorbed: the run stops on the next refusal.
        assert!(!log.absorb(access(0x21, DeviceErrorDirection::Write, Some(1)), at(1, 3)));
        let report = log.report();
        assert_eq!((report.events, report.groups.len()), (2, 1));
        assert_eq!(report.stopped, Some(DeviceErrorLogStop::GroupLimit));
    }

    #[test]
    fn a_clock_overflow_is_not_absorbable() {
        assert!(!absorbable(&DeviceError::ClockOverflow));
        assert!(absorbable(&DeviceError::UnsupportedConfiguration));
        assert!(absorbable(&DeviceError::ReadOnly));
    }
}
