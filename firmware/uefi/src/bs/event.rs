//! BSP event lifecycle and cooperative TPL dispatch from UEFI 2.11 §7.1.

use super::state;
use crate::uefi::{guid::Guid, status::Status};
use core::ffi::c_void;
use core::ptr;

pub type Event = *mut c_void;
pub type Tpl = usize;
pub type EventNotify = unsafe extern "efiapi" fn(Event, *mut c_void);

pub const TPL_APPLICATION: Tpl = 4;
pub const TPL_CALLBACK: Tpl = 8;
pub const TPL_NOTIFY: Tpl = 16;
pub const TPL_HIGH_LEVEL: Tpl = 31;

pub const EVT_NOTIFY_WAIT: u32 = 0x100;
pub const EVT_NOTIFY_SIGNAL: u32 = 0x200;
const MAX_EVENTS: usize = 64;

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
    pending: [usize; MAX_EVENTS],
    pending_count: usize,
    last_event: usize,
    current_tpl: Tpl,
}

impl EventRegistry {
    pub(crate) const fn new() -> Self {
        Self {
            entries: [None; MAX_EVENTS],
            pending: [0; MAX_EVENTS],
            pending_count: 0,
            last_event: 0,
            current_tpl: TPL_APPLICATION,
        }
    }

    fn create(
        &mut self,
        event_type: u32,
        notify_tpl: Tpl,
        function: Option<EventNotify>,
        context: *mut c_void,
        group: Option<Guid>,
    ) -> Result<Event, Status> {
        let notify = match event_type {
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
            .iter_mut()
            .find(|entry| entry.is_none())
            .ok_or(Status::OUT_OF_RESOURCES)?;
        let token = self
            .last_event
            .checked_add(1)
            .ok_or(Status::OUT_OF_RESOURCES)?;
        let event = ptr::without_provenance_mut(token);
        *slot = Some(Entry {
            event,
            event_type,
            notify,
            group,
            signaled: false,
            queued: false,
        });
        self.last_event = token;
        Ok(event)
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
        if entry.event_type == EVT_NOTIFY_SIGNAL {
            self.enqueue(index);
        }
    }

    fn signal(&mut self, event: Event) -> Status {
        let Ok(index) = self.find(event) else {
            return Status::INVALID_PARAMETER;
        };
        if let Some(group) = self.entries[index].and_then(|entry| entry.group) {
            for index in 0..MAX_EVENTS {
                if self.entries[index].is_some_and(|entry| entry.group == Some(group)) {
                    self.signal_one(index);
                }
            }
        } else {
            self.signal_one(index);
        }
        Status::SUCCESS
    }

    fn check(&mut self, event: Event, queue_wait: bool) -> Status {
        let Ok(index) = self.find(event) else {
            return Status::INVALID_PARAMETER;
        };
        let Some(entry) = &mut self.entries[index] else {
            return Status::INVALID_PARAMETER;
        };
        if entry.event_type == EVT_NOTIFY_SIGNAL {
            return Status::INVALID_PARAMETER;
        }
        if entry.signaled {
            entry.signaled = false;
            return Status::SUCCESS;
        }
        if queue_wait && entry.event_type == EVT_NOTIFY_WAIT {
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
        if entry.event_type == EVT_NOTIFY_SIGNAL {
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

    fn restore_tpl(&mut self, old_tpl: Tpl) -> bool {
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
unsafe fn dispatch_pending() {
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
    unsafe {
        create_event_ex(
            event_type,
            notify_tpl,
            function,
            context,
            ptr::null(),
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
    if event.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let group = if group.is_null() {
        None
    } else {
        Some(unsafe { group.read() })
    };
    unsafe {
        state::with(|state| {
            match state
                .events
                .create(event_type, notify_tpl, function, context.cast_mut(), group)
            {
                Ok(created) => {
                    event.write(created);
                    Status::SUCCESS
                }
                Err(status) => status,
            }
        })
    }
}

/// # Safety
/// Call on the BSP. Registered callbacks may execute before return.
pub unsafe extern "efiapi" fn signal_event(event: Event) -> Status {
    let status = unsafe { state::with(|state| state.events.signal(event)) };
    if status == Status::SUCCESS {
        unsafe { dispatch_pending() };
    }
    status
}

/// # Safety
/// Call on the BSP. Registered wait callbacks may execute before return.
pub unsafe extern "efiapi" fn check_event(event: Event) -> Status {
    let status = unsafe { state::with(|state| state.events.check(event, true)) };
    if status != Status::NOT_READY {
        return status;
    }
    unsafe {
        dispatch_pending();
        state::with(|state| state.events.check(event, false))
    }
}

/// # Safety
/// Call on the BSP.
pub unsafe extern "efiapi" fn close_event(event: Event) -> Status {
    unsafe { state::with(|state| state.events.close(event)) }
}

/// # Safety
/// Call on the BSP with a valid TPL at or above the current level.
pub unsafe extern "efiapi" fn raise_tpl(new_tpl: Tpl) -> Tpl {
    unsafe { state::try_with(|state| state.events.raise_tpl(new_tpl)) }.unwrap_or(TPL_APPLICATION)
}

/// # Safety
/// Call on the BSP with the TPL returned by the corresponding RaiseTPL.
pub unsafe extern "efiapi" fn restore_tpl(old_tpl: Tpl) {
    if unsafe { state::try_with(|state| state.events.restore_tpl(old_tpl)) } == Some(true) {
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
