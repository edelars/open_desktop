#![no_std]
#![no_main]

use core::{arch::global_asm, ffi::c_void, ptr, slice};

use od_firmware::{
    block::block_io::{BLOCK_IO_PROTOCOL_GUID, BlockIo},
    mem::map::MemoryType,
    tpm::{TCG2_PROTOCOL_GUID, Tcg2Protocol},
    uefi::{
        guid::Guid,
        loaded_image::{LOADED_IMAGE_PROTOCOL_GUID, LoadedImageProtocol},
        status::Status,
        system_table::{BootServices, SystemTable},
        types::Handle,
    },
};
use od_firmware_test_apps::{ascii_to_char16, output};

const DATA_LBA: u64 = 40 * 1024 * 1024 / 512;
const READ_BYTES: usize = 64 * 1024 * 1024;
const PE_BYTES: usize = 1024 * 1024;
const PAGES: usize = READ_BYTES / 4096;
const PE_DATA_RVA: usize = 0x3000;
const PE_DATA_BYTES: usize = PE_BYTES - 0x600;
const PCR: u8 = 8;
const EVENT: &[u8] = b"S7-27";
const MEASUREMENT: &[u8; 64] = include_bytes!(concat!(env!("OD_FW_BENCH_DIR"), "/digests.bin"));

#[repr(C)]
struct Call {
    target: usize,
    args: [usize; 6],
}

// EFI x64 uses the Microsoft x64 convention. Reserve 32 bytes of shadow space,
// two stack arguments, and eight bytes to align RSP before the call. Samples
// bracket the call and return themselves, leaving argument setup unmeasured.
global_asm!(
    ".globl od_s727_call",
    ".globl od_s727_call_begin",
    ".globl od_s727_call_end",
    "od_s727_call:",
    "sub rsp, 56",
    "mov r10, rcx",
    "mov rax, [r10]",
    "mov rcx, [r10 + 8]",
    "mov rdx, [r10 + 16]",
    "mov r8, [r10 + 24]",
    "mov r9, [r10 + 32]",
    "mov r11, [r10 + 40]",
    "mov [rsp + 32], r11",
    "mov r11, [r10 + 48]",
    "mov [rsp + 40], r11",
    "od_s727_call_begin:",
    "call rax",
    "od_s727_call_end:",
    "add rsp, 56",
    "ret",
);

unsafe extern "efiapi" {
    fn od_s727_call(call: *const Call) -> Status;
    static od_s727_call_begin: u8;
    static od_s727_call_end: u8;
}

#[inline(never)]
extern "efiapi" fn empty() -> Status {
    Status::SUCCESS
}

macro_rules! line {
    ($system:expr, $text:literal) => {{
        const TEXT: &[u8] = concat!($text, "\r\n\0").as_bytes();
        static WIDE: [u16; TEXT.len()] = ascii_to_char16(TEXT);
        check(unsafe { output($system, WIDE.as_ptr()) })?;
    }};
}

#[unsafe(no_mangle)]
unsafe extern "efiapi" fn efi_main(handle: Handle, system: *mut SystemTable) -> Status {
    if handle.is_null() || system.is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe { run(handle, system) }
        .err()
        .unwrap_or(Status::SUCCESS)
}

unsafe fn run(handle: Handle, system: *mut SystemTable) -> Result<(), Status> {
    let boot = unsafe { (*system).boot_services.as_ref() }.ok_or(Status::NOT_READY)?;
    let image = unsafe { loaded_image(boot, handle)? };
    let esp = image.parent_handle.is_null();
    if esp {
        line!(system, "OD-S7-27 route ESP");
    } else {
        line!(system, "OD-S7-27 route FV");
    }
    unsafe {
        address(system, b"OD-S7-27 base ", image.image_base as usize)?;
        address(system, b"OD-S7-27 entry ", efi_main as *const () as usize)?;
        address(
            system,
            b"OD-S7-27 begin ",
            ptr::addr_of!(od_s727_call_begin) as usize,
        )?;
        address(
            system,
            b"OD-S7-27 end ",
            ptr::addr_of!(od_s727_call_end) as usize,
        )?;
    }
    // The boot-manager run measures entry only. Exercise the public services
    // once per repeat, from the explicitly selected FV application.
    if !esp {
        let block = unsafe { raw_block_io(boot)? };
        let media = unsafe { (*block).media.as_ref() }.ok_or(Status::NO_MEDIA)?;
        if media.block_size != 512 || media.last_block < DATA_LBA + (READ_BYTES / 512) as u64 - 1 {
            return Err(Status::BAD_BUFFER_SIZE);
        }
        let mut buffer = 0;
        check(unsafe { (boot.allocate_pages)(0, MemoryType::LOADER_DATA, PAGES, &mut buffer) })?;
        let result = unsafe { services(system, boot, handle, block, media.media_id, buffer) };
        let free = unsafe { (boot.free_pages)(buffer, PAGES) };
        result?;
        check(free)?;
    }
    line!(system, "OD-S7-27 verified");
    Ok(())
}

unsafe fn address<const N: usize>(
    system: *mut SystemTable,
    prefix: &[u8; N],
    value: usize,
) -> Result<(), Status> {
    let mut wide = [0u16; 34];
    for (out, &byte) in wide.iter_mut().zip(prefix) {
        *out = u16::from(byte);
    }
    for (index, out) in wide[N..N + 16].iter_mut().enumerate() {
        let digit = (value >> (60 - 4 * index)) & 15;
        *out = u16::from(b"0123456789abcdef"[digit]);
    }
    wide[N + 16] = 13;
    wide[N + 17] = 10;
    check(unsafe { output(system, wide.as_ptr()) })
}

fn check(status: Status) -> Result<(), Status> {
    if status == Status::SUCCESS {
        Ok(())
    } else {
        Err(status)
    }
}

#[inline(never)]
fn verify_bytes(actual: &[u8], expected: &[u8]) -> bool {
    if actual.len() != expected.len() {
        return false;
    }
    for (byte, &value) in actual.iter().zip(expected) {
        // Keep these verification reads byte-wise on the firmware interpreter.
        if unsafe { ptr::from_ref(byte).read_volatile() } != value {
            return false;
        }
    }
    true
}

unsafe fn interface<T>(boot: &BootServices, handle: Handle, guid: &Guid) -> Result<*mut T, Status> {
    let mut value = ptr::null_mut::<c_void>();
    check(unsafe { (boot.handle_protocol)(handle, guid, &mut value) })?;
    if value.is_null() {
        return Err(Status::NOT_FOUND);
    }
    Ok(value.cast())
}

unsafe fn loaded_image(
    boot: &BootServices,
    handle: Handle,
) -> Result<&LoadedImageProtocol, Status> {
    let image =
        unsafe { interface::<LoadedImageProtocol>(boot, handle, &LOADED_IMAGE_PROTOCOL_GUID)? };
    Ok(unsafe { &*image })
}

unsafe fn raw_block_io(boot: &BootServices) -> Result<*mut BlockIo, Status> {
    let mut count = 0;
    let mut handles = ptr::null_mut::<Handle>();
    check(unsafe {
        (boot.locate_handle_buffer)(
            2,
            &BLOCK_IO_PROTOCOL_GUID,
            ptr::null_mut(),
            &mut count,
            &mut handles,
        )
    })?;
    let result = (|| {
        if count == 0 || handles.is_null() {
            return Err(Status::NOT_FOUND);
        }
        for &handle in unsafe { slice::from_raw_parts(handles, count) } {
            let io = unsafe { interface::<BlockIo>(boot, handle, &BLOCK_IO_PROTOCOL_GUID)? };
            if unsafe { (*io).media.as_ref() }.is_some_and(|media| media.logical_partition == 0) {
                return Ok(io);
            }
        }
        Err(Status::NOT_FOUND)
    })();
    check(unsafe { (boot.free_pool)(handles.cast()) })?;
    result
}

unsafe fn services(
    system: *mut SystemTable,
    boot: &BootServices,
    parent: Handle,
    block: *mut BlockIo,
    media: u32,
    buffer: u64,
) -> Result<(), Status> {
    check(unsafe {
        od_s727_call(&Call {
            target: empty as *const () as usize,
            args: [0; 6],
        })
    })?;
    line!(system, "OD-S7-27 empty complete");
    check(unsafe {
        od_s727_call(&Call {
            target: (*block).read_blocks as usize,
            args: [
                block as usize,
                media as usize,
                DATA_LBA as usize,
                READ_BYTES,
                buffer as usize,
                0,
            ],
        })
    })?;
    let bytes = buffer as *const u8;
    if unsafe { bytes.read() } != b'M' || unsafe { bytes.add(READ_BYTES - 1).read() } != 0xa6 {
        return Err(Status::DEVICE_ERROR);
    }
    line!(system, "OD-S7-27 ReadBlocks verified");

    let mut loaded = ptr::null_mut();
    check(unsafe {
        od_s727_call(&Call {
            target: boot.load_image as usize,
            args: [
                0,
                parent as usize,
                0,
                buffer as usize,
                PE_BYTES,
                (&raw mut loaded) as usize,
            ],
        })
    })?;
    let image = unsafe { loaded_image(boot, loaded)? };
    let data = unsafe { image.image_base.cast::<u8>().add(PE_DATA_RVA) };
    if image.image_size < (PE_DATA_RVA + PE_DATA_BYTES) as u64
        || unsafe { data.read() } != 0xa5
        || unsafe { data.add(PE_DATA_BYTES - 1).read() } != 0xa5
    {
        return Err(Status::LOAD_ERROR);
    }
    check(unsafe { (boot.unload_image)(loaded) })?;
    line!(system, "OD-S7-27 LoadImage verified");

    let mut interface = ptr::null_mut();
    check(unsafe { (boot.locate_protocol)(&TCG2_PROTOCOL_GUID, ptr::null_mut(), &mut interface) })?;
    let tpm = unsafe { interface.cast::<Tcg2Protocol>().as_ref() }.ok_or(Status::NOT_FOUND)?;
    unsafe { hash_log_extend(tpm, buffer)? };
    line!(system, "OD-S7-27 HashLogExtendEvent verified");
    Ok(())
}

unsafe fn hash_log_extend(tpm: &Tcg2Protocol, buffer: u64) -> Result<(), Status> {
    const HEADER_BYTES: usize = 18;
    const EVENT_BYTES: usize = HEADER_BYTES + EVENT.len();
    let mut event = [0u8; EVENT_BYTES];
    event[..4].copy_from_slice(&(EVENT_BYTES as u32).to_le_bytes());
    event[4] = 14; // Packed EFI_TCG2_EVENT_HEADER size.
    event[8] = 1; // HeaderVersion.
    event[10] = PCR;
    event[14] = 5; // EV_ACTION, with zero flags (log and extend raw bytes).
    event[HEADER_BYTES..].copy_from_slice(EVENT);
    check(unsafe {
        od_s727_call(&Call {
            target: tpm.hash_log_extend_event as usize,
            args: [
                ptr::from_ref(tpm) as usize,
                0,
                buffer as usize,
                PE_BYTES,
                event.as_ptr() as usize,
                0,
            ],
        })
    })?;
    let (mut start, mut last, mut truncated) = (0, 0, 1);
    check(unsafe { (tpm.get_event_log)(tpm, 2, &mut start, &mut last, &mut truncated) })?;
    if start == 0 || last < start || truncated != 0 {
        return Err(Status::VOLUME_CORRUPTED);
    }
    const EVENT2_HEADER_BYTES: usize = 100;
    let record =
        unsafe { slice::from_raw_parts(last as *const u8, EVENT2_HEADER_BYTES + EVENT.len()) };
    if !verify_bytes(
        &record[..14],
        &[PCR, 0, 0, 0, 5, 0, 0, 0, 2, 0, 0, 0, 0x0b, 0],
    ) || !verify_bytes(&record[14..46], &MEASUREMENT[..32])
        || !verify_bytes(&record[46..48], &[0x0c, 0])
        || !verify_bytes(
            &record[96..EVENT2_HEADER_BYTES],
            &(EVENT.len() as u32).to_le_bytes(),
        )
        || !verify_bytes(&record[EVENT2_HEADER_BYTES..], EVENT)
    {
        return Err(Status::VOLUME_CORRUPTED);
    }
    // TPM2_PCR_Read for SHA-256 PCR 8, initially zero on the fresh TPM.
    const READ: [u8; 20] = [
        0x80, 1, 0, 0, 0, 20, 0, 0, 1, 0x7e, 0, 0, 0, 1, 0, 0x0b, 3, 0, 1, 0,
    ];
    let mut reply = [0; 62];
    check(unsafe {
        (tpm.submit_command)(
            tpm,
            READ.len() as u32,
            READ.as_ptr(),
            reply.len() as u32,
            reply.as_mut_ptr(),
        )
    })?;
    if !verify_bytes(&reply[..10], &[0x80, 1, 0, 0, 0, 62, 0, 0, 0, 0])
        || !verify_bytes(
            &reply[14..30],
            &[0, 0, 0, 1, 0, 0x0b, 3, 0, 1, 0, 0, 0, 0, 1, 0, 32],
        )
        || !verify_bytes(&reply[30..], &MEASUREMENT[32..])
    {
        return Err(Status::DEVICE_ERROR);
    }
    Ok(())
}
