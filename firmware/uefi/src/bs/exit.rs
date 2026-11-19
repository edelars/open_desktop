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
            if state.exit.phase != ExitPhase::Boot {
                return Status::INVALID_PARAMETER;
            }
            state.exit.image = image;
            Status::SUCCESS
        })
    }
}

/// # Safety
/// Call on the BSP from the active image. Registered notification handlers
/// may execute before return. A successful call ends Boot Services.
pub unsafe extern "efiapi" fn exit_boot_services(image: Handle, map_key: usize) -> Status {
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
        Some(Err(status)) => return status,
        None => return Status::INVALID_PARAMETER,
    }

    let status = unsafe {
        state::with(|state| {
            let status = state
                .exit
                .validate_map_key(map_key, state.allocator.map().key());
            if status != Status::SUCCESS {
                return status;
            }
            state.events.disable_timers();
            state.events.signal_group(EXIT_BOOT_SERVICES);
            Status::SUCCESS
        })
    };
    if status != Status::SUCCESS {
        return status;
    }
    unsafe {
        event::dispatch_pending();
        state::with(|state| {
            let Some(mut system) = state.system_table else {
                return Status::INVALID_PARAMETER;
            };
            system.as_mut().clear_boot_services();
            state.exit.phase = ExitPhase::Exited;
            Status::SUCCESS
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
