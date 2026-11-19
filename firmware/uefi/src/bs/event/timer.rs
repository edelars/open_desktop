//! Cooperative EFI timers backed by the TSC frequency in the handoff.
use super::{Event, EventRegistry, MAX_EVENTS, TPL_APPLICATION, dispatch_pending};
use crate::bs::state;
use crate::uefi::status::Status;

pub const EVT_TIMER: u32 = 0x8000_0000;
pub const TIMER_CANCEL: u32 = 0;
pub const TIMER_PERIODIC: u32 = 1;
pub const TIMER_RELATIVE: u32 = 2;

#[derive(Clone, Copy)]
pub(super) struct TimerClock {
    pub(super) hz: u64,
    pub(super) read: fn() -> u64,
}

impl TimerClock {
    fn ticks_for<const UNITS_PER_SECOND: u64>(self, duration: u64) -> Option<u64> {
        let product = u128::from(duration) * u128::from(self.hz);
        u64::try_from(product.div_ceil(u128::from(UNITS_PER_SECOND))).ok()
    }
}

#[derive(Clone, Copy)]
pub(super) struct Timer {
    deadline: u64,
    // None is a one-shot timer; a periodic interval is always nonzero.
    period: Option<u64>,
}

impl EventRegistry {
    pub(in crate::bs) fn set_clock(&mut self, hz: u64, read: fn() -> u64) {
        self.clock = Some(TimerClock { hz, read });
    }

    fn arm_at(
        &mut self,
        event: Event,
        delay: u32,
        trigger: u64,
        now: u64,
        clock: TimerClock,
    ) -> Status {
        if !self.timers_active {
            return Status::NOT_READY;
        }
        let Ok(index) = self.find(event) else {
            return Status::INVALID_PARAMETER;
        };
        if !self.entries[index].is_some_and(|entry| entry.event_type & EVT_TIMER != 0) {
            return Status::INVALID_PARAMETER;
        }
        if delay == TIMER_CANCEL {
            self.timers[index] = None;
            return Status::SUCCESS;
        }
        if !matches!(delay, TIMER_PERIODIC | TIMER_RELATIVE) {
            return Status::INVALID_PARAMETER;
        }
        let Some(ticks) = clock.ticks_for::<10_000_000>(trigger) else {
            return Status::INVALID_PARAMETER;
        };
        // A zero trigger first fires on the next tick, including periodic mode.
        let ticks = ticks.max(1);
        let Some(deadline) = now.checked_add(ticks) else {
            return Status::INVALID_PARAMETER;
        };
        self.timers[index] = Some(Timer {
            deadline,
            period: (delay == TIMER_PERIODIC).then_some(ticks),
        });
        Status::SUCCESS
    }

    fn poll_at(&mut self, now: u64) {
        if !self.timers_active {
            return;
        }
        self.watchdog.poll(now);
        for index in 0..MAX_EVENTS {
            let Some(timer) = self.timers[index] else {
                continue;
            };
            if now < timer.deadline {
                continue;
            }
            self.timers[index] = timer.period.and_then(|period| {
                // Advance by whole periods from the old deadline. Skipped
                // expirations result in a single signal at this poll.
                let since_deadline = now - timer.deadline;
                let until_next = period - since_deadline % period;
                now.checked_add(until_next).map(|deadline| Timer {
                    deadline,
                    period: Some(period),
                })
            });
            self.signal_at(index);
        }
    }

    pub(super) fn poll_clock(&mut self) {
        if self.timers_active
            && let Some(clock) = self.clock
        {
            self.poll_at((clock.read)());
        }
    }
}

/// # Safety
/// Call on the BSP with an event token returned by CreateEvent or CreateEventEx.
pub unsafe extern "efiapi" fn set_timer(event: Event, delay: u32, trigger: u64) -> Status {
    let status = unsafe {
        state::with(|state| {
            let Some(clock) = state.events.clock else {
                return Status::NOT_READY;
            };
            let now = (clock.read)();
            let status = state.events.arm_at(event, delay, trigger, now, clock);
            state.events.poll_at(now);
            status
        })
    };
    unsafe { dispatch_pending() };
    status
}

/// # Safety
/// Call on the BSP. The aligned event array lives throughout the wait,
/// including any callback. The index output is writable and disjoint
/// from it and from firmware-private state.
pub unsafe extern "efiapi" fn wait_for_event(
    count: usize,
    events: *const Event,
    index: *mut usize,
) -> Status {
    let Some(tpl) = (unsafe { state::try_with(|state| state.events.current_tpl) }) else {
        return Status::NOT_READY;
    };
    if tpl != TPL_APPLICATION {
        return Status::UNSUPPORTED;
    }
    if count == 0
        || count > isize::MAX as usize / size_of::<Event>()
        || events.is_null()
        || index.is_null()
    {
        return Status::INVALID_PARAMETER;
    }
    loop {
        for position in 0..count {
            // Read one token at a time so the callback cannot alias a Rust
            // reference to the caller's event array.
            let event = unsafe { events.add(position).read() };
            let status = unsafe { super::check_event(event) };
            if status != Status::NOT_READY {
                unsafe { index.write(position) };
                return status;
            }
        }
    }
}

/// # Safety
/// Call on the BSP.
pub unsafe extern "efiapi" fn stall(microseconds: usize) -> Status {
    let Some(clock) = (unsafe { state::try_with(|state| state.events.clock).flatten() }) else {
        return Status::NOT_READY;
    };
    let Some(ticks) = clock.ticks_for::<1_000_000>(microseconds as u64) else {
        return Status::INVALID_PARAMETER;
    };
    let Some(deadline) = (clock.read)().checked_add(ticks) else {
        return Status::INVALID_PARAMETER;
    };
    loop {
        let now = (clock.read)();
        unsafe {
            let _ = state::try_with(|state| state.events.poll_at(now));
            dispatch_pending();
        }
        if now >= deadline {
            return Status::SUCCESS;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{EVT_NOTIFY_SIGNAL, EVT_NOTIFY_WAIT, EventNotify, TPL_CALLBACK, TPL_NOTIFY};
    use super::*;
    use core::{ffi::c_void, ptr};

    fn clock(hz: u64) -> TimerClock {
        TimerClock { hz, read: || 0 }
    }

    fn create(registry: &mut EventRegistry, kind: u32, notify: Option<EventNotify>) -> Event {
        registry
            .create(kind, TPL_CALLBACK, notify, ptr::null_mut(), None)
            .unwrap()
    }

    unsafe extern "efiapi" fn callback(_: Event, _: *mut c_void) {}

    #[test]
    fn relative_timer_rounds_up_and_zero_uses_the_next_tick() {
        let clock = clock(3_333_333);
        let mut events = EventRegistry::new();
        let event = create(&mut events, EVT_TIMER, None);
        assert_eq!(clock.ticks_for::<10_000_000>(4), Some(2));
        assert_eq!(
            events.arm_at(event, TIMER_RELATIVE, 4, 100, clock),
            Status::SUCCESS
        );
        events.poll_at(101);
        assert_eq!(events.check(event, false), Status::NOT_READY);
        events.poll_at(102);
        assert_eq!(events.check(event, false), Status::SUCCESS);
        events.poll_at(500);
        assert_eq!(events.check(event, false), Status::NOT_READY);
        assert_eq!(
            events.arm_at(event, TIMER_RELATIVE, 0, 500, clock),
            Status::SUCCESS
        );
        events.poll_at(500);
        assert_eq!(events.check(event, false), Status::NOT_READY);
        events.poll_at(501);
        assert_eq!(events.check(event, false), Status::SUCCESS);
    }

    #[test]
    fn periodic_timer_keeps_phase_and_coalesces_masked_expirations() {
        let clock = clock(10_000_000);
        let mut events = EventRegistry::new();
        let event = create(&mut events, EVT_TIMER | EVT_NOTIFY_SIGNAL, Some(callback));
        assert_eq!(
            events.arm_at(event, TIMER_PERIODIC, 10, 100, clock),
            Status::SUCCESS
        );
        let application = events.raise_tpl(TPL_NOTIFY);
        events.poll_at(137);
        assert_eq!(events.timers[0].unwrap().deadline, 140);
        events.poll_at(149);
        assert_eq!(events.timers[0].unwrap().deadline, 150);
        assert_eq!(events.pending_count, 1);
        assert!(events.take_notification().is_none());
        assert!(events.restore_tpl(application));
        assert_eq!(events.take_notification().unwrap().event, event);
        assert!(events.restore_tpl(application));
        events.poll_at(149);
        assert!(events.take_notification().is_none());
        events.poll_at(150);
        assert_eq!(events.take_notification().unwrap().event, event);
    }

    #[test]
    fn cancelling_and_closing_remove_deadlines_and_bad_input_preserves_them() {
        let clock = clock(10_000_000);
        let mut events = EventRegistry::new();
        let plain = create(&mut events, 0, None);
        assert_eq!(
            events.arm_at(plain, TIMER_CANCEL, 0, 0, clock),
            Status::INVALID_PARAMETER
        );
        let event = create(&mut events, EVT_TIMER | EVT_NOTIFY_WAIT, Some(callback));
        let index = events.find(event).unwrap();
        assert_eq!(
            events.arm_at(event, TIMER_PERIODIC, 0, 10, clock),
            Status::SUCCESS
        );
        assert_eq!(events.timers[index].unwrap().deadline, 11);
        assert_eq!(
            events.arm_at(event, TIMER_RELATIVE, 2, u64::MAX - 1, clock),
            Status::INVALID_PARAMETER
        );
        assert_eq!(events.timers[index].unwrap().deadline, 11);
        assert_eq!(
            events.arm_at(event, TIMER_CANCEL, u64::MAX, 10, clock),
            Status::SUCCESS
        );
        events.poll_at(20);
        assert_eq!(events.check(event, false), Status::NOT_READY);
        assert_eq!(
            events.arm_at(event, TIMER_RELATIVE, 1, 20, clock),
            Status::SUCCESS
        );
        assert_eq!(events.close(event), Status::SUCCESS);
        let replacement = create(&mut events, EVT_TIMER, None);
        assert_eq!(events.find(replacement).unwrap(), index);
        events.poll_at(21);
        assert_eq!(events.check(replacement, false), Status::NOT_READY);
    }

    #[test]
    fn exit_drops_scheduled_and_pending_timer_activity() {
        let clock = clock(10_000_000);
        let mut events = EventRegistry::new();
        events.set_clock(clock.hz, clock.read);
        let timer = create(&mut events, EVT_TIMER | EVT_NOTIFY_SIGNAL, Some(callback));
        let notification = create(&mut events, EVT_NOTIFY_SIGNAL, Some(callback));
        assert_eq!(
            events.arm_at(timer, TIMER_PERIODIC, 1, 0, clock),
            Status::SUCCESS
        );
        let old_tpl = events.raise_tpl(TPL_NOTIFY);
        events.poll_at(1);
        assert_eq!(events.signal(notification), Status::SUCCESS);
        assert_eq!(events.pending_count, 2);
        events.disable_timers();
        assert!(events.clock.is_some());
        assert!(events.timers.iter().all(Option::is_none));
        assert!(!events.entries[0].unwrap().signaled);
        assert_eq!(
            events.arm_at(timer, TIMER_PERIODIC, 1, 2, clock),
            Status::NOT_READY,
        );
        events.poll_at(100);
        assert!(events.restore_tpl(old_tpl));
        assert_eq!(events.take_notification().unwrap().event, notification);
        assert!(events.restore_tpl(old_tpl));
        assert!(events.take_notification().is_none());
    }
}
