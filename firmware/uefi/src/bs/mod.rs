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

/// Publish both serial protocols and the System Table console in one BSP edit.
///
/// # Safety
/// The vtables must stay valid through ExitBootServices.
#[cfg(target_os = "none")]
pub(crate) unsafe fn publish_console(
    input: *mut crate::console::text_in::SimpleTextInputProtocol,
    output: *mut crate::console::text_out::SimpleTextOutputProtocol,
) -> crate::uefi::status::Status {
    use crate::console::{
        text_in::SIMPLE_TEXT_INPUT_PROTOCOL_GUID, text_out::SIMPLE_TEXT_OUTPUT_PROTOCOL_GUID,
    };
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
                guid == SIMPLE_TEXT_INPUT_PROTOCOL_GUID || guid == SIMPLE_TEXT_OUTPUT_PROTOCOL_GUID
            });
            Status::SUCCESS
        })
    };
    unsafe { protocol::dispatch_after_install(status) }
}
