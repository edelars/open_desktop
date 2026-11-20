#![no_std]
#![no_main]

use core::{ptr, slice};
use od_firmware::tpm::{FINAL_EVENTS_GUID, TCG2_PROTOCOL_GUID};
use od_firmware::uefi::{
    status::Status,
    system_table::SystemTable,
    types::{Char16, Handle},
};
use od_firmware_test_apps::{ascii_to_char16, output};

const TEXT: &[u8] = b"OD-UEFI TPM unavailable: no TCG2 protocol or final events\r\n\0";
static MESSAGE: [Char16; TEXT.len()] = ascii_to_char16(TEXT);
static MESSAGE_ADDRESS: &[Char16; TEXT.len()] = &MESSAGE;

#[unsafe(no_mangle)]
unsafe extern "efiapi" fn efi_main(handle: Handle, system: *mut SystemTable) -> Status {
    let Some(table) = (unsafe { system.as_ref() }) else {
        return Status::INVALID_PARAMETER;
    };
    if handle.is_null() || table.boot_services.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let mut interface = ptr::null_mut();
    let status = unsafe {
        ((*table.boot_services).locate_protocol)(
            &TCG2_PROTOCOL_GUID,
            ptr::null_mut(),
            &mut interface,
        )
    };
    if status != Status::NOT_FOUND {
        return Status::LOAD_ERROR;
    }
    let (entries, count) = table.configuration_tables();
    if entries.is_null() || !(1..=32).contains(&count) {
        return Status::LOAD_ERROR;
    }
    if unsafe { slice::from_raw_parts(entries, count) }
        .iter()
        .any(|entry| entry.vendor_guid == FINAL_EVENTS_GUID)
    {
        return Status::LOAD_ERROR;
    }
    let message = unsafe { ptr::read_volatile(&MESSAGE_ADDRESS) };
    unsafe { output(system, message.as_ptr()) }
}
