//! BSP event lifecycle and cooperative TPL dispatch from UEFI 2.11 §7.1.

use super::state;
use crate::uefi::{guid::Guid, status::Status};
use core::ffi::c_void;
use core::ptr;

mod runtime;
mod timer;
mod watchdog;

use runtime::RuntimeEventRegistrations;
pub(crate) use runtime::RuntimeEventTable;
pub use runtime::{EVT_RUNTIME, EVT_SIGNAL_VIRTUAL_ADDRESS_CHANGE, VIRTUAL_ADDRESS_CHANGE};
pub use timer::{
    EVT_TIMER, TIMER_CANCEL, TIMER_PERIODIC, TIMER_RELATIVE, set_timer, stall, wait_for_event,
};
use timer::{Timer, TimerClock};
pub use watchdog::set_watchdog_timer;
use watchdog::{Watchdog, dispatch_watchdog};

pub type Event = *mut c_void;
pub type Tpl = usize;
pub type EventNotify = unsafe extern "efiapi" fn(Event, *mut c_void);

pub const TPL_APPLICATION: Tpl = 4;
pub const TPL_CALLBACK: Tpl = 8;
pub const TPL_NOTIFY: Tpl = 16;
pub const TPL_HIGH_LEVEL: Tpl = 31;

pub const EVT_NOTIFY_WAIT: u32 = 0x100;
pub const EVT_NOTIFY_SIGNAL: u32 = 0x200;
pub const EVT_SIGNAL_EXIT_BOOT_SERVICES: u32 = 0x201;
const MAX_EVENTS: usize = 64;

#[cfg(target_os = "none")]
const READY_TO_BOOT: Guid = Guid {
    data1: 0x7ce8_8fb3,
    data2: 0x4bd7,
    data3: 0x4679,
    data4: [0x87, 0xa8, 0xa8, 0xd8, 0xde, 0xe5, 0x0d, 0x2b],
};
#[cfg(target_os = "none")]
const AFTER_READY_TO_BOOT: Guid = Guid {
    data1: 0x3a2a_00ad,
    data2: 0x98b9,
    data3: 0x4cdf,
    data4: [0xa4, 0x78, 0x70, 0x27, 0x77, 0xf1, 0xc1, 0x0b],
};

#[derive(Clone, Copy)]
struct Notify {
    callback: EventNotify,
    context: *mut c_void,
    tpl: Tpl,
}

#[derive(Clone, Copy)]
struct Entry {
    event: Event,
    event_type: u32,
    notify: Option<Notify>,
    group: Option<Guid>,
    signaled: bool,
    queued: bool,
}

#[derive(Clone, Copy)]
struct Dispatch {
    event: Event,
    notify: Notify,
    old_tpl: Tpl,
}

pub(crate) struct EventRegistry {
    entries: [Option<Entry>; MAX_EVENTS],
    timers: [Option<Timer>; MAX_EVENTS],
    clock: Option<TimerClock>,
    timers_active: bool,
    watchdog: Watchdog,
    runtime: RuntimeEventRegistrations,
    pending: [usize; MAX_EVENTS],
    pending_count: usize,
    last_event: usize,
    current_tpl: Tpl,
}

impl EventRegistry {
    pub(crate) const fn new() -> Self {
        Self {
            entries: [None; MAX_EVENTS],
            timers: [None; MAX_EVENTS],
            clock: None,
            timers_active: true,
            watchdog: Watchdog::new(),
            runtime: RuntimeEventRegistrations::new(),
            pending: [0; MAX_EVENTS],
            pending_count: 0,
            last_event: 0,
            current_tpl: TPL_APPLICATION,
        }
    }

    fn prepare_event(
        &self,
        event_type: u32,
        notify_tpl: Tpl,
        function: Option<EventNotify>,
        context: *mut c_void,
        group: Option<Guid>,
    ) -> Result<(usize, Entry), Status> {
        let notify = match event_type & !EVT_TIMER {
            0 => None,
            EVT_NOTIFY_WAIT | EVT_NOTIFY_SIGNAL
                if matches!(notify_tpl, TPL_CALLBACK | TPL_NOTIFY) =>
            {
                Some(Notify {
                    callback: function.ok_or(Status::INVALID_PARAMETER)?,
                    context,
                    tpl: notify_tpl,
                })
            }
            _ => return Err(Status::INVALID_PARAMETER),
        };
        let slot = self
            .entries
            .iter()
            .position(|entry| entry.is_none())
            .ok_or(Status::OUT_OF_RESOURCES)?;
        let token = self
            .last_event
            .checked_add(1)
            .ok_or(Status::OUT_OF_RESOURCES)?;
        let event = ptr::without_provenance_mut(token);
        Ok((
            slot,
            Entry {
                event,
                event_type,
                notify,
                group,
                signaled: false,
                queued: false,
            },
        ))
    }

    fn commit_event(&mut self, slot: usize, entry: Entry) -> Event {
        self.entries[slot] = Some(entry);
        self.last_event = entry.event.addr();
        entry.event
    }

    #[cfg(test)]
    fn create(
        &mut self,
        event_type: u32,
        notify_tpl: Tpl,
        function: Option<EventNotify>,
        context: *mut c_void,
        group: Option<Guid>,
    ) -> Result<Event, Status> {
        let (slot, entry) = self.prepare_event(event_type, notify_tpl, function, context, group)?;
        Ok(self.commit_event(slot, entry))
    }

    pub(super) fn runtime_event_table(&self) -> *mut RuntimeEventTable {
        self.runtime.root()
    }

    fn find(&self, event: Event) -> Result<usize, Status> {
        self.entries
            .iter()
            .position(|entry| entry.is_some_and(|entry| entry.event == event))
            .ok_or(Status::INVALID_PARAMETER)
    }

    fn enqueue(&mut self, index: usize) {
        let Some(entry) = &mut self.entries[index] else {
            return;
        };
        if !entry.queued && entry.notify.is_some() {
            self.pending[self.pending_count] = index;
            self.pending_count += 1;
            entry.queued = true;
        }
    }

    fn remove_pending(&mut self, position: usize) -> usize {
        let index = self.pending[position];
        self.pending
            .copy_within(position + 1..self.pending_count, position);
        self.pending_count -= 1;
        index
    }

    fn signal_one(&mut self, index: usize) {
        let Some(entry) = &mut self.entries[index] else {
            return;
        };
        if entry.signaled {
            return;
        }
        entry.signaled = true;
        if entry.event_type & EVT_NOTIFY_SIGNAL != 0 {
            self.enqueue(index);
        }
    }

    pub(super) fn contains(&self, event: Event) -> bool {
        self.find(event).is_ok()
    }

    pub(super) fn signal(&mut self, event: Event) -> Status {
        let Ok(index) = self.find(event) else {
            return Status::INVALID_PARAMETER;
        };
        self.signal_at(index);
        Status::SUCCESS
    }

    fn signal_at(&mut self, index: usize) {
        if let Some(group) = self.entries[index].and_then(|entry| entry.group) {
            self.signal_group(group);
        } else {
            self.signal_one(index);
        }
    }

    pub(super) fn signal_group(&mut self, group: Guid) {
        for index in 0..MAX_EVENTS {
            if self.entries[index].is_some_and(|entry| entry.group == Some(group)) {
                self.signal_one(index);
            }
        }
    }

    pub(super) fn current_tpl(&self) -> Tpl {
        self.current_tpl
    }

    pub(super) fn disable_timers(&mut self) {
        self.timers_active = false;
        self.watchdog.cancel();
        self.timers.fill(None);
        for entry in self.entries.iter_mut().flatten() {
            if entry.event_type & EVT_TIMER != 0 {
                entry.signaled = false;
                entry.queued = false;
            }
        }
        for position in (0..self.pending_count).rev() {
            let index = self.pending[position];
            if self.entries[index].is_some_and(|entry| entry.event_type & EVT_TIMER != 0) {
                self.remove_pending(position);
            }
        }
    }

    fn check(&mut self, event: Event, queue_wait: bool) -> Status {
        let Ok(index) = self.find(event) else {
            return Status::INVALID_PARAMETER;
        };
        let Some(entry) = &mut self.entries[index] else {
            return Status::INVALID_PARAMETER;
        };
        if entry.event_type & EVT_NOTIFY_SIGNAL != 0 {
            return Status::INVALID_PARAMETER;
        }
        if entry.signaled {
            entry.signaled = false;
            return Status::SUCCESS;
        }
        if queue_wait && entry.event_type & EVT_NOTIFY_WAIT != 0 {
            self.enqueue(index);
        }
        Status::NOT_READY
    }

    fn close(&mut self, event: Event) -> Status {
        let Ok(index) = self.find(event) else {
            return Status::INVALID_PARAMETER;
        };
        if let Some(position) = self.pending[..self.pending_count]
            .iter()
            .position(|&queued| queued == index)
        {
            self.remove_pending(position);
        }
        self.entries[index] = None;
        self.timers[index] = None;
        self.runtime.forget(index);
        Status::SUCCESS
    }

    fn take_notification(&mut self) -> Option<Dispatch> {
        let mut priority = self.current_tpl;
        let mut selected = None;
        for (position, &index) in self.pending[..self.pending_count].iter().enumerate() {
            if let Some(notify) = self.entries[index].and_then(|entry| entry.notify)
                && notify.tpl > priority
            {
                priority = notify.tpl;
                selected = Some((position, notify));
            }
        }
        let (position, notify) = selected?;
        let index = self.remove_pending(position);
        let entry = self.entries[index].as_mut()?;
        entry.queued = false;
        if entry.event_type & EVT_NOTIFY_SIGNAL != 0 {
            // Handling a signal notification consumes the signal. A callback
            // may signal the same event again for a later dispatch.
            entry.signaled = false;
        }
        let old_tpl = self.current_tpl;
        self.current_tpl = notify.tpl;
        Some(Dispatch {
            event: entry.event,
            notify,
            old_tpl,
        })
    }

    fn raise_tpl(&mut self, new_tpl: Tpl) -> Tpl {
        let old = self.current_tpl;
        if valid_tpl(new_tpl) && new_tpl >= old {
            self.current_tpl = new_tpl;
        }
        old
    }

    pub(super) fn restore_tpl(&mut self, old_tpl: Tpl) -> bool {
        if !valid_tpl(old_tpl) || old_tpl > self.current_tpl {
            return false;
        }
        self.current_tpl = old_tpl;
        true
    }
}

fn valid_tpl(tpl: Tpl) -> bool {
    matches!(
        tpl,
        TPL_APPLICATION | TPL_CALLBACK | TPL_NOTIFY | TPL_HIGH_LEVEL
    )
}

/// Call on the BSP after dropping all references into the service state.
pub(super) unsafe fn dispatch_pending() {
    unsafe { dispatch_watchdog() };
    loop {
        // Only scalar values and the guest callback pointer cross the guard.
        let Some(dispatch) =
            (unsafe { state::try_with(|state| state.events.take_notification()).flatten() })
        else {
            return;
        };
        unsafe { (dispatch.notify.callback)(dispatch.event, dispatch.notify.context) };
        // A nested callback may close the event or queue more work. No
        // reference to the selected entry is retained.
        let _ = unsafe {
            state::try_with(|state| {
                state.events.current_tpl = dispatch.old_tpl;
            })
        };
    }
}

/// Drive timer notifications during a protocol's polling call.
///
/// # Safety
/// Call on the BSP without holding any borrow of firmware-private state.
#[cfg(target_os = "none")]
pub(crate) unsafe fn poll_due_events() {
    unsafe {
        let _ = state::try_with(|state| state.events.poll_clock());
        dispatch_pending();
    }
}

/// Signal the two boot groups in order, outside all caller-held state borrows.
#[cfg(target_os = "none")]
pub(crate) unsafe fn signal_ready_to_boot() -> Status {
    for group in [READY_TO_BOOT, AFTER_READY_TO_BOOT] {
        let result = unsafe { state::try_with(|state| state.events.signal_group(group)) };
        if result.is_none() {
            return Status::NOT_READY;
        }
        unsafe { dispatch_pending() };
    }
    Status::SUCCESS
}

/// # Safety
/// Call on the BSP. A non-null event output must be aligned, writable and
/// outside firmware-private state. Notify callbacks obey the EFI ABI.
pub unsafe extern "efiapi" fn create_event(
    event_type: u32,
    notify_tpl: Tpl,
    function: Option<EventNotify>,
    context: *mut c_void,
    event: *mut Event,
) -> Status {
    let (kind, group, runtime_kind) = match event_type {
        EVT_SIGNAL_EXIT_BOOT_SERVICES => (
            EVT_NOTIFY_SIGNAL,
            Some(super::exit::EXIT_BOOT_SERVICES),
            None,
        ),
        EVT_SIGNAL_VIRTUAL_ADDRESS_CHANGE => (
            EVT_NOTIFY_SIGNAL,
            Some(VIRTUAL_ADDRESS_CHANGE),
            Some(EVT_SIGNAL_VIRTUAL_ADDRESS_CHANGE),
        ),
        other => (
            other & !EVT_RUNTIME,
            None,
            (other & EVT_RUNTIME != 0).then_some(other),
        ),
    };
    unsafe {
        publish_event(
            kind,
            notify_tpl,
            function,
            context,
            group,
            runtime_kind,
            event,
        )
    }
}

/// # Safety
/// Call on the BSP. Optional group storage is readable and aligned. A
/// non-null event output is aligned and writable outside private state.
pub unsafe extern "efiapi" fn create_event_ex(
    event_type: u32,
    notify_tpl: Tpl,
    function: Option<EventNotify>,
    context: *const c_void,
    group: *const Guid,
    event: *mut Event,
) -> Status {
    if event.is_null()
        || matches!(
            event_type,
            EVT_SIGNAL_EXIT_BOOT_SERVICES | EVT_SIGNAL_VIRTUAL_ADDRESS_CHANGE
        )
    {
        return Status::INVALID_PARAMETER;
    }
    let group = if group.is_null() {
        None
    } else {
        Some(unsafe { group.read() })
    };
    let runtime_kind = (event_type & EVT_RUNTIME != 0 || group == Some(VIRTUAL_ADDRESS_CHANGE))
        .then_some(event_type | EVT_RUNTIME);
    unsafe {
        publish_event(
            event_type & !EVT_RUNTIME,
            notify_tpl,
            function,
            context.cast_mut(),
            group,
            runtime_kind,
            event,
        )
    }
}

unsafe fn publish_event(
    event_type: u32,
    notify_tpl: Tpl,
    function: Option<EventNotify>,
    context: *mut c_void,
    group: Option<Guid>,
    runtime_type: Option<u32>,
    event: *mut Event,
) -> Status {
    if event.is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe {
        state::with(|state| {
            let (slot, entry) = match state
                .events
                .prepare_event(event_type, notify_tpl, function, context, group)
            {
                Ok(entry) => entry,
                Err(status) => return status,
            };
            if let Some(runtime_type) = runtime_type {
                if !state.exit.can_change_map() {
                    return Status::ACCESS_DENIED;
                }
                if state.runtime_services.is_none() {
                    return Status::NOT_READY;
                }
                if let Err(status) =
                    state
                        .events
                        .runtime
                        .retain(state.allocator, slot, entry, runtime_type)
                {
                    return status;
                }
            }
            event.write(state.events.commit_event(slot, entry));
            Status::SUCCESS
        })
    }
}

/// # Safety
/// Call on the BSP. Registered callbacks may execute before return.
pub unsafe extern "efiapi" fn signal_event(event: Event) -> Status {
    let status = unsafe {
        state::with(|state| {
            state.events.poll_clock();
            state.events.signal(event)
        })
    };
    unsafe { dispatch_pending() };
    status
}

/// # Safety
/// Call on the BSP. Registered wait callbacks may execute before return.
pub unsafe extern "efiapi" fn check_event(event: Event) -> Status {
    let status = unsafe {
        state::with(|state| {
            state.events.poll_clock();
            state.events.check(event, true)
        })
    };
    unsafe { dispatch_pending() };
    if status != Status::NOT_READY {
        return status;
    }
    unsafe { state::with(|state| state.events.check(event, false)) }
}

/// # Safety
/// Call on the BSP.
pub unsafe extern "efiapi" fn close_event(event: Event) -> Status {
    unsafe {
        state::with(|state| {
            let status = state.events.close(event);
            if status == Status::SUCCESS {
                state.protocols.unregister_event(event);
            }
            status
        })
    }
}

/// # Safety
/// Call on the BSP with a valid TPL at or above the current level.
pub unsafe extern "efiapi" fn raise_tpl(new_tpl: Tpl) -> Tpl {
    let old_tpl = unsafe {
        state::try_with(|state| {
            state.events.poll_clock();
            let previous = state.events.raise_tpl(new_tpl);
            #[cfg(target_os = "none")]
            if new_tpl == TPL_HIGH_LEVEL && previous < TPL_HIGH_LEVEL {
                core::arch::asm!("cli", options(nostack));
            }
            previous
        })
    }
    .unwrap_or(TPL_APPLICATION);
    unsafe { dispatch_watchdog() };
    old_tpl
}

/// # Safety
/// Call on the BSP with the TPL returned by the corresponding RaiseTPL.
pub unsafe extern "efiapi" fn restore_tpl(old_tpl: Tpl) {
    let restored_from = unsafe {
        state::try_with(|state| {
            let previous = state.events.current_tpl();
            if !state.events.restore_tpl(old_tpl) {
                return None;
            }
            state.events.poll_clock();
            Some(previous)
        })
    }
    .flatten();
    if restored_from.is_some() {
        #[cfg(target_os = "none")]
        if restored_from == Some(TPL_HIGH_LEVEL) && old_tpl < TPL_HIGH_LEVEL {
            unsafe { core::arch::asm!("sti", options(nostack)) };
        }
        unsafe { dispatch_pending() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "efiapi" fn callback(_: Event, _: *mut c_void) {}

    fn event(registry: &mut EventRegistry, event_type: u32, tpl: Tpl) -> Event {
        registry
            .create(event_type, tpl, Some(callback), ptr::null_mut(), None)
            .unwrap()
    }

    #[test]
    fn dispatch_is_priority_ordered_fifo_and_coalesces_while_masked() {
        let mut registry = EventRegistry::new();
        let first = event(&mut registry, EVT_NOTIFY_SIGNAL, TPL_CALLBACK);
        let second = event(&mut registry, EVT_NOTIFY_SIGNAL, TPL_CALLBACK);
        let high = event(&mut registry, EVT_NOTIFY_SIGNAL, TPL_NOTIFY);
        assert_eq!(registry.raise_tpl(TPL_NOTIFY), TPL_APPLICATION);
        for token in [first, second, first, high] {
            assert_eq!(registry.signal(token), Status::SUCCESS);
        }
        assert_eq!(registry.pending_count, 3);
        assert!(registry.take_notification().is_none());
        assert!(registry.restore_tpl(TPL_APPLICATION));

        for (token, tpl) in [
            (high, TPL_NOTIFY),
            (first, TPL_CALLBACK),
            (second, TPL_CALLBACK),
        ] {
            let notification = registry.take_notification().unwrap();
            assert_eq!(notification.event, token);
            assert_eq!(registry.current_tpl, tpl);
            assert_eq!(notification.old_tpl, TPL_APPLICATION);
            assert!(registry.restore_tpl(notification.old_tpl));
        }
        assert!(registry.take_notification().is_none());
        assert_eq!(registry.signal(first), Status::SUCCESS);
        assert_eq!(registry.take_notification().unwrap().event, first);
    }

    #[test]
    fn wait_consumes_signals_and_close_cancels_pending_work() {
        let mut registry = EventRegistry::new();
        let wait = event(&mut registry, EVT_NOTIFY_WAIT, TPL_CALLBACK);
        assert_eq!(registry.check(wait, true), Status::NOT_READY);
        assert_eq!(registry.check(wait, true), Status::NOT_READY);
        assert_eq!(registry.pending_count, 1);
        let notification = registry.take_notification().unwrap();
        assert_eq!(notification.event, wait);
        assert_eq!(registry.signal(wait), Status::SUCCESS);
        assert!(registry.restore_tpl(notification.old_tpl));
        assert_eq!(registry.check(wait, false), Status::SUCCESS);
        assert_eq!(registry.check(wait, false), Status::NOT_READY);
        assert_eq!(registry.check(wait, true), Status::NOT_READY);
        assert_eq!(registry.close(wait), Status::SUCCESS);
        assert!(registry.take_notification().is_none());
        let later = event(&mut registry, EVT_NOTIFY_WAIT, TPL_CALLBACK);
        assert_ne!(later, wait);
        assert_eq!(registry.signal(wait), Status::INVALID_PARAMETER);
        assert_eq!(registry.check(later, false), Status::NOT_READY);
    }

    #[test]
    fn group_members_are_signaled_before_dispatch_and_validate_notification_type() {
        let mut registry = EventRegistry::new();
        let group = Guid {
            data1: 0x2211,
            data2: 0x3344,
            data3: 0x4555,
            data4: [0xaa; 8],
        };
        let plain = registry
            .create(0, 0, None, ptr::null_mut(), Some(group))
            .unwrap();
        let notification = registry
            .create(
                EVT_NOTIFY_SIGNAL,
                TPL_NOTIFY,
                Some(callback),
                ptr::null_mut(),
                Some(group),
            )
            .unwrap();
        assert_eq!(registry.signal(notification), Status::SUCCESS);
        assert_eq!(registry.check(plain, false), Status::SUCCESS);
        assert_eq!(
            registry.check(notification, false),
            Status::INVALID_PARAMETER
        );
        assert_eq!(registry.take_notification().unwrap().event, notification);
        assert_eq!(
            registry.create(
                EVT_NOTIFY_WAIT | EVT_NOTIFY_SIGNAL,
                TPL_CALLBACK,
                Some(callback),
                ptr::null_mut(),
                None
            ),
            Err(Status::INVALID_PARAMETER)
        );
        assert_eq!(
            registry.create(EVT_NOTIFY_WAIT, TPL_CALLBACK, None, ptr::null_mut(), None),
            Err(Status::INVALID_PARAMETER)
        );
        assert_eq!(
            registry.create(
                EVT_NOTIFY_WAIT,
                TPL_APPLICATION,
                Some(callback),
                ptr::null_mut(),
                None
            ),
            Err(Status::INVALID_PARAMETER)
        );
    }
}
