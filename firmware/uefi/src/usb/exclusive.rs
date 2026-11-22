//! One owner of the driver at a time (ISSUE-647).
//!
//! The firmware is single-threaded, but callbacks run inside driver calls: the driver waits with
//! `stall`, which dispatches events, and the console's timer event polls the keyboard. A second
//! `&mut` to the driver from such a callback would alias the first. [`Exclusive::with`] hands
//! the value out once; a nested call finds it busy and gets [`Access::Busy`] instead of a
//! second reference (the caller then simply skips that poll).
//!
//! Pure logic, covered by host tests.

use core::cell::{Cell, UnsafeCell};

#[derive(Debug, Eq, PartialEq)]
pub(super) enum Access<T> {
    /// No value was ever stored.
    Absent,
    /// The value is in use further up the call stack.
    Busy,
    Done(T),
}

pub(super) struct Exclusive<T> {
    busy: Cell<bool>,
    value: UnsafeCell<Option<T>>,
}

// SAFETY: firmware code runs on the BSP only; `with` never hands out two references.
unsafe impl<T> Sync for Exclusive<T> {}

impl<T> Exclusive<T> {
    pub(super) const fn empty() -> Self {
        Self {
            busy: Cell::new(false),
            value: UnsafeCell::new(None),
        }
    }

    /// Stores the value. Returns false (and drops `value`) while the cell is in use.
    pub(super) fn set(&self, value: T) -> bool {
        if self.busy.get() {
            return false;
        }
        // SAFETY: not busy, so no reference to the contents exists.
        unsafe { *self.value.get() = Some(value) };
        true
    }

    pub(super) fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> Access<R> {
        if self.busy.get() {
            return Access::Busy;
        }
        self.busy.set(true);
        // SAFETY: `busy` was clear and stays set until `f` returns: this is the only reference.
        let result = unsafe { (*self.value.get()).as_mut() }.map(f);
        self.busy.set(false);
        match result {
            Some(result) => Access::Done(result),
            None => Access::Absent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_cell_is_absent_and_a_filled_one_runs_the_closure() {
        let cell = Exclusive::<u32>::empty();
        assert_eq!(cell.with(|value| *value), Access::Absent);
        assert!(cell.set(7));
        assert_eq!(
            cell.with(|value| {
                *value += 1;
                *value
            }),
            Access::Done(8)
        );
        assert_eq!(cell.with(|value| *value), Access::Done(8));
    }

    #[test]
    fn a_nested_call_gets_busy_and_the_cell_is_usable_afterwards() {
        let cell = Exclusive::<u32>::empty();
        cell.set(1);
        let outer = cell.with(|value| {
            *value = 2;
            // What a timer callback inside `stall` would do.
            let nested = cell.with(|inner| *inner);
            assert_eq!(nested, Access::Busy);
            assert!(!cell.set(99), "no replacement while in use");
            *value
        });
        assert_eq!(outer, Access::Done(2));
        assert_eq!(cell.with(|value| *value), Access::Done(2));
    }

    #[test]
    fn busy_is_released_after_each_call() {
        let cell = Exclusive::<()>::empty();
        // Even the call that finds no value leaves the cell free.
        assert_eq!(cell.with(|_| ()), Access::Absent);
        assert!(cell.set(()));
        for _ in 0..3 {
            assert_eq!(cell.with(|_| ()), Access::Done(()));
        }
    }
}
