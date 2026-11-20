//! Guest verification of the published image execution table.

use super::{IMAGE_SECURITY_DATABASE, check};
use core::{ptr, slice};
use od_firmware::{
    device_path::{
        DevicePathNode,
        utilities::{DEVICE_PATH_UTILITIES_PROTOCOL_GUID, DevicePathUtilitiesProtocol},
    },
    uefi::{
        loaded_image::LOADED_IMAGE_DEVICE_PATH_PROTOCOL_GUID,
        status::Status,
        system_table::{BootServices, SystemTable},
        types::Handle,
    },
};

const END: [u8; 4] = [0x7f, 0xff, 4, 0];
const BOOT_NAME: &[u8] = b"B\0O\0O\0T\0X\x006\x004\0.\0E\0F\0I\0\0\0";
const SHA256_GUID: [u8; 16] = [
    0x26, 0x16, 0xc4, 0xc1, 0x4c, 0x50, 0x92, 0x40, 0xac, 0xa9, 0x41, 0xf9, 0x36, 0x93, 0x43, 0x28,
];

pub unsafe fn table(system: *mut SystemTable) -> Result<*const u8, Status> {
    let system = unsafe { system.as_ref() }.ok_or(Status::LOAD_ERROR)?;
    let (entries, count) = system.configuration_tables();
    if entries.is_null() || !(1..=32).contains(&count) {
        return Err(Status::LOAD_ERROR);
    }
    let entries = unsafe { slice::from_raw_parts(entries, count) };
    entries
        .iter()
        .find(|entry| entry.vendor_guid == IMAGE_SECURITY_DATABASE && !entry.vendor_table.is_null())
        .map(|entry| entry.vendor_table.cast::<u8>().cast_const())
        .ok_or(Status::LOAD_ERROR)
}

pub unsafe fn boot_path(boot: &BootServices, image: Handle) -> Result<&[u8], Status> {
    let mut interface = ptr::null_mut();
    check(unsafe {
        (boot.handle_protocol)(
            image,
            &LOADED_IMAGE_DEVICE_PATH_PROTOCOL_GUID,
            &mut interface,
        )
    })?;
    let path = interface.cast::<DevicePathNode>();
    check(unsafe {
        (boot.locate_protocol)(
            &DEVICE_PATH_UTILITIES_PROTOCOL_GUID,
            ptr::null_mut(),
            &mut interface,
        )
    })?;
    let utilities = unsafe { interface.cast::<DevicePathUtilitiesProtocol>().as_ref() }
        .ok_or(Status::LOAD_ERROR)?;
    let size = unsafe { (utilities.get_device_path_size)(path) };
    if !(4..=4096).contains(&size) {
        return Err(Status::LOAD_ERROR);
    }
    Ok(unsafe { slice::from_raw_parts(path.cast::<u8>(), size) })
}

pub unsafe fn count(
    system: *mut SystemTable,
    original: *const u8,
    expected: usize,
) -> Result<(), Status> {
    if unsafe { table(system) }? != original
        || unsafe { original.cast::<usize>().read() } != expected
    {
        return Err(Status::LOAD_ERROR);
    }
    Ok(())
}

#[inline(never)]
fn matches_bytes(actual: &[u8], expected: &[u8]) -> bool {
    actual.len() == expected.len()
        && actual.iter().zip(expected).all(|(actual, expected)| {
            // Read the firmware-owned table byte by byte.
            let actual = unsafe { ptr::read_volatile(actual) };
            actual == *expected
        })
}

pub unsafe fn verify_records(
    table: *const u8,
    boot_path: &[u8],
    revoked_digest: &[u8],
) -> Result<(), Status> {
    let first_size = 8 + BOOT_NAME.len() + boot_path.len();
    let first = unsafe { slice::from_raw_parts(table.add(size_of::<usize>()), first_size) };
    if first[..4] != [0; 4]
        || first[4..8] != (first_size as u32).to_le_bytes()
        || &first[8..8 + BOOT_NAME.len()] != BOOT_NAME
        || &first[8 + BOOT_NAME.len()..] != boot_path
    {
        return Err(Status::LOAD_ERROR);
    }

    let second = unsafe { slice::from_raw_parts(first.as_ptr().add(first_size), 90) };
    if !matches_bytes(&second[..10], &[4, 0, 0, 0, 90, 0, 0, 0, 0, 0])
        || !matches_bytes(&second[10..14], &END)
        || !matches_bytes(&second[14..30], &SHA256_GUID)
        || !matches_bytes(&second[30..42], &[76, 0, 0, 0, 0, 0, 0, 0, 48, 0, 0, 0])
        || !matches_bytes(&second[42..58], &[0; 16])
        || !matches_bytes(&second[58..], revoked_digest)
    {
        return Err(Status::LOAD_ERROR);
    }
    Ok(())
}
