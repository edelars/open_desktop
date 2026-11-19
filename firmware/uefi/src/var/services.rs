//! Attribute and global-variable policy from UEFI 2.11 §§3.3 and 8.2.

use super::codec::{Name, Record};
use super::store::VariableStore;
use crate::uefi::{guid::Guid, status::Status};

pub const NON_VOLATILE: u32 = 0x01;
pub const BOOTSERVICE_ACCESS: u32 = 0x02;
pub const RUNTIME_ACCESS: u32 = 0x04;
pub const HARDWARE_ERROR_RECORD: u32 = 0x08;
pub const AUTHENTICATED_WRITE_ACCESS: u32 = 0x10;
pub const TIME_BASED_AUTHENTICATED_WRITE_ACCESS: u32 = 0x20;
pub const APPEND_WRITE: u32 = 0x40;
pub const ENHANCED_AUTHENTICATED_ACCESS: u32 = 0x80;
pub const BOOT_RUNTIME: u32 = BOOTSERVICE_ACCESS | RUNTIME_ACCESS;
pub const NON_VOLATILE_RUNTIME: u32 = NON_VOLATILE | BOOT_RUNTIME;
pub const GLOBAL_VARIABLE: Guid = Guid {
    data1: 0x8be4_df61,
    data2: 0x93ca,
    data3: 0x11d2,
    data4: [0xaa, 0x0d, 0x00, 0xe0, 0x98, 0x03, 0x2b, 0x8c],
};
const UNSUPPORTED: u32 = HARDWARE_ERROR_RECORD
    | AUTHENTICATED_WRITE_ACCESS
    | TIME_BASED_AUTHENTICATED_WRITE_ACCESS
    | ENHANCED_AUTHENTICATED_ACCESS;
const IMAGE_SECURITY_DATABASE: Guid = Guid {
    data1: 0xd719_b2cb,
    data2: 0x3d3a,
    data3: 0x4596,
    data4: [0xa3, 0xbc, 0xda, 0xd0, 0x0e, 0x67, 0x65, 0x6f],
};
const PLATFORM_LANGUAGE: &[u8] = b"en-US\0";

pub(super) fn validate_attributes(attributes: u32) -> Result<(), Status> {
    if attributes & UNSUPPORTED != 0 {
        return Err(Status::UNSUPPORTED);
    }
    if attributes & !(NON_VOLATILE_RUNTIME | APPEND_WRITE) != 0
        || (attributes & RUNTIME_ACCESS != 0 && attributes & BOOTSERVICE_ACCESS == 0)
    {
        return Err(Status::INVALID_PARAMETER);
    }
    Ok(())
}

pub(super) fn persistent_attributes(attributes: u32, guid: Guid, name: Name<'_>) -> bool {
    attributes & (NON_VOLATILE | BOOTSERVICE_ACCESS | APPEND_WRITE)
        == NON_VOLATILE | BOOTSERVICE_ACCESS
        && validate_attributes(attributes).is_ok()
        && !read_only(guid, name)
        && global_attributes(guid, name).is_none_or(|expected| attributes == expected)
}

pub(super) fn read_only(guid: Guid, name: Name<'_>) -> bool {
    guid == IMAGE_SECURITY_DATABASE
        || (guid == GLOBAL_VARIABLE
            && (matches_ascii(name, b"SetupMode")
                || matches_ascii(name, b"SecureBoot")
                || matches_ascii(name, b"PlatformLangCodes")
                || matches_ascii(name, b"OsIndicationsSupported")
                || matches_ascii(name, b"PK")
                || matches_ascii(name, b"KEK")))
}

pub(super) fn valid_write(guid: Guid, name: Name<'_>, value: &[u8], append: bool) -> bool {
    !(guid == GLOBAL_VARIABLE && matches_ascii(name, b"PlatformLang"))
        || (!append && value == PLATFORM_LANGUAGE)
}

#[derive(Clone, Copy)]
pub(crate) enum BootDefaults {
    All,
    Volatile,
    NonVolatile,
}

pub(crate) fn seed_defaults(
    store: &mut VariableStore,
    console_path: &[u8],
    selection: BootDefaults,
) -> u32 {
    let variables: [(&[u8], u32, &[u8]); 9] = [
        (b"PlatformLangCodes", BOOT_RUNTIME, PLATFORM_LANGUAGE),
        (b"PlatformLang", NON_VOLATILE_RUNTIME, PLATFORM_LANGUAGE),
        (b"ConIn", NON_VOLATILE_RUNTIME, console_path),
        (b"ConOut", NON_VOLATILE_RUNTIME, console_path),
        (b"ErrOut", NON_VOLATILE_RUNTIME, console_path),
        (b"Timeout", NON_VOLATILE_RUNTIME, &[0; 2]),
        (b"OsIndicationsSupported", BOOT_RUNTIME, &[0; 8]),
        (b"SecureBoot", BOOT_RUNTIME, &[0]),
        (b"SetupMode", BOOT_RUNTIME, &[1]),
    ];
    let mut missing = 0;
    for (ascii, attributes, value) in variables {
        let selected = match selection {
            BootDefaults::All => true,
            BootDefaults::Volatile => attributes & NON_VOLATILE == 0,
            BootDefaults::NonVolatile => attributes & NON_VOLATILE != 0,
        };
        if selected && seed_ascii(store, ascii, attributes, value).is_err() {
            missing += 1;
        }
    }
    missing
}

fn seed_ascii(
    store: &mut VariableStore,
    ascii: &[u8],
    attributes: u32,
    value: &[u8],
) -> Result<(), Status> {
    let mut bytes = [0; 2 * b"OsIndicationsSupported".len()];
    for (pair, &byte) in bytes.chunks_exact_mut(2).zip(ascii) {
        pair[0] = byte;
    }
    let name = Name::new(&bytes[..2 * ascii.len()]).unwrap();
    if store.get(GLOBAL_VARIABLE, name).is_some() {
        return Ok(());
    }
    store.seed(Record {
        guid: GLOBAL_VARIABLE,
        name,
        attributes,
        value,
    })
}

pub(super) fn global_attributes(guid: Guid, name: Name<'_>) -> Option<u32> {
    if guid != GLOBAL_VARIABLE {
        return None;
    }
    if read_only(guid, name) {
        return Some(BOOT_RUNTIME);
    }
    if matches_ascii(name, b"BootCurrent") {
        return Some(BOOT_RUNTIME);
    }
    if matches_ascii(name, b"PlatformLang")
        || matches_ascii(name, b"ConIn")
        || matches_ascii(name, b"ConOut")
        || matches_ascii(name, b"ErrOut")
        || matches_ascii(name, b"Timeout")
        || matches_ascii(name, b"BootNext")
        || matches_ascii(name, b"BootOrder")
        || boot_option_name(name)
    {
        return Some(NON_VOLATILE_RUNTIME);
    }
    None
}

fn boot_option_name(name: Name<'_>) -> bool {
    if name.as_bytes().len() != 16 {
        return false;
    }
    let mut units = name.units();
    units
        .by_ref()
        .take(4)
        .eq(b"Boot".iter().copied().map(u16::from))
        && units.all(|unit| (0x30..=0x39).contains(&unit) || (0x41..=0x46).contains(&unit))
}

fn matches_ascii(name: Name<'_>, ascii: &[u8]) -> bool {
    name.as_bytes().len() == 2 * ascii.len()
        && name.units().eq(ascii.iter().copied().map(u16::from))
}
