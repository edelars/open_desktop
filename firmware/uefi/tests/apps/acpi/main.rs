#![no_std]
#![no_main]

use core::{ptr, slice};
use od_firmware::acpi::{
    blob::{ACPI_RECLAIM_END, ACPI_REGION_BASE, FACS_ADDRESS},
    fadt::{FACS_LEN, validate_facs},
    install::EFI_ACPI_20_TABLE_GUID,
    rsdp::{RSDP_LEN, validate_rsdp},
    table::{SDT_HEADER_LEN, validate_table},
};
use od_firmware::smbios::{
    TABLE_CAPACITY, TABLE_OFFSET,
    entry::{ENTRY_LEN, SMBIOS3_TABLE_GUID, validate},
};
use od_firmware::uefi::{
    guid::Guid,
    status::Status,
    system_table::SystemTable,
    types::{Char16, Handle},
};
use od_firmware_test_apps::{ascii_to_char16, output};

#[unsafe(no_mangle)]
unsafe extern "efiapi" fn efi_main(handle: Handle, system: *mut SystemTable) -> Status {
    if handle.is_null() || system.is_null() {
        return Status::INVALID_PARAMETER;
    }
    match unsafe { inspect(system) } {
        Some(()) => Status::SUCCESS,
        None => Status::LOAD_ERROR,
    }
}

// Only inspect memory owned by this board's ACPI reservations.
unsafe fn inspect(system: *mut SystemTable) -> Option<()> {
    let address = unsafe { configuration_address(system, &EFI_ACPI_20_TABLE_GUID) }?;
    if address != ACPI_REGION_BASE {
        return None;
    }
    let rsdp = unsafe { reclaim(address, RSDP_LEN) }?;
    validate_rsdp(rsdp).ok()?;
    unsafe { print_table(system, b"RSDP", rsdp.len(), true) }?;

    let xsdt = unsafe { sdt(le64(rsdp, 24), b"XSDT") }?;
    if xsdt.len() != SDT_HEADER_LEN + 3 * 8 {
        return None;
    }
    unsafe { print_table(system, b"XSDT", xsdt.len(), true) }?;
    let fadt = unsafe { sdt(le64(xsdt, SDT_HEADER_LEN), b"FACP") }?;
    let madt = unsafe { sdt(le64(xsdt, SDT_HEADER_LEN + 8), b"APIC") }?;
    let mcfg = unsafe { sdt(le64(xsdt, SDT_HEADER_LEN + 16), b"MCFG") }?;
    unsafe { print_table(system, b"FACP", fadt.len(), true) }?;
    unsafe { print_table(system, b"APIC", madt.len(), true) }?;
    unsafe { print_table(system, b"MCFG", mcfg.len(), true) }?;
    if fadt.len() < 148 || le64(fadt, 132) != FACS_ADDRESS {
        return None;
    }
    if fadt[112] & 1 == 0 {
        return None;
    }
    // The board advertises cache write-back/invalidation and the application runs at CPL0.
    unsafe { core::arch::asm!("wbinvd", options(nostack, preserves_flags)) };
    let dsdt = unsafe { sdt(le64(fadt, 140), b"DSDT") }?;
    unsafe { print_table(system, b"DSDT", dsdt.len(), true) }?;
    let facs = unsafe {
        slice::from_raw_parts(
            ptr::with_exposed_provenance(FACS_ADDRESS as usize),
            FACS_LEN,
        )
    };
    validate_facs(facs).ok()?;
    unsafe { print_table(system, b"FACS", facs.len(), false) }?;
    unsafe { inspect_smbios(system) }
}

unsafe fn inspect_smbios(system: *mut SystemTable) -> Option<()> {
    let entry_address = unsafe { configuration_address(system, &SMBIOS3_TABLE_GUID) }?;
    if !(0x0060_0000..(1 << 36)).contains(&entry_address) || entry_address & 0xfff != 0 {
        return None;
    }
    let entry =
        unsafe { &*ptr::with_exposed_provenance::<[u8; ENTRY_LEN]>(entry_address as usize) };
    let (address, len) = validate(entry)?;
    if address != entry_address + TABLE_OFFSET as u64 || len > TABLE_CAPACITY {
        return None;
    }
    unsafe { print_table(system, b"_SM3", ENTRY_LEN, true) }?;
    let table =
        unsafe { slice::from_raw_parts(ptr::with_exposed_provenance(address as usize), len) };
    let mut remaining = table;
    const STRUCTURES: &[(u8, usize)] = &[
        (0, 0x1a),
        (1, 0x1b),
        (2, 0x0f),
        (3, 0x16),
        (4, 0x33),
        (16, 0x17),
        (17, 0x64),
        (19, 0x1f),
        (20, 0x23),
        (32, 0x0b),
        (127, 4),
    ];
    for &(kind, expected_len) in STRUCTURES {
        let formatted_len = *remaining.get(1)? as usize;
        if remaining.first() != Some(&kind) || formatted_len != expected_len {
            return None;
        }
        let (formatted, strings) = remaining.split_at_checked(formatted_len)?;
        let terminator = strings.windows(2).position(|pair| pair == [0, 0])?;
        if kind == 1
            && (formatted.get(7) != Some(&3)
                || formatted.get(8..0x18)?
                    != b"\x79\xa3\x31\x49\x27\x43\x80\x43\xa2\x9b\x88\x19\x20\x31\x6b\x40"
                || !strings[..terminator].ends_with(b"\0OD-S7-13"))
        {
            return None;
        }
        if kind == 4
            && (formatted.get(8..16)? != [0x00, 0x06, 0, 0, 0, 0, 0, 0]
                || formatted.get(0x14..0x18)? != [0xe8, 0x03, 0xe8, 0x03]
                || formatted.get(0x23..0x26)? != [1, 1, 1])
        {
            return None;
        }
        if kind == 16 && (formatted.get(7..11)? != [0, 0, 2, 0] || formatted.get(0x0d) != Some(&1))
        {
            return None;
        }
        if kind == 17
            && (formatted.get(4..6)? != [0x00, 0x10]
                || formatted.get(8..14)? != [64, 0, 64, 0, 128, 0]
                || le64(formatted, 0x3c) != 128 << 20)
        {
            return None;
        }
        if kind == 19
            && (formatted.get(4..12)? != [0, 0, 0, 0, 0xff, 0xff, 1, 0]
                || formatted.get(12..15)? != [0, 0x10, 1])
        {
            return None;
        }
        if kind == 20
            && (formatted.get(4..12)? != [0, 0, 0, 0, 0xff, 0xff, 1, 0]
                || formatted.get(12..16)? != [0, 0x11, 0, 0x13])
        {
            return None;
        }
        remaining = &strings[terminator + 2..];
    }
    if !remaining.is_empty() {
        return None;
    }
    unsafe { print_table(system, b"SMBI", len, false) }
}

unsafe fn configuration_address(system: *mut SystemTable, guid: &Guid) -> Option<u64> {
    let (tables, count) = unsafe { (*system).configuration_tables() };
    if tables.is_null() || count > 32 {
        return None;
    }
    let mut address = None;
    for entry in unsafe { slice::from_raw_parts(tables, count) } {
        if entry.vendor_guid == *guid {
            if address.is_some() {
                return None;
            }
            address = Some(entry.vendor_table.addr() as u64);
        }
    }
    address
}

unsafe fn reclaim(address: u64, len: usize) -> Option<&'static [u8]> {
    if address < ACPI_REGION_BASE || address.checked_add(len as u64)? > ACPI_RECLAIM_END {
        return None;
    }
    Some(unsafe { slice::from_raw_parts(ptr::with_exposed_provenance(address as usize), len) })
}

unsafe fn sdt(address: u64, signature: &[u8; 4]) -> Option<&'static [u8]> {
    let header = unsafe { reclaim(address, SDT_HEADER_LEN) }?;
    let len = u32::from_le_bytes(header[4..8].try_into().ok()?) as usize;
    let table = unsafe { reclaim(address, len) }?;
    validate_table(table, signature).ok()?;
    Some(table)
}

fn le64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

unsafe fn print_table(
    system: *mut SystemTable,
    signature: &[u8; 4],
    len: usize,
    checksum: bool,
) -> Option<()> {
    const LINE: &[u8] = b"OD-UEFI acpi .... len=0x0000 checksum=ok\r\n\0";
    const NO_CHECKSUM: &[u8] = b"OD-UEFI acpi .... len=0x0000 checksum=absent\r\n\0";
    let mut text: [Char16; NO_CHECKSUM.len()] = ascii_to_char16(NO_CHECKSUM);
    if checksum {
        text[..LINE.len()].copy_from_slice(&ascii_to_char16::<{ LINE.len() }>(LINE));
    }
    for index in 0..4 {
        text[13 + index] = u16::from(signature[index]);
        let digit = (len >> ((3 - index) * 4)) & 0xf;
        text[24 + index] = u16::from(b"0123456789abcdef"[digit]);
    }
    (unsafe { output(system, text.as_ptr()) } == Status::SUCCESS).then_some(())
}
