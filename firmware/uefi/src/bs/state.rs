//! Shared state of the boot processor's EFI services.

use super::event::EventRegistry;
use super::protocol::ProtocolRegistry;
use crate::mem::pages::PageAllocator;
use crate::uefi::status::Status;
use core::cell::{Cell, UnsafeCell};

struct BspServices {
    busy: Cell<bool>,
    allocator: UnsafeCell<Option<PageAllocator<'static, 'static>>>,
    protocols: UnsafeCell<ProtocolRegistry>,
    events: UnsafeCell<EventRegistry>,
}

// SAFETY: only the BSP accesses these fields. Unsafe entrances require the
// BSP and the guard prevents nested mutable borrows. The guarded operations
// never dispatch notifications or yield control to an EFI image.
unsafe impl Sync for BspServices {}

static BOOT_SERVICES: BspServices = BspServices {
    busy: Cell::new(false),
    allocator: UnsafeCell::new(None),
    protocols: UnsafeCell::new(ProtocolRegistry::new()),
    events: UnsafeCell::new(EventRegistry::new()),
};

pub(crate) struct ServiceState<'a> {
    pub allocator: &'a mut PageAllocator<'static, 'static>,
    pub protocols: &'a mut ProtocolRegistry,
    pub events: &'a mut EventRegistry,
}

struct BusyGuard<'a>(&'a Cell<bool>);

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// Takes ownership of the live memory map and page allocations.
///
/// # Safety
/// Invoke once on the BSP, before calling or publishing the services.
pub unsafe fn initialize(allocator: PageAllocator<'static, 'static>) {
    // SAFETY: initialization precedes the first access and has a single caller.
    unsafe { BOOT_SERVICES.allocator.get().write(Some(allocator)) };
}

/// The caller must be the BSP. The closure must return before invoking a
/// notification or yielding control to an EFI image.
pub(crate) unsafe fn with(operation: impl FnOnce(ServiceState<'_>) -> Status) -> Status {
    unsafe { try_with(operation) }.unwrap_or(Status::NOT_READY)
}

/// Like with(), but returns None when the state is unavailable.
pub(crate) unsafe fn try_with<R>(operation: impl FnOnce(ServiceState<'_>) -> R) -> Option<R> {
    if BOOT_SERVICES.busy.replace(true) {
        return None;
    }
    let _guard = BusyGuard(&BOOT_SERVICES.busy);
    // SAFETY: the caller executes on the BSP, and the busy flag owns both fields.
    unsafe {
        let allocator = (&mut *BOOT_SERVICES.allocator.get()).as_mut()?;
        Some(operation(ServiceState {
            allocator,
            protocols: &mut *BOOT_SERVICES.protocols.get(),
            events: &mut *BOOT_SERVICES.events.get(),
        }))
    }
}
