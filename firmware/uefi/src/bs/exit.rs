//! UEFI 2.11 §7.4.6 boot-services exit on the BSP.
use super::{event, state};
use crate::uefi::{guid::Guid, status::Status, types::Handle};
use core::ptr;

pub const BEFORE_EXIT_BOOT_SERVICES: Guid = Guid {
    data1: 0x8be0_e274,
    data2: 0x3970,
    data3: 0x4b44,
    data4: [0x80, 0xc5, 0x1a, 0xb9, 0x50, 0x2f, 0x3b, 0xfc],
};
pub const EXIT_BOOT_SERVICES: Guid = Guid {
    data1: 0x27ab_f055,
    data2: 0xb1b8,
    data3: 0x4c26,
    data4: [0x80, 0x48, 0x74, 0x8f, 0x37, 0xba, 0xa2, 0xdf],
};

#[derive(Clone, Copy, Eq, PartialEq)]
enum ExitPhase {
    Boot,
    BeforeExit,
    ExitNotifying,
    Exited,
}

pub(crate) struct BootExit {
    phase: ExitPhase,
    image: Handle,
    before_notified: bool,
}

impl BootExit {
    pub(crate) const fn new() -> Self {
        Self {
            phase: ExitPhase::Boot,
            image: ptr::null_mut(),
            before_notified: false,
        }
    }

    pub(super) fn exited(&self) -> bool {
        self.phase == ExitPhase::Exited
    }

    pub(super) fn can_change_map(&self) -> bool {
        matches!(self.phase, ExitPhase::Boot | ExitPhase::BeforeExit)
    }

    pub(super) fn can_start_image(&self) -> bool {
        self.phase == ExitPhase::Boot
    }

    pub(super) fn replace_image(&mut self, image: Handle) -> Handle {
        core::mem::replace(&mut self.image, image)
    }

    fn begin(&mut self, image: Handle, tpl: event::Tpl) -> Result<bool, Status> {
        if self.phase != ExitPhase::Boot
            || tpl != event::TPL_APPLICATION
            || image.is_null()
            || image != self.image
        {
            return Err(Status::INVALID_PARAMETER);
        }
        self.phase = ExitPhase::BeforeExit;
        let should_notify = !self.before_notified;
        self.before_notified = true;
        Ok(should_notify)
    }

    fn validate_map_key(&mut self, supplied: usize, current: u64) -> Status {
        if supplied as u64 != current {
            self.phase = ExitPhase::Boot;
            return Status::INVALID_PARAMETER;
        }
        self.phase = ExitPhase::ExitNotifying;
        Status::SUCCESS
    }
}

/// Selects the active image whose handle can request the exit.
///
/// # Safety
/// Call on the BSP with the current EFI image handle from the entry boundary.
/// The private boot diagnostic binds its own temporary handle through this
/// boundary until the image loader supplies a guest image.
pub unsafe fn set_boot_image_handle(image: Handle) -> Status {
    if image.is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe {
        state::with(|state| {
            if !state.exit.can_start_image() {
                return Status::INVALID_PARAMETER;
            }
            state.exit.replace_image(image);
            Status::SUCCESS
        })
    }
}

/// Makes the pages of the runtime images writable, keeping their executability (the code pages
/// stay executable, the data pages stay NX). The fixup tables and the rest of the allocation keep
/// their read-only mapping. Images whose pages are not all present in the tables are skipped.
///
/// # Safety
/// The BSP owns `tables`; the images describe loaded runtime images.
#[cfg(any(target_os = "none", test))]
pub(crate) unsafe fn unlock_runtime_images(
    tables: &crate::paging::ActivePageTables,
    images: &[crate::rt::virt::RuntimeImage],
) {
    use crate::paging::PAGE_BYTES;
    for image in images.iter().filter(|image| image.base != 0) {
        let start = image.base & !(PAGE_BYTES - 1);
        let end = (image.base + image.size).next_multiple_of(PAGE_BYTES);
        if unsafe { tables.verify(start..end, false) } {
            unsafe { tables.make_writable(start..end) };
        }
    }
    unsafe { tables.flush() };
}

/// # Safety
/// Call on the BSP from the active image. Registered notification handlers
/// may execute before return. A successful call ends Boot Services.
pub unsafe extern "efiapi" fn exit_boot_services(image: Handle, map_key: usize) -> Status {
    use crate::tpm::measure::action;
    if crate::tpm::active() && !unsafe { super::boot_services_active() } {
        return Status::INVALID_PARAMETER;
    }
    // A full log or unavailable TPM does not change the ExitBootServices ABI.
    let _ = action(5, b"Exit Boot Services Invocation");
    let notify_before = unsafe {
        state::try_with(|state| {
            if state.system_table.is_none() {
                return Err(Status::INVALID_PARAMETER);
            }
            let first_exit = state.exit.begin(image, state.events.current_tpl())?;
            if first_exit {
                state.events.signal_group(BEFORE_EXIT_BOOT_SERVICES);
            }
            Ok(first_exit)
        })
    };
    match notify_before {
        Some(Ok(true)) => unsafe { event::dispatch_pending() },
        Some(Ok(false)) => {}
        Some(Err(status)) => {
            let _ = action(5, b"Exit Boot Services Returned with Failure");
            return status;
        }
        None => {
            let _ = action(5, b"Exit Boot Services Returned with Failure");
            return Status::INVALID_PARAMETER;
        }
    }

    let status = unsafe {
        state::with(|state| {
            let status = state
                .exit
                .validate_map_key(map_key, state.allocator.map().key());
            if status != Status::SUCCESS {
                return status;
            }
            if let Some(runtime) = state.runtime_services {
                core::ptr::addr_of_mut!((*runtime.as_ptr()).event_table)
                    .write(state.events.runtime_event_table());
            }
            state.events.disable_timers();
            state.events.signal_group(EXIT_BOOT_SERVICES);
            Status::SUCCESS
        })
    };
    if status != Status::SUCCESS {
        let _ = action(5, b"Exit Boot Services Returned with Failure");
        return status;
    }
    unsafe {
        event::dispatch_pending();
        state::with(|state| {
            let Some(mut system) = state.system_table else {
                return Status::INVALID_PARAMETER;
            };
            // The final callbacks may have updated the user policy without changing the map.
            #[cfg(target_os = "none")]
            if let Err(status) = crate::tpm::secure_boot::refresh() {
                let _ = action(5, b"Exit Boot Services Returned with Failure");
                return status;
            }
            // Once exit callbacks have run, complete the irreversible transition.
            let _ = action(5, b"Exit Boot Services Returned with Success");
            system.as_mut().clear_boot_services();
            if let Some(runtime) = state.runtime_services
                && let Some(variables) = core::ptr::addr_of!((*runtime.as_ptr()).variables)
                    .read()
                    .as_mut()
            {
                variables.enter_runtime();
            }
            if let Some(runtime) = state.runtime_services {
                let runtime = runtime.as_ptr();
                // The OS maps the runtime images writable while SetVirtualAddressMap runs, but a
                // loader that calls it on the firmware's own tables needs them writable too:
                // EDK2 lifts the read-only protection of runtime images at ExitBootServices.
                #[cfg(any(target_os = "none", test))]
                if let Some(tables) = state.active_tables {
                    unlock_runtime_images(&tables, &(*core::ptr::addr_of!((*runtime).images)).0);
                }
                (&mut *core::ptr::addr_of_mut!((*runtime).final_runtime_map))
                    .capture(state.allocator.map().descriptors());
                core::ptr::addr_of_mut!((*runtime).at_runtime).write(true);
            }
            state.exit.phase = ExitPhase::Exited;
            Status::SUCCESS
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S7-31l: at ExitBootServices the runtime images become writable on the firmware's own tables
    /// (as EDK2 does), so SetVirtualAddressMap can apply its fixups through them; executability
    /// is kept, pages outside the images and images with a hole are untouched.
    #[test]
    fn exit_makes_runtime_image_pages_writable_and_keeps_executability() {
        use crate::paging::{ActivePageTables, PAGE_BYTES};
        use crate::rt::virt::RuntimeImage;
        const ENTRIES: usize = 512;
        const PRESENT: u64 = 1;
        const WRITABLE: u64 = 1 << 1;
        const NO_EXECUTE: u64 = 1 << 63;
        #[repr(align(4096))]
        struct Arena([u64; ENTRIES * 4]);
        let mut arena = Arena([0; ENTRIES * 4]);
        let root = arena.0.as_mut_ptr().expose_provenance() as u64;
        let tables = ActivePageTables::new(root, 4 * PAGE_BYTES);
        for table in 0..3 {
            arena.0[table * ENTRIES] = root + (table as u64 + 1) * PAGE_BYTES | PRESENT | WRITABLE;
        }
        let leaf_of = |page: u64| ENTRIES * 3 + ((page >> 12) as usize & (ENTRIES - 1));
        let flags = |arena: &Arena, page: u64| arena.0[leaf_of(page)] & !0x000f_ffff_ffff_f000;
        // Image A: two read-only code pages, one read-only NX data page, then a read-only NX page
        // outside the image (the fixup tables).
        let a = 0x100000u64;
        let code = PRESENT;
        let data = PRESENT | NO_EXECUTE;
        for (index, bits) in [code, code, data, data].into_iter().enumerate() {
            let page = a + index as u64 * PAGE_BYTES;
            arena.0[leaf_of(page)] = page | bits;
        }
        // Image B: page 0x108000 is present, page 0x109000 is not mapped.
        arena.0[leaf_of(0x108000)] = 0x108000 | code;
        let images = [
            RuntimeImage {
                base: a,
                size: 0x2800,
                ..RuntimeImage::EMPTY
            },
            RuntimeImage::EMPTY,
            RuntimeImage {
                base: 0x108000,
                size: 0x2000,
                ..RuntimeImage::EMPTY
            },
        ];
        unsafe { unlock_runtime_images(&tables, &images) };
        // Code: writable and still executable; data: writable and still NX.
        assert_eq!(flags(&arena, a), PRESENT | WRITABLE);
        assert_eq!(flags(&arena, a + PAGE_BYTES), PRESENT | WRITABLE);
        assert_eq!(
            flags(&arena, a + 2 * PAGE_BYTES),
            PRESENT | WRITABLE | NO_EXECUTE
        );
        // Past the image: unchanged.
        assert_eq!(flags(&arena, a + 3 * PAGE_BYTES), data);
        // The image with a hole is skipped as a whole.
        assert_eq!(flags(&arena, 0x108000), code);
    }

    #[test]
    fn before_exit_runs_once_and_map_mismatch_leaves_the_boot_mutation_path_open() {
        let mut exit = BootExit::new();
        let active = ptr::without_provenance_mut(9);
        let other = ptr::without_provenance_mut(10);
        exit.image = active;
        assert_eq!(
            exit.begin(other, event::TPL_APPLICATION),
            Err(Status::INVALID_PARAMETER)
        );
        assert_eq!(
            exit.begin(active, event::TPL_CALLBACK),
            Err(Status::INVALID_PARAMETER)
        );
        assert_eq!(exit.begin(active, event::TPL_APPLICATION), Ok(true));
        assert!(exit.can_change_map());
        assert_eq!(
            exit.begin(active, event::TPL_APPLICATION),
            Err(Status::INVALID_PARAMETER)
        );
        assert_eq!(exit.validate_map_key(17, 18), Status::INVALID_PARAMETER);
        assert!(exit.can_change_map());
        assert_eq!(exit.begin(active, event::TPL_APPLICATION), Ok(false));
        assert_eq!(exit.validate_map_key(18, 18), Status::SUCCESS);
        assert!(!exit.can_change_map());
        assert_eq!(
            exit.begin(active, event::TPL_APPLICATION),
            Err(Status::INVALID_PARAMETER)
        );
    }
}
