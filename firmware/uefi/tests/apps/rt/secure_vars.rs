//! Runtime probe for the explicitly disabled Secure Boot profile with protected NVRAM.

use core::{mem::MaybeUninit, ptr, slice};

use od_firmware::{
    secure_boot::siglist::SignatureType,
    uefi::{
        guid::Guid,
        status::Status,
        system_table::{BootServices, RuntimeServices},
        types::Char16,
    },
    var::services::{APPEND_WRITE, GLOBAL_VARIABLE, NON_VOLATILE_RUNTIME},
};
use od_firmware_test_apps::ascii_to_char16;

use super::{NV, VENDOR};

const AUTH: u32 = 0x27;
const DESCRIPTOR_BYTES: usize = 40;
const LIST_HEADER_BYTES: usize = 28;
const ENTRY_BYTES: usize = 48;
const ENTRIES: usize = 400;
const DBX_BYTES: usize = LIST_HEADER_BYTES + ENTRY_BYTES * ENTRIES;
const DATABASE: Guid = Guid {
    data1: 0xd719_b2cb,
    data2: 0x3d3a,
    data3: 0x4596,
    data4: [0xa3, 0xbc, 0xda, 0xd0, 0x0e, 0x67, 0x65, 0x6f],
};
static DBX: [Char16; 4] = ascii_to_char16(b"dbx\0");
static PK: [Char16; 3] = ascii_to_char16(b"PK\0");
static SETUP_MODE: [Char16; 10] = ascii_to_char16(b"SetupMode\0");
static SECURE_BOOT: [Char16; 11] = ascii_to_char16(b"SecureBoot\0");
const PUBLIC_PK: &[u8] =
    include_bytes!("../../../../../tests/fixtures/firmware/certs/windows-uefi-ca-2023.der");

pub(super) unsafe fn available(runtime: *mut RuntimeServices) -> Result<bool, Status> {
    let mut storage = 0;
    let mut remaining = 0;
    let mut max = 0;
    let status =
        unsafe { ((*runtime).query_variable_info)(AUTH, &mut storage, &mut remaining, &mut max) };
    match status {
        Status::SUCCESS if storage >= remaining && max == 65536 => Ok(true),
        Status::UNSUPPORTED => Ok(false),
        _ => Err(Status::DEVICE_ERROR),
    }
}

#[inline(never)]
pub(super) unsafe fn before_exit(boot: *mut BootServices, runtime: *mut RuntimeServices) -> bool {
    if !unsafe { expect_modes(runtime, 1) && query_append(runtime) } {
        return false;
    }
    const BUFFER_BYTES: usize = DESCRIPTOR_BYTES + DBX_BYTES;
    let mut storage = MaybeUninit::<[u8; BUFFER_BYTES]>::uninit();
    let buffer = unsafe {
        ((*boot).set_mem)(storage.as_mut_ptr().cast(), BUFFER_BYTES, 0);
        storage.assume_init_mut()
    };
    descriptor(buffer, 10);
    let entries = list(
        &mut buffer[DESCRIPTOR_BYTES..],
        SignatureType::Sha256,
        ENTRY_BYTES,
        ENTRIES,
    );
    for (index, entry) in entries.chunks_exact_mut(ENTRY_BYTES).enumerate() {
        entry[16..20].copy_from_slice(&(index as u32).to_le_bytes());
    }
    if unsafe { write(runtime, &DBX, &DATABASE, AUTH, buffer) } != Status::SUCCESS {
        return false;
    }

    let mut size = DBX_BYTES;
    let mut attributes = 0;
    if unsafe {
        ((*runtime).get_variable)(
            DBX.as_ptr(),
            &DATABASE,
            &mut attributes,
            &mut size,
            buffer.as_mut_ptr().cast(),
        )
    } != Status::SUCCESS
        || size != DBX_BYTES
        || attributes != AUTH
    {
        return false;
    }
    if !buffer[LIST_HEADER_BYTES..size]
        .chunks_exact(ENTRY_BYTES)
        .enumerate()
        .all(|(index, entry)| entry[16..20] == (index as u32).to_le_bytes())
    {
        return false;
    }
    let mut name = [0; 128];
    name[..NV.len()].copy_from_slice(&NV);
    let mut name_bytes = size_of_val(&name);
    let mut guid = VENDOR;
    // ODRtTest was the last ordinary NV variable created by the loader above this probe.
    (unsafe {
        ((*runtime).get_next_variable_name)(&mut name_bytes, name.as_mut_ptr(), &mut guid)
            == Status::SUCCESS
    }) && guid == DATABASE
        && name_bytes == size_of_val(&DBX)
        && name[..DBX.len()] == DBX
}

pub(super) unsafe fn after_exit(runtime: *mut RuntimeServices) -> bool {
    let mut buffer = [0; DESCRIPTOR_BYTES + LIST_HEADER_BYTES + ENTRY_BYTES];
    descriptor(&mut buffer, 9);
    let entry = list(
        &mut buffer[DESCRIPTOR_BYTES..],
        SignatureType::Sha256,
        ENTRY_BYTES,
        1,
    );
    entry[16..20].copy_from_slice(&999u32.to_le_bytes());
    (unsafe { write(runtime, &DBX, &DATABASE, AUTH | APPEND_WRITE, &buffer) }) == Status::SUCCESS
        && unsafe { expect_dbx_size(runtime, DBX_BYTES + LIST_HEADER_BYTES + ENTRY_BYTES) }
}

pub(super) unsafe fn after_virtual_map(runtime: *mut RuntimeServices) -> bool {
    let mut buffer = [0; DESCRIPTOR_BYTES + LIST_HEADER_BYTES + 16 + PUBLIC_PK.len()];
    descriptor(&mut buffer, 11);
    let entry = list(
        &mut buffer[DESCRIPTOR_BYTES..],
        SignatureType::X509,
        16 + PUBLIC_PK.len(),
        1,
    );
    entry[16..].copy_from_slice(PUBLIC_PK);
    if unsafe { write(runtime, &PK, &GLOBAL_VARIABLE, AUTH, &buffer) } != Status::SUCCESS
        || !unsafe { expect_modes(runtime, 0) }
    {
        return false;
    }
    descriptor(&mut buffer, 12);
    unsafe {
        write(runtime, &DBX, &DATABASE, AUTH, &buffer[..DESCRIPTOR_BYTES])
            == Status::SECURITY_VIOLATION
            && expect_dbx_size(runtime, DBX_BYTES + LIST_HEADER_BYTES + ENTRY_BYTES)
    }
}

/// Changes an ordinary persisted value between EFI calls through the documented MMIO protocol.
#[inline(never)]
pub(super) unsafe fn verify_authoritative_monotonic(runtime: *mut RuntimeServices) -> bool {
    const BASE: usize = 0xff00_0000;
    const REGISTERS: usize = BASE + 0x4_0000;
    const INTERNAL_GUID: [u8; 16] = [
        0xde, 0xfe, 0x84, 0xcf, 0x01, 0xa5, 0xc2, 0x46, 0x8a, 0x8d, 0x12, 0xba, 0x32, 0x24, 0x19,
        0x2f,
    ];
    const INTERNAL_NAME: &[u8] = b"O\0D\0M\0o\0n\0o\0t\0o\0n\0i\0c\0H\0i\0g\0h\0";
    let registers = ptr::with_exposed_provenance_mut::<u32>(REGISTERS);
    let window = ptr::with_exposed_provenance_mut::<u8>(BASE);
    const MAX_IMAGE_BYTES: usize = 24 * 1024;
    let mut storage = MaybeUninit::<[u8; MAX_IMAGE_BYTES]>::uninit();
    let bytes = storage.as_mut_ptr().cast::<u8>();
    unsafe {
        let count = registers.add(4).read_volatile() as usize;
        if !(24..=MAX_IMAGE_BYTES).contains(&count) {
            return false;
        }
        registers.write_volatile(0);
        registers.add(1).write_volatile(count as u32);
        registers.add(2).write_volatile(1);
        if registers.add(3).read_volatile() != 1 {
            return false;
        }
        for index in 0..count {
            bytes.add(index).write(window.add(index).read_volatile());
        }
        let image = slice::from_raw_parts_mut(bytes, count);
        let mut rest = &mut image[24..];
        let mut found = false;
        while let Some(header) = rest.first_chunk::<40>() {
            let length = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
            if length < 40 || length > rest.len() {
                return false;
            }
            let (record, remaining) = rest.split_at_mut(length);
            if record[20..36] == INTERNAL_GUID
                && record.len() == 40 + INTERNAL_NAME.len() + 4
                && record[40..40 + INTERNAL_NAME.len()] == *INTERNAL_NAME
            {
                record[40 + INTERNAL_NAME.len()..].copy_from_slice(&41u32.to_le_bytes());
                record[36..40].fill(0);
                let crc = ieee_crc(record);
                record[36..40].copy_from_slice(&crc.to_le_bytes());
                found = true;
                break;
            }
            rest = remaining;
        }
        if !found {
            return false;
        }
        registers.write_volatile(0);
        registers.add(1).write_volatile(count as u32);
        registers.add(2).write_volatile(2);
        if registers.add(3).read_volatile() != 2 {
            return false;
        }
        for (index, &byte) in image.iter().enumerate() {
            window.add(index).write_volatile(byte);
        }
        registers.add(2).write_volatile(3);
        if registers.add(3).read_volatile() != 3 {
            return false;
        }
        let mut high = 0;
        ((*runtime).get_next_high_monotonic_count)(&mut high) == Status::SUCCESS && high == 42
    }
}

fn ieee_crc(bytes: &[u8]) -> u32 {
    // Reflected IEEE polynomial, folded four bits at a time.
    const NIBBLES: [u32; 16] = [
        0,
        0x1db7_1064,
        0x3b6e_20c8,
        0x26d9_30ac,
        0x76dc_4190,
        0x6b6b_51f4,
        0x4db2_6158,
        0x5005_713c,
        0xedb8_8320,
        0xf00f_9344,
        0xd6d6_a3e8,
        0xcb61_b38c,
        0x9b64_c2b0,
        0x86d3_d2d4,
        0xa00a_e278,
        0xbdbd_f21c,
    ];
    let mut value = u32::MAX;
    for &byte in bytes {
        value ^= u32::from(byte);
        for _ in 0..2 {
            value = (value >> 4) ^ NIBBLES[(value & 0xf) as usize];
        }
    }
    !value
}

fn descriptor(buffer: &mut [u8], second: u8) {
    buffer[..DESCRIPTOR_BYTES].copy_from_slice(&[
        0xea, 7, 11, 19, 23, 10, second, 0, 0, 0, 0, 0, 0, 0, 0, 0, 24, 0, 0, 0, 0, 2, 0xf1, 0x0e,
        0x9d, 0xd2, 0xaf, 0x4a, 0xdf, 0x68, 0xee, 0x49, 0x8a, 0xa9, 0x34, 0x7d, 0x37, 0x56, 0x65,
        0xa7,
    ]);
}

fn list(buffer: &mut [u8], kind: SignatureType, entry_size: usize, count: usize) -> &mut [u8] {
    let length = LIST_HEADER_BYTES + entry_size * count;
    buffer[..16].copy_from_slice(&kind.guid_bytes());
    buffer[16..20].copy_from_slice(&(length as u32).to_le_bytes());
    buffer[20..24].fill(0);
    buffer[24..28].copy_from_slice(&(entry_size as u32).to_le_bytes());
    &mut buffer[LIST_HEADER_BYTES..length]
}

unsafe fn write(
    runtime: *mut RuntimeServices,
    name: &[Char16],
    guid: &Guid,
    attrs: u32,
    data: &[u8],
) -> Status {
    unsafe {
        ((*runtime).set_variable)(name.as_ptr(), guid, attrs, data.len(), data.as_ptr().cast())
    }
}

unsafe fn expect_modes(runtime: *mut RuntimeServices, setup: u8) -> bool {
    for (name, expected) in [(SETUP_MODE.as_slice(), setup), (SECURE_BOOT.as_slice(), 0)] {
        let mut size = 1;
        let mut value = 0xffu8;
        if unsafe {
            ((*runtime).get_variable)(
                name.as_ptr(),
                &GLOBAL_VARIABLE,
                ptr::null_mut(),
                &mut size,
                ptr::from_mut(&mut value).cast(),
            )
        } != Status::SUCCESS
            || size != 1
            || value != expected
        {
            return false;
        }
    }
    true
}

unsafe fn expect_dbx_size(runtime: *mut RuntimeServices, expected: usize) -> bool {
    let mut size = 0;
    let mut attributes = 0;
    let status = unsafe {
        ((*runtime).get_variable)(
            DBX.as_ptr(),
            &DATABASE,
            &mut attributes,
            &mut size,
            ptr::null_mut(),
        )
    };
    status == Status::BUFFER_TOO_SMALL && size == expected && attributes == AUTH
}

unsafe fn query_append(runtime: *mut RuntimeServices) -> bool {
    for attributes in [AUTH, NON_VOLATILE_RUNTIME] {
        let mut plain = [0; 3];
        let mut append = [0; 3];
        for (flags, data) in [
            (attributes, &mut plain),
            (attributes | APPEND_WRITE, &mut append),
        ] {
            let ptr = data.as_mut_ptr();
            if unsafe { ((*runtime).query_variable_info)(flags, ptr, ptr.add(1), ptr.add(2)) }
                != Status::SUCCESS
            {
                return false;
            }
        }
        if plain != append {
            return false;
        }
    }
    true
}
