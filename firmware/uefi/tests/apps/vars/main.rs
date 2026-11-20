#![no_std]
#![no_main]

use core::{mem::size_of_val, ptr};
use od_firmware::console::COM1_DEVICE_PATH;
use od_firmware::uefi::{
    guid::Guid,
    status::Status,
    system_table::{RuntimeServices, SystemTable},
    types::{Char16, Handle},
};
use od_firmware::var::services::{
    APPEND_WRITE, BOOT_RUNTIME, GLOBAL_VARIABLE, NON_VOLATILE_RUNTIME,
};
use od_firmware_test_apps::{ascii_to_char16, output};

const VENDOR: Guid = Guid {
    data1: 0xd759_9e21,
    data2: 0x6cfd,
    data3: 0x42d1,
    data4: [0xa1, 0xfc, 0x78, 0x6e, 0xf7, 0xa7, 0x74, 0x72],
};
static NAME: [Char16; 7] = ascii_to_char16(b"ODTest\0");
static SETUP_MODE: [Char16; 10] = ascii_to_char16(b"SetupMode\0");
static CONSOLE_OUT: [Char16; 7] = ascii_to_char16(b"ConOut\0");
const BANNER: &[u8] = b"OD-UEFI vars\r\n\0";
static START: [Char16; BANNER.len()] = ascii_to_char16(BANNER);
static START_ADDRESS: &[Char16; BANNER.len()] = &START;
const VERIFIED: &[u8] = b"OD-UEFI vars defaults create read append enumerate delete verified\r\n\0";
static FINISH: [Char16; VERIFIED.len()] = ascii_to_char16(VERIFIED);
static NV_PROBE: [Char16; 8] = ascii_to_char16(b"ODNvram\0");
const NV_VALUE: &[u8; 8] = b"od-s7-10";
const NV_SET: &[u8] = b"OD-UEFI vars NV probe set\r\n\0";
const NV_RESTORED: &[u8] = b"OD-UEFI vars NV probe restored od-s7-10\r\n\0";
static SET_MESSAGE: [Char16; NV_SET.len()] = ascii_to_char16(NV_SET);
static RESTORED_MESSAGE: [Char16; NV_RESTORED.len()] = ascii_to_char16(NV_RESTORED);
const NV_COUNT: &[u8] = b"OD-UEFI vars NV monotonic high advanced\r\n\0";
static COUNT_MESSAGE: [Char16; NV_COUNT.len()] = ascii_to_char16(NV_COUNT);

#[unsafe(no_mangle)]
unsafe extern "efiapi" fn efi_main(handle: Handle, system: *mut SystemTable) -> Status {
    if handle.is_null() || system.is_null() {
        return Status::INVALID_PARAMETER;
    }
    // SAFETY: StartImage supplies the live EFI system table for this image.
    let Some(rt) = (unsafe { (*system).runtime_services.as_ref() }) else {
        return Status::INVALID_PARAMETER;
    };
    let message = unsafe { ptr::read_volatile(&START_ADDRESS) };
    let status = unsafe { output(system, message.as_ptr()) };
    if status != Status::SUCCESS {
        return status;
    }
    if !check_defaults(rt) || !check_store(rt) {
        return Status::LOAD_ERROR;
    }
    let status = unsafe { output(system, FINISH.as_ptr()) };
    if status != Status::SUCCESS {
        return status;
    }
    let restored = match check_nv_probe(rt) {
        Ok(restored) => restored,
        Err(error) => return error,
    };
    let message = if restored {
        RESTORED_MESSAGE.as_ptr()
    } else {
        SET_MESSAGE.as_ptr()
    };
    unsafe {
        let printed = output(system, message);
        if printed != Status::SUCCESS {
            return printed;
        }
        let mut count = 0;
        let boot = (*system).boot_services;
        match ((*boot).get_next_monotonic_count)(&mut count) {
            Status::SUCCESS if count == (u64::from(restored) + 1) << 32 | 1 => {
                output(system, COUNT_MESSAGE.as_ptr())
            }
            Status::DEVICE_ERROR if !restored => Status::SUCCESS,
            _ => Status::LOAD_ERROR,
        }
    }
}

fn check_nv_probe(rt: &RuntimeServices) -> Result<bool, Status> {
    let mut value = [0; NV_VALUE.len()];
    let mut size = value.len();
    let mut attributes = 0;
    // SAFETY: the name and all buffers are live and cover their EFI lengths.
    unsafe {
        match (rt.get_variable)(
            NV_PROBE.as_ptr(),
            &VENDOR,
            &mut attributes,
            &mut size,
            value.as_mut_ptr().cast(),
        ) {
            Status::NOT_FOUND => {
                let status = (rt.set_variable)(
                    NV_PROBE.as_ptr(),
                    &VENDOR,
                    NON_VOLATILE_RUNTIME,
                    NV_VALUE.len(),
                    NV_VALUE.as_ptr().cast(),
                );
                if status != Status::SUCCESS {
                    return Err(status);
                }
                Ok(false)
            }
            Status::SUCCESS
                if size == NV_VALUE.len()
                    && attributes == NON_VOLATILE_RUNTIME
                    && &value == NV_VALUE =>
            {
                Ok(true)
            }
            _ => Err(Status::LOAD_ERROR),
        }
    }
}

fn check_defaults(rt: &RuntimeServices) -> bool {
    let mut data = [0u8; 24];
    let mut size = data.len();
    let mut attrs = 0;
    // SAFETY: the NUL-terminated names and output buffers are live for each call.
    unsafe {
        (rt.get_variable)(
            CONSOLE_OUT.as_ptr(),
            &GLOBAL_VARIABLE,
            &mut attrs,
            &mut size,
            data.as_mut_ptr().cast(),
        ) == Status::SUCCESS
            && size == data.len()
            && attrs == NON_VOLATILE_RUNTIME
            && data == COM1_DEVICE_PATH
            && (rt.get_variable)(
                SETUP_MODE.as_ptr(),
                &GLOBAL_VARIABLE,
                &mut attrs,
                &mut size,
                data.as_mut_ptr().cast(),
            ) == Status::SUCCESS
            && size == 1
            && attrs == BOOT_RUNTIME
            && data[0] == 1
            && (rt.set_variable)(SETUP_MODE.as_ptr(), &GLOBAL_VARIABLE, 0, 0, ptr::null())
                == Status::WRITE_PROTECTED
    }
}

fn check_store(rt: &RuntimeServices) -> bool {
    let (mut total, mut remaining, mut maximum) = (0, 0, 0);
    let mut length = 0;
    let mut attributes = 0;
    let mut data = [0u8; 5];
    let mut remaining_after = 0;
    let name = NAME.as_ptr();
    let attrs = NON_VOLATILE_RUNTIME;
    // SAFETY: both input values and all output buffers cover their declared sizes.
    unsafe {
        (rt.query_variable_info)(
            attrs | APPEND_WRITE,
            &mut total,
            &mut remaining,
            &mut maximum,
        ) == Status::SUCCESS
            && remaining <= total
            && maximum >= data.len() as u64
            && (rt.set_variable)(name, &VENDOR, attrs, 3, b"abc".as_ptr().cast()) == Status::SUCCESS
            && (rt.get_variable)(name, &VENDOR, &mut attributes, &mut length, ptr::null_mut())
                == Status::BUFFER_TOO_SMALL
            && length == 3
            && attributes == attrs
            && (rt.set_variable)(
                name,
                &VENDOR,
                attrs | APPEND_WRITE,
                2,
                b"de".as_ptr().cast(),
            ) == Status::SUCCESS
            && {
                length = data.len();
                (rt.get_variable)(
                    name,
                    &VENDOR,
                    &mut attributes,
                    &mut length,
                    data.as_mut_ptr().cast(),
                ) == Status::SUCCESS
            }
            && length == data.len()
            && attributes == attrs
            && &data == b"abcde"
            && listed(rt)
            && (rt.query_variable_info)(attrs, &mut total, &mut remaining_after, &mut maximum)
                == Status::SUCCESS
            && remaining_after < remaining
            && (rt.set_variable)(name, &VENDOR, 0, 0, ptr::null()) == Status::SUCCESS
            && (rt.get_variable)(
                name,
                &VENDOR,
                ptr::null_mut(),
                &mut length,
                data.as_mut_ptr().cast(),
            ) == Status::NOT_FOUND
            && (rt.query_variable_info)(attrs, &mut total, &mut remaining_after, &mut maximum)
                == Status::SUCCESS
            && remaining_after == remaining
    }
}

fn listed(rt: &RuntimeServices) -> bool {
    let mut name = [0u16; 128];
    let mut guid = GLOBAL_VARIABLE;
    let mut size = 2;
    // SAFETY: the initial input is an empty name. The same allocated buffer is
    // reused as the cursor, resetting its capacity before each call.
    if unsafe { (rt.get_next_variable_name)(&mut size, name.as_mut_ptr(), &mut guid) }
        != Status::BUFFER_TOO_SMALL
        || size <= 2
        || name[0] != 0
    {
        return false;
    }
    for _ in 0..32 {
        size = size_of_val(&name);
        if unsafe { (rt.get_next_variable_name)(&mut size, name.as_mut_ptr(), &mut guid) }
            != Status::SUCCESS
        {
            return false;
        }
        if guid == VENDOR && size == size_of_val(&NAME) && name[..NAME.len()] == NAME {
            return true;
        }
    }
    false
}
