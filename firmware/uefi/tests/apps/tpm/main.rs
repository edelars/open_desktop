#![no_std]
#![no_main]

use core::{arch::asm, mem::MaybeUninit, ptr, slice};
use od_firmware::acpi::{
    blob::{ACPI_RECLAIM_END, ACPI_REGION_BASE},
    install::EFI_ACPI_20_TABLE_GUID,
    rsdp::{RSDP_LEN, validate_rsdp},
    sha256::sha256,
    table::{SDT_HEADER_LEN, validate_table},
};
use od_firmware::mem::map::{MemoryDescriptor, MemoryType};
use od_firmware::tpm::{FINAL_EVENTS_GUID, TCG2_PROTOCOL_GUID, Tcg2Protocol};
use od_firmware::uefi::{
    guid::Guid,
    loaded_image::{LOADED_IMAGE_PROTOCOL_GUID, LoadedImageProtocol},
    status::Status,
    system_table::SystemTable,
    types::Handle,
};
use od_firmware_test_apps::{ascii_to_char16, output};

const LOG_CAPACITY: usize = 32 * 1024;
const PCRS: [u32; 4] = [0, 1, 4, 5];

#[unsafe(no_mangle)]
unsafe extern "efiapi" fn efi_main(handle: Handle, system: *mut SystemTable) -> Status {
    if handle.is_null() || system.is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe { verify(handle, system) }
        .err()
        .unwrap_or(Status::SUCCESS)
}

unsafe fn verify(handle: Handle, system: *mut SystemTable) -> Result<(), Status> {
    macro_rules! line {
        ($text:literal) => {{
            const TEXT: &[u8] = concat!($text, "\r\n\0").as_bytes();
            static MESSAGE: [u16; TEXT.len()] = ascii_to_char16(TEXT);
            check(unsafe { output(system, MESSAGE.as_ptr()) })?;
        }};
    }
    unsafe { verify_acpi(system) }.ok_or(Status::LOAD_ERROR)?;
    line!("OD-UEFI TPM2: CRB method=7 control=0xfed40040");
    let boot = unsafe { &*(*system).boot_services };
    let mut interface = ptr::null_mut();
    check(unsafe { (boot.handle_protocol)(handle, &LOADED_IMAGE_PROTOCOL_GUID, &mut interface) })?;
    let image =
        unsafe { interface.cast::<LoadedImageProtocol>().as_ref() }.ok_or(Status::NOT_FOUND)?;
    let esp = image.parent_handle.is_null();
    check(unsafe { (boot.locate_protocol)(&TCG2_PROTOCOL_GUID, ptr::null_mut(), &mut interface) })?;
    let tpm = unsafe { interface.cast::<Tcg2Protocol>().as_ref() }.ok_or(Status::NOT_FOUND)?;

    let mut caps = [0; 36];
    caps[0] = 27;
    if unsafe { (tpm.get_capability)(tpm, caps.as_mut_ptr()) } != Status::BUFFER_TOO_SMALL
        || caps[0] != 28
    {
        return Err(Status::BAD_BUFFER_SIZE);
    }
    caps[0] = 36;
    check(unsafe { (tpm.get_capability)(tpm, caps.as_mut_ptr()) })?;
    let mut banks = 0;
    check(unsafe { (tpm.get_active_pcr_banks)(tpm, &mut banks) })?;
    if caps[..5] != [36, 1, 1, 1, 1]
        || le32(&caps, 8) != 2
        || le32(&caps, 12) != 2
        || caps[16] != 1
        || caps[18..22] != [0x80, 0x0f, 0x80, 0x0f]
        || le32(&caps, 28) != 1
        || le32(&caps, 32) != banks
        || banks != 2
    {
        return Err(Status::DEVICE_ERROR);
    }
    line!("OD-UEFI TCG2: SHA-256 bank");

    let mut start = 0;
    let mut last = 0;
    let mut truncated = 1;
    check(unsafe { (tpm.get_event_log)(tpm, 2, &mut start, &mut last, &mut truncated) })?;
    if start == 0 || truncated != 0 {
        return Err(Status::VOLUME_CORRUPTED);
    }
    let before = usize::try_from(last.checked_sub(start).ok_or(Status::VOLUME_CORRUPTED)?)
        .map_err(|_| Status::VOLUME_CORRUPTED)?;
    if !(65..=LOG_CAPACITY - 50).contains(&before) {
        return Err(Status::VOLUME_CORRUPTED);
    }
    let last_header = unsafe { slice::from_raw_parts(last as *const u8, 50) };
    let extent = before + 50 + le32(last_header, 46) as usize;
    if extent > LOG_CAPACITY {
        return Err(Status::VOLUME_CORRUPTED);
    }
    let log = unsafe { slice::from_raw_parts(start as *const u8, extent) };
    let replayed = replay(log, before, esp).ok_or(Status::VOLUME_CORRUPTED)?;
    line!("OD-UEFI TCG2: Spec ID Event03");
    if esp {
        line!("OD-UEFI TCG2: boot variables and GPT verified");
    }

    // The final events table records a subsequent event. EV_NO_ACTION must
    // carry zero digests and must not change the PCRs just replayed.
    let final_events = unsafe { config(system, &FINAL_EVENTS_GUID) }.ok_or(Status::NOT_FOUND)?;
    let initial = unsafe { slice::from_raw_parts(final_events.cast_const(), 16) };
    if le64(initial, 0) != 1 || le64(initial, 8) != 0 {
        return Err(Status::VOLUME_CORRUPTED);
    }
    const NOTE: &[u8] = b"Open Desktop diagnostic";
    let mut event = [0u8; 18 + NOTE.len()];
    let event_len = event.len() as u32;
    event[..4].copy_from_slice(&event_len.to_le_bytes());
    event[4] = 14;
    event[8] = 1;
    event[14] = 3;
    event[18..].copy_from_slice(NOTE);
    check(unsafe {
        (tpm.hash_log_extend_event)(
            tpm,
            0,
            NOTE.as_ptr() as u64,
            NOTE.len() as u64,
            event.as_ptr(),
        )
    })?;
    let mut second_start = 0;
    let mut second_last = 0;
    check(unsafe {
        (tpm.get_event_log)(tpm, 2, &mut second_start, &mut second_last, &mut truncated)
    })?;
    let final_entry = unsafe { slice::from_raw_parts(final_events.cast_const(), 66 + NOTE.len()) };
    if second_start != start
        || truncated != 0
        || second_last <= last
        || le64(final_entry, 8) != 1
        || le32(final_entry, 16) != 0
        || le32(final_entry, 20) != 3
        || final_entry[30..62] != [0; 32]
        || le32(final_entry, 62) as usize != NOTE.len()
        || &final_entry[66..] != NOTE
    {
        return Err(Status::VOLUME_CORRUPTED);
    }
    line!("OD-UEFI TCG2: final events verified");

    const READ: [u8; 20] = [
        0x80, 1, 0, 0, 0, 20, 0, 0, 1, 0x7e, 0, 0, 0, 1, 0, 0x0b, 3, 0x33, 0, 0,
    ];
    let mut reply = [0; 164];
    check(unsafe {
        (tpm.submit_command)(
            tpm,
            READ.len() as u32,
            READ.as_ptr(),
            reply.len() as u32,
            reply.as_mut_ptr(),
        )
    })?;
    if reply[2..10] != [0, 0, 0, 164, 0, 0, 0, 0]
        || reply[14..28] != [0, 0, 0, 1, 0, 0x0b, 3, 0x33, 0, 0, 0, 0, 0, 4]
    {
        return Err(Status::DEVICE_ERROR);
    }
    for (bank, value) in reply[28..].chunks_exact(34).zip(replayed) {
        if bank[..2] != [0, 32] || bank[2..] != value {
            return Err(Status::DEVICE_ERROR);
        }
    }
    line!("OD-UEFI PCR0/1/4/5 log replay: match");

    const GET_RANDOM: [u8; 12] = [0x80, 1, 0, 0, 0, 12, 0, 0, 1, 0x7b, 0, 16];
    let mut random = [0; 28];
    check(unsafe {
        (tpm.submit_command)(
            tpm,
            GET_RANDOM.len() as u32,
            GET_RANDOM.as_ptr(),
            random.len() as u32,
            random.as_mut_ptr(),
        )
    })?;
    if random[2..12] != [0, 0, 0, 28, 0, 0, 0, 0, 0, 16] {
        return Err(Status::DEVICE_ERROR);
    }
    line!("OD-UEFI TPM2_GetRandom: 16 bytes");

    const TEST_PARMS: [u8; 22] = [
        0x80, 1, 0, 0, 0, 22, 0, 0, 1, 0x8a, // TPM2_TestParms
        0, 0x23, 0, 0x10, 0, 0x18, 0, 0x0b, 0, 3, 0, 0x10, // ECDSA-SHA256, P-256
    ];
    let mut parameters_reply = [0; 10];
    check(unsafe {
        (tpm.submit_command)(
            tpm,
            TEST_PARMS.len() as u32,
            TEST_PARMS.as_ptr(),
            parameters_reply.len() as u32,
            parameters_reply.as_mut_ptr(),
        )
    })?;
    if parameters_reply != [0x80, 1, 0, 0, 0, 10, 0, 0, 0, 0] {
        return Err(Status::DEVICE_ERROR);
    }
    line!("OD-UEFI TPM2_TestParms: ECDSA-SHA256 P-256");
    unsafe { exit_and_verify_final_events(handle, system, final_events.cast_const()) }
}

unsafe fn exit_and_verify_final_events(
    handle: Handle,
    system: *mut SystemTable,
    events: *const u8,
) -> Result<(), Status> {
    let boot = unsafe { &*(*system).boot_services };
    let exit = boot.exit_boot_services;
    let mut map = MaybeUninit::<[MemoryDescriptor; 256]>::uninit();
    for attempt in 0..3 {
        let mut size = size_of_val(&map);
        let mut key = 0;
        let mut descriptor_size = 0;
        let mut version = 0;
        check(unsafe {
            (boot.get_memory_map)(
                &mut size,
                map.as_mut_ptr().cast(),
                &mut key,
                &mut descriptor_size,
                &mut version,
            )
        })?;
        if descriptor_size != size_of::<MemoryDescriptor>()
            || size > size_of_val(&map)
            || size % descriptor_size != 0
        {
            return Err(Status::LOAD_ERROR);
        }
        let descriptors = unsafe {
            slice::from_raw_parts(
                map.as_ptr().cast::<MemoryDescriptor>(),
                size / descriptor_size,
            )
        };
        if !descriptors.iter().any(|region| {
            region.memory_type == MemoryType::RUNTIME_SERVICES_DATA.raw()
                && region.attribute >> 63 == 1
                && region.physical_start <= events as u64
                && (events as u64) + LOG_CAPACITY as u64
                    <= region.physical_start + region.number_of_pages * 4096
        }) {
            return Err(Status::LOAD_ERROR);
        }
        let status = unsafe {
            exit(
                handle,
                if attempt == 0 {
                    key.wrapping_add(1)
                } else {
                    key
                },
            )
        };
        if status == Status::SUCCESS {
            let verified = attempt > 0 && unsafe { verify_exit_events(events) };
            let output: &[u8] = if verified {
                b"OD-UEFI TCG2: final events retained after EBS\r\n"
            } else {
                b"OD-UEFI TCG2: final events verification failed\r\n"
            };
            for &byte in output {
                unsafe {
                    asm!("out dx, al", in("dx") 0x3f8u16, in("al") byte, options(nomem, nostack, preserves_flags))
                };
            }
            unsafe { asm!("cli", "2: hlt", "jmp 2b", options(nomem, nostack, noreturn)) };
        }
        if status != Status::INVALID_PARAMETER {
            return Err(status);
        }
    }
    Err(Status::INVALID_PARAMETER)
}

unsafe fn verify_exit_events(events: *const u8) -> bool {
    const INVOCATION: &[u8] = b"Exit Boot Services Invocation";
    const FAILURE: &[u8] = b"Exit Boot Services Returned with Failure";
    const SUCCESS: &[u8] = b"Exit Boot Services Returned with Success";
    let header = unsafe { slice::from_raw_parts(events, 16) };
    let count = le64(header, 8);
    if count != 5 && count != 7 {
        return false;
    }
    let mut offset = 16 + 50 + b"Open Desktop diagnostic".len();
    for i in 1..count {
        let expected = if i == count - 1 {
            SUCCESS
        } else if i % 2 == 0 {
            FAILURE
        } else {
            INVOCATION
        };
        let header = unsafe { slice::from_raw_parts(events.add(offset), 50) };
        if le32(header, 0) != 5
            || le32(header, 4) != 0x8000_0007
            || le32(header, 8) != 1
            || le32(header, 46) as usize != expected.len()
        {
            return false;
        }
        let data = unsafe { slice::from_raw_parts(events.add(offset + 50), expected.len()) };
        if data != expected || header[14..46] != sha256(expected) {
            return false;
        }
        offset += 50 + expected.len();
    }
    true
}

fn replay(log: &[u8], last: usize, esp: bool) -> Option<[[u8; 32]; 4]> {
    if log[..8] != [0, 0, 0, 0, 3, 0, 0, 0]
        || log[8..28] != [0; 20]
        || le32(log, 28) != 33
        || &log[32..48] != b"Spec ID Event03\0"
        || log[52..65] != [0, 2, 106, 2, 1, 0, 0, 0, 0x0b, 0, 32, 0, 0]
    {
        return None;
    }
    let mut offset = 65usize;
    let mut banks = [[0u8; 32]; 4];
    let mut seen = 0u8;
    let mut required_events = 0u16;
    loop {
        let header = log.get(offset..offset.checked_add(50)?)?;
        if le32(header, 8) != 1 || header[12..14] != [0x0b, 0] {
            return None;
        }
        let next = offset
            .checked_add(50)?
            .checked_add(le32(header, 46) as usize)?;
        let payload = log.get(offset + 50..next)?;
        let pcr = le32(header, 0);
        let kind = le32(header, 4);
        match (pcr, kind) {
            (0, 8) => required_events |= 1,
            (0, 0x8000_000a) if required_events & 1 == 1 => required_events |= 2,
            (1, 0x8000_000b) => required_events |= 4,
            (4, 0x8000_0007) if payload == b"Calling EFI Application from Boot Option" => {
                required_events |= 8;
            }
            (4, 4) if required_events & 8 == 8 => required_events |= 16,
            (4, 0x8000_0003) if required_events & 16 == 16 => required_events |= 32,
            (1, 0x8000_000c) if required_events & 16 == 0 => {
                let name_len = le64(payload.get(..32)?, 16);
                let name = payload.get(32..32 + usize::try_from(name_len).ok()?.checked_mul(2)?)?;
                if name == b"B\0o\0o\0t\0O\0r\0d\0e\0r\0" {
                    required_events |= 64;
                } else if name == b"B\0o\0o\0t\x000\x000\x000\x000\0" {
                    required_events |= 128;
                }
            }
            (5, 0x8000_000d) if required_events & 16 == 16 => {
                let gpt = payload.get(..100)?;
                let slots = usize::try_from(le64(gpt, 92)).ok()?;
                if !gpt.starts_with(b"EFI PART")
                    || gpt[56..72] != [0; 16]
                    || slots != le32(gpt, 80) as usize
                    || payload.len() != 100 + slots.checked_mul(128)?
                {
                    return None;
                }
                required_events |= 256;
            }
            (4, 0x8000_0007) if payload == b"Booting to Boot0000 Option" => {
                required_events |= 512;
            }
            _ => (),
        }
        if le32(header, 4) != 3
            && let Some(index) = PCRS.iter().position(|&pcr| pcr == le32(header, 0))
        {
            let mut pair = [0; 64];
            pair[..32].copy_from_slice(&banks[index]);
            pair[32..].copy_from_slice(&header[14..46]);
            banks[index] = sha256(&pair);
            seen |= 1 << index;
        }
        if offset == last {
            let expected = if esp { 1023 } else { 63 };
            return (seen == 15 && required_events & expected == expected).then_some(banks);
        }
        if next > last {
            return None;
        }
        offset = next;
    }
}

unsafe fn verify_acpi(system: *mut SystemTable) -> Option<()> {
    let rsdp = unsafe { config(system, &EFI_ACPI_20_TABLE_GUID) }? as u64;
    let rsdp = unsafe { reclaim(rsdp, RSDP_LEN) }?;
    validate_rsdp(rsdp).ok()?;
    let xsdt = unsafe { sdt(le64(rsdp, 24)) }?;
    validate_table(xsdt, b"XSDT").ok()?;
    for entry in xsdt[SDT_HEADER_LEN..].chunks_exact(8) {
        let table = unsafe { sdt(le64(entry, 0)) }?;
        if &table[..4] == b"TPM2" {
            validate_table(table, b"TPM2").ok()?;
            return (table.len() == 52
                && table[8] == 6
                && le32(table, 36) == 0
                && le64(table, 40) == 0xfed4_0040
                && le32(table, 48) == 7)
                .then_some(());
        }
    }
    None
}

unsafe fn config(system: *mut SystemTable, guid: &Guid) -> Option<*mut u8> {
    let system = unsafe { &*system };
    let (entries, count) = system.configuration_tables();
    if entries.is_null() || count > 32 {
        return None;
    }
    let entries = unsafe { slice::from_raw_parts(entries, count) };
    entries
        .iter()
        .find(|entry| entry.vendor_guid == *guid)
        .and_then(|entry| (!entry.vendor_table.is_null()).then_some(entry.vendor_table.cast()))
}

unsafe fn reclaim(address: u64, len: usize) -> Option<&'static [u8]> {
    if address < ACPI_REGION_BASE || address.checked_add(len as u64)? > ACPI_RECLAIM_END {
        return None;
    }
    Some(unsafe { slice::from_raw_parts(address as *const u8, len) })
}

unsafe fn sdt(address: u64) -> Option<&'static [u8]> {
    let header = unsafe { reclaim(address, SDT_HEADER_LEN) }?;
    let len = le32(header, 4) as usize;
    if len < SDT_HEADER_LEN {
        return None;
    }
    unsafe { reclaim(address, len) }
}

fn le32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn le64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn check(status: Status) -> Result<(), Status> {
    if status == Status::SUCCESS {
        Ok(())
    } else {
        Err(status)
    }
}
