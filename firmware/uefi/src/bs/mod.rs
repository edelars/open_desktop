//! Firmware implementations of UEFI Boot Services.

pub(crate) mod config_table;
pub mod event;
pub mod exit;
pub mod image;
pub mod memory;
pub mod misc;
pub mod protocol;
mod state;

pub use state::{attach_image_paging, attach_system_tables, attach_watchdog, initialize};

#[cfg(not(target_os = "uefi"))]
pub(crate) unsafe fn with_runtime_variables(
    operation: impl FnOnce(&mut crate::rt::variable::RuntimeVariables) -> crate::uefi::status::Status,
) -> crate::uefi::status::Status {
    use crate::uefi::status::Status;
    #[cfg(not(target_os = "none"))]
    {
        let _ = operation;
        Status::UNSUPPORTED
    }
    #[cfg(target_os = "none")]
    unsafe {
        state::try_with(|state| {
            let Some(table) = state.runtime_services else {
                return Status::UNSUPPORTED;
            };
            let pointer = core::ptr::addr_of!((*table.as_ptr()).variables).read();
            let Some(variables) = pointer.as_mut() else {
                return Status::UNSUPPORTED;
            };
            operation(variables)
        })
        .unwrap_or(Status::UNSUPPORTED)
    }
}

/// Publish the serial protocols, device path and System Table console together.
///
/// # Safety
/// The vtables must stay valid through ExitBootServices.
#[cfg(target_os = "none")]
pub(crate) unsafe fn publish_console(
    input: *mut crate::console::text_in::SimpleTextInputProtocol,
    output: *mut crate::console::text_out::SimpleTextOutputProtocol,
) -> crate::uefi::status::Status {
    use crate::console::{
        COM1_DEVICE_PATH, text_in::SIMPLE_TEXT_INPUT_PROTOCOL_GUID,
        text_out::SIMPLE_TEXT_OUTPUT_PROTOCOL_GUID,
    };
    use crate::device_path::DEVICE_PATH_PROTOCOL_GUID;
    use crate::uefi::status::Status;
    use core::ptr;
    use protocol::multiple::ProtocolPair;

    let pairs = [
        ProtocolPair {
            protocol: SIMPLE_TEXT_INPUT_PROTOCOL_GUID,
            interface: input.cast(),
        },
        ProtocolPair {
            protocol: SIMPLE_TEXT_OUTPUT_PROTOCOL_GUID,
            interface: output.cast(),
        },
        ProtocolPair {
            protocol: DEVICE_PATH_PROTOCOL_GUID,
            interface: COM1_DEVICE_PATH.as_ptr().cast_mut().cast(),
        },
    ];
    let status = unsafe {
        state::with(|state| {
            let Some(mut system) = state.system_table else {
                return Status::NOT_READY;
            };
            let handle = match state.protocols.install_bindings(ptr::null_mut(), &pairs) {
                Ok(handle) => handle,
                Err(status) => return status,
            };
            system
                .as_mut()
                .publish_console(handle, input.cast(), output.cast());
            state.protocols.signal_installations(state.events, |guid| {
                guid == SIMPLE_TEXT_INPUT_PROTOCOL_GUID
                    || guid == SIMPLE_TEXT_OUTPUT_PROTOCOL_GUID
                    || guid == DEVICE_PATH_PROTOCOL_GUID
            });
            Status::SUCCESS
        })
    };
    unsafe { protocol::dispatch_after_install(status) }
}
