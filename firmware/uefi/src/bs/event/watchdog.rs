//! Boot watchdog driven by the shared timer polling clock (UEFI 2.11 §7.5.1).
use super::EventRegistry;
use crate::bs::state;
use crate::uefi::status::Status;

const FIRST_LOADER_WATCHDOG_CODE: u64 = 0x1_0000;

#[derive(Clone, Copy)]
struct ResetTarget {
    port: u16,
    value: u8,
    reset: fn(u16, u8, u64) -> !,
}

#[derive(Clone, Copy)]
enum WatchdogState {
    Disarmed,
    Armed { deadline: u128, code: u64 },
    Expired(u64),
}

pub(super) struct Watchdog {
    target: Option<ResetTarget>,
    state: WatchdogState,
}

impl Watchdog {
    pub(super) const fn new() -> Self {
        Self {
            target: None,
            state: WatchdogState::Disarmed,
        }
    }

    fn arm(&mut self, now: u64, hz: u64, seconds: usize, code: u64) -> Status {
        if code < FIRST_LOADER_WATCHDOG_CODE {
            return Status::INVALID_PARAMETER;
        }
        if self.target.is_none() {
            return Status::UNSUPPORTED;
        }
        self.state = WatchdogState::Armed {
            deadline: u128::from(now) + seconds as u128 * u128::from(hz),
            code,
        };
        Status::SUCCESS
    }

    pub(super) fn cancel(&mut self) {
        self.state = WatchdogState::Disarmed;
    }

    pub(super) fn poll(&mut self, now: u64) {
        if let WatchdogState::Armed { deadline, code } = self.state
            && u128::from(now) >= deadline
        {
            self.state = WatchdogState::Expired(code);
        }
    }

    fn take_reset(&mut self) -> Option<(ResetTarget, u64)> {
        let WatchdogState::Expired(code) = self.state else {
            return None;
        };
        self.cancel();
        self.target.map(|target| (target, code))
    }
}

impl EventRegistry {
    pub(in crate::bs) fn set_watchdog_target(
        &mut self,
        port: u16,
        value: u8,
        reset: fn(u16, u8, u64) -> !,
    ) {
        self.watchdog.target = Some(ResetTarget { port, value, reset });
    }
}

/// # Safety
/// Call on the BSP. WatchdogData is optional caller-owned data. Only the
/// numeric code is kept for the expiration report.
pub unsafe extern "efiapi" fn set_watchdog_timer(
    timeout: usize,
    watchdog_code: u64,
    _data_size: usize,
    _watchdog_data: *const u16,
) -> Status {
    unsafe {
        state::with(|state| {
            if timeout == 0 {
                state.events.watchdog.cancel();
                return Status::SUCCESS;
            }
            let Some(clock) = state.events.clock.filter(|_| state.events.timers_active) else {
                return Status::DEVICE_ERROR;
            };
            state
                .events
                .watchdog
                .arm((clock.read)(), clock.hz, timeout, watchdog_code)
        })
    }
}

/// Execute a reset with no references into Boot Services state. Its dispatch
/// is independent of notification TPL.
pub(super) unsafe fn dispatch_watchdog() {
    if let Some((target, code)) =
        unsafe { state::try_with(|state| state.events.watchdog.take_reset()).flatten() }
    {
        (target.reset)(target.port, target.value, code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_reset(_: u16, _: u8, _: u64) -> ! {
        panic!("unit tests inspect the pending reset without issuing it")
    }

    #[test]
    fn watchdog_rearm_replaces_the_deadline_and_expiry_is_consumed_once() {
        let mut events = EventRegistry::new();
        assert_eq!(events.watchdog.arm(0, 10, 1, 0x10000), Status::UNSUPPORTED);
        events.set_watchdog_target(0x60c, 6, no_reset);
        assert_eq!(events.watchdog.arm(100, 10, 1, 0x10000), Status::SUCCESS);
        assert_eq!(
            events.watchdog.arm(104, 10, 1, 0xffff),
            Status::INVALID_PARAMETER
        );
        assert_eq!(events.watchdog.arm(105, 10, 2, 0x10042), Status::SUCCESS);
        events.watchdog.poll(124);
        assert!(events.watchdog.take_reset().is_none());
        events.watchdog.poll(125);
        let (target, code) = events.watchdog.take_reset().unwrap();
        assert_eq!((target.port, target.value, code), (0x60c, 6, 0x10042));
        events.watchdog.poll(500);
        assert!(events.watchdog.take_reset().is_none());
    }

    #[test]
    fn watchdog_cancellation_and_boot_exit_discard_armed_or_due_reset() {
        let mut events = EventRegistry::new();
        events.set_watchdog_target(0x60c, 6, no_reset);
        assert_eq!(events.watchdog.arm(0, 5, 1, 0x10000), Status::SUCCESS);
        events.watchdog.cancel();
        events.watchdog.poll(5);
        assert!(events.watchdog.take_reset().is_none());

        assert_eq!(events.watchdog.arm(5, 5, 1, 0x10000), Status::SUCCESS);
        events.watchdog.poll(10);
        events.disable_timers();
        assert!(events.watchdog.take_reset().is_none());
    }

    #[test]
    fn the_full_timeout_width_fits_the_watchdog_clock_deadline() {
        let mut events = EventRegistry::new();
        events.set_watchdog_target(0x60c, 6, no_reset);
        assert_eq!(
            events.watchdog.arm(u64::MAX, u64::MAX, usize::MAX, 0x10000),
            Status::SUCCESS,
        );
        events.watchdog.poll(u64::MAX);
        assert!(events.watchdog.take_reset().is_none());
    }
}
