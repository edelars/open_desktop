//! Shared state of the boot processor's EFI services.

use super::config_table::ConfigurationTables;
use super::event::EventRegistry;
use super::exit::BootExit;
use super::image::ImageRegistry;
use super::protocol::ProtocolRegistry;
use crate::mem::pages::PageAllocator;
use crate::paging::{ActivePageTables, BootPaging};
use crate::uefi::status::Status;
use crate::uefi::system_table::{RuntimeServicesStorage, SystemTable};
use core::cell::{Cell, UnsafeCell};
use core::ptr::NonNull;

struct BspServices {
    busy: Cell<bool>,
    allocator: UnsafeCell<Option<PageAllocator<'static, 'static>>>,
    protocols: UnsafeCell<ProtocolRegistry>,
    images: UnsafeCell<ImageRegistry>,
    active_tables: Cell<Option<ActivePageTables>>,
    events: UnsafeCell<EventRegistry>,
    configuration: UnsafeCell<ConfigurationTables>,
    system_table: Cell<Option<NonNull<SystemTable>>>,
    runtime_services: Cell<Option<NonNull<RuntimeServicesStorage>>>,
    #[cfg(target_os = "none")]
    image_policy_mmio: Cell<Option<u64>>,
    exit: UnsafeCell<BootExit>,
}

// SAFETY: only the BSP accesses these fields. Unsafe entrances require the
// BSP and the guard prevents nested mutable borrows. The guarded operations
// never dispatch notifications or yield control to an EFI image.
unsafe impl Sync for BspServices {}

static BOOT_SERVICES: BspServices = BspServices {
    busy: Cell::new(false),
    allocator: UnsafeCell::new(None),
    protocols: UnsafeCell::new(ProtocolRegistry::new()),
    images: UnsafeCell::new(ImageRegistry::new()),
    active_tables: Cell::new(None),
    events: UnsafeCell::new(EventRegistry::new()),
    configuration: UnsafeCell::new(ConfigurationTables::new()),
    system_table: Cell::new(None),
    runtime_services: Cell::new(None),
    #[cfg(target_os = "none")]
    image_policy_mmio: Cell::new(None),
    exit: UnsafeCell::new(BootExit::new()),
};

pub(crate) struct ServiceState<'a> {
    pub allocator: &'a mut PageAllocator<'static, 'static>,
    pub protocols: &'a mut ProtocolRegistry,
    pub images: &'a mut ImageRegistry,
    pub active_tables: Option<ActivePageTables>,
    pub events: &'a mut EventRegistry,
    pub configuration: &'a mut ConfigurationTables,
    pub system_table: Option<NonNull<SystemTable>>,
    pub runtime_services: Option<NonNull<RuntimeServicesStorage>>,
    pub exit: &'a mut BootExit,
}

struct BusyGuard<'a>(&'a Cell<bool>);

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// Takes ownership of the live map and binds the event timers to the BSP clock.
///
/// # Safety
/// Invoke once on the BSP, before calling or publishing services. The frequency
/// is from the validated handoff, and the clock reader must not call services.
pub unsafe fn initialize(
    allocator: PageAllocator<'static, 'static>,
    tsc_hz: u64,
    read_tsc: fn() -> u64,
) {
    // SAFETY: initialization precedes the first access and has a single caller.
    unsafe {
        (&mut *BOOT_SERVICES.events.get()).set_clock(tsc_hz, read_tsc);
        BOOT_SERVICES.allocator.get().write(Some(allocator));
    }
}

/// Host tests: bind (or unbind) the active tables the memory services publish permissions in.
#[cfg(test)]
pub(crate) fn set_active_tables_for_test(tables: Option<ActivePageTables>) {
    BOOT_SERVICES.active_tables.set(tables);
}

/// # Safety
/// The BSP must already be executing on `paging`'s private identity tables.
/// Their RAM reservation must remain valid until `ExitBootServices`.
pub unsafe fn attach_image_paging(paging: &BootPaging<'_, '_>) {
    BOOT_SERVICES.active_tables.set(Some(ActivePageTables::new(
        paging.root(),
        paging.table_words() as u64 * 8,
    )));
}

/// # Safety
/// Call once on the BSP before publishing the tables. Both must remain at
/// their addresses and be exclusively writable by Boot Services when updated.
/// The runtime storage lives in runtime memory and survives boot exit.
pub unsafe fn attach_system_tables(system: *mut SystemTable, runtime: *mut RuntimeServicesStorage) {
    BOOT_SERVICES.system_table.set(NonNull::new(system));
    BOOT_SERVICES.runtime_services.set(NonNull::new(runtime));
    if !runtime.is_null() {
        unsafe { core::ptr::addr_of_mut!((*runtime).system).write(system) };
    }
}

/// # Safety
/// Call once on the BSP before publishing Boot Services. The validated board handoff
/// guarantees that a present aperture supports complete-file authorization.
#[cfg(target_os = "none")]
pub unsafe fn attach_image_policy(mmio: Option<u64>) {
    BOOT_SERVICES.image_policy_mmio.set(mmio);
}

#[cfg(target_os = "none")]
pub(crate) fn image_policy_mmio() -> Option<u64> {
    BOOT_SERVICES.image_policy_mmio.get()
}

/// # Safety
/// Call once on the BSP before any service calls. The register and reset
/// value come from validated fixed hardware. The action terminates the run.
pub unsafe fn attach_watchdog(port: u16, value: u8, reset: fn(u16, u8, u64) -> !) {
    // SAFETY: the BSP has not exposed the services or started notifications.
    unsafe { (&mut *BOOT_SERVICES.events.get()).set_watchdog_target(port, value, reset) };
}

/// The caller must be the BSP. The closure must return before invoking a
/// notification or yielding control to an EFI image.
pub(crate) unsafe fn with(operation: impl FnOnce(ServiceState<'_>) -> Status) -> Status {
    unsafe { try_with(operation) }.unwrap_or(Status::NOT_READY)
}

/// The BSP owns state while making a memory-map change. EXIT notifications
/// can run services that preserve the map, while mutations are frozen.
pub(crate) unsafe fn with_map_mutation(
    operation: impl FnOnce(ServiceState<'_>) -> Status,
) -> Status {
    unsafe {
        with(|state| {
            if state.exit.can_change_map() {
                operation(state)
            } else {
                Status::ACCESS_DENIED
            }
        })
    }
}

/// Like with(), but returns None when the state is unavailable.
pub(crate) unsafe fn try_with<R>(operation: impl FnOnce(ServiceState<'_>) -> R) -> Option<R> {
    if BOOT_SERVICES.busy.replace(true) {
        return None;
    }
    let _guard = BusyGuard(&BOOT_SERVICES.busy);
    // SAFETY: the caller executes on the BSP, and the busy flag owns the state.
    unsafe {
        let exit = &mut *BOOT_SERVICES.exit.get();
        if exit.exited() {
            return None;
        }
        let allocator = (&mut *BOOT_SERVICES.allocator.get()).as_mut()?;
        Some(operation(ServiceState {
            allocator,
            protocols: &mut *BOOT_SERVICES.protocols.get(),
            images: &mut *BOOT_SERVICES.images.get(),
            active_tables: BOOT_SERVICES.active_tables.get(),
            events: &mut *BOOT_SERVICES.events.get(),
            configuration: &mut *BOOT_SERVICES.configuration.get(),
            system_table: BOOT_SERVICES.system_table.get(),
            runtime_services: BOOT_SERVICES.runtime_services.get(),
            exit,
        }))
    }
}
