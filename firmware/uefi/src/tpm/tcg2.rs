//! UEFI TCG2 protocol for the fixed CRB and active SHA-256 PCR bank.

use core::{ffi::c_void, ptr, slice};
use sha2::{Digest, Sha256};

use crate::bs;
use crate::mem::map::MemoryType;
use crate::pe::{DigestAlgorithm, PeImage, authenticode_digest};
use crate::uefi::{guid::Guid, status::Status};

use super::crb_driver::Crb;
use super::event_log::{CAPACITY, EV_NO_ACTION, EventLog};

pub const TCG2_PROTOCOL_GUID: Guid = Guid {
    data1: 0x607f766c,
    data2: 0x7455,
    data3: 0x42be,
    data4: [0x93, 0x0b, 0xe4, 0xd7, 0x6d, 0xb2, 0x72, 0x0f],
};
pub const FINAL_EVENTS_GUID: Guid = Guid {
    data1: 0x1e2ed096,
    data2: 0x30e2,
    data3: 0x4254,
    data4: [0xbd, 0x89, 0x86, 0x3b, 0xbe, 0xf8, 0x23, 0x25],
};
const SHA256_BANK: u32 = 2;
const TCG2_FORMAT: u32 = 2;
const EXTEND_ONLY: u64 = 1;
const PE_COFF_IMAGE: u64 = 0x10;

#[repr(C)]
pub struct Tcg2Protocol {
    pub get_capability: unsafe extern "efiapi" fn(*const Self, *mut u8) -> Status,
    pub get_event_log:
        unsafe extern "efiapi" fn(*const Self, u32, *mut u64, *mut u64, *mut u8) -> Status,
    pub hash_log_extend_event:
        unsafe extern "efiapi" fn(*const Self, u64, u64, u64, *const u8) -> Status,
    pub submit_command:
        unsafe extern "efiapi" fn(*const Self, u32, *const u8, u32, *mut u8) -> Status,
    pub get_active_pcr_banks: unsafe extern "efiapi" fn(*const Self, *mut u32) -> Status,
    pub set_active_pcr_banks: unsafe extern "efiapi" fn(*const Self, u32) -> Status,
    pub get_result_of_set_active_pcr_banks:
        unsafe extern "efiapi" fn(*const Self, *mut u32, *mut u32) -> Status,
}

static PROTOCOL: Tcg2Protocol = Tcg2Protocol {
    get_capability,
    get_event_log,
    hash_log_extend_event,
    submit_command,
    get_active_pcr_banks,
    set_active_pcr_banks,
    get_result_of_set_active_pcr_banks,
};

pub(super) struct Tcg2 {
    crb: Crb,
    manufacturer: u32,
    log: EventLog,
    pub(super) separated: bool,
    #[cfg(target_os = "none")]
    pub(super) pending_gpt: Option<bs::memory::BootPool>,
}

// Only the BSP accesses TCG2. No operation holding this state calls back into
// Boot Services or invokes guest notification functions.
pub(super) static mut TCG2: Option<Tcg2> = None;

pub fn active() -> bool {
    unsafe { (*ptr::addr_of!(TCG2)).is_some() }
}

/// Initializes the handoff-validated CRB on the BSP before loading EFI images.
///
/// # Safety
/// Boot Services are initialized and the aperture is mapped as device memory.
pub unsafe fn install(base: u64) -> Result<(), Status> {
    if active() {
        return Err(Status::ALREADY_STARTED);
    }
    let crb = unsafe { Crb::open(base)? };
    let manufacturer = initialize_tpm(crb)?;
    let main_log = allocate_log(MemoryType::BOOT_SERVICES_DATA)?;
    let final_events = match allocate_log(MemoryType::RUNTIME_SERVICES_DATA) {
        Ok(log) => log,
        Err(status) => {
            unsafe { bs::memory::free_pool(main_log.cast()) };
            return Err(status);
        }
    };
    let log = unsafe { EventLog::new(main_log, final_events) };
    let status = unsafe {
        bs::config_table::install_configuration_table(&FINAL_EVENTS_GUID, final_events.cast())
    };
    if status == Status::SUCCESS {
        unsafe {
            ptr::addr_of_mut!(TCG2).write(Some(Tcg2 {
                crb,
                manufacturer,
                log,
                separated: false,
                #[cfg(target_os = "none")]
                pending_gpt: None,
            }));
        }
        let mut handle = ptr::null_mut();
        let status = unsafe {
            bs::protocol::install_protocol_interface(
                &mut handle,
                &TCG2_PROTOCOL_GUID,
                0, // EFI_NATIVE_INTERFACE
                ptr::from_ref(&PROTOCOL).cast_mut().cast(),
            )
        };
        if status == Status::SUCCESS {
            return Ok(());
        }
        unsafe { ptr::addr_of_mut!(TCG2).write(None) };
        let _ = unsafe {
            bs::config_table::install_configuration_table(&FINAL_EVENTS_GUID, ptr::null_mut())
        };
        unsafe {
            bs::memory::free_pool(main_log.cast());
            bs::memory::free_pool(final_events.cast());
        }
        return Err(status);
    }
    unsafe {
        bs::memory::free_pool(main_log.cast());
        bs::memory::free_pool(final_events.cast());
    }
    Err(status)
}

fn allocate_log(ty: MemoryType) -> Result<*mut u8, Status> {
    let mut buffer: *mut c_void = ptr::null_mut();
    let status = unsafe { bs::memory::allocate_pool(ty, CAPACITY, &mut buffer) };
    if status != Status::SUCCESS {
        return Err(status);
    }
    Ok(buffer.cast())
}

fn checked_command(crb: Crb, cmd: &[u8], out: &mut [u8]) -> Result<usize, Status> {
    let size = crb.submit(cmd.as_ptr(), cmd.len(), out.as_mut_ptr(), out.len())?;
    if out[6..10] != [0; 4] {
        return Err(Status::DEVICE_ERROR);
    }
    Ok(size)
}

fn initialize_tpm(crb: Crb) -> Result<u32, Status> {
    const STARTUP: [u8; 12] = [0x80, 1, 0, 0, 0, 12, 0, 0, 1, 0x44, 0, 0];
    const SELF_TEST: [u8; 11] = [0x80, 1, 0, 0, 0, 11, 0, 0, 1, 0x43, 1];
    const GET_PCRS: [u8; 22] = [
        0x80, 1, 0, 0, 0, 22, 0, 0, 1, 0x7a, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 1,
    ];
    const GET_MANUFACTURER: [u8; 22] = [
        0x80, 1, 0, 0, 0, 22, 0, 0, 1, 0x7a, 0, 0, 0, 6, 0, 0, 1, 5, 0, 0, 0, 1,
    ];
    let mut response = [0; 32];
    checked_command(crb, &STARTUP, &mut response)?;
    checked_command(crb, &SELF_TEST, &mut response)?;
    let size = checked_command(crb, &GET_PCRS, &mut response)?;
    // No continuation, one SHA256 bank with all 24 PCRs implemented.
    if size != 25 || response[10..25] != [0, 0, 0, 0, 5, 0, 0, 0, 1, 0, 0x0b, 3, 0xff, 0xff, 0xff] {
        return Err(Status::UNSUPPORTED);
    }
    let size = checked_command(crb, &GET_MANUFACTURER, &mut response)?;
    if size != 27 || response[11..23] != [0, 0, 0, 6, 0, 0, 0, 1, 0, 0, 1, 5] {
        return Err(Status::DEVICE_ERROR);
    }
    Ok(u32::from_be_bytes(response[23..27].try_into().unwrap()))
}

pub(crate) fn hash(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// Measures existing immutable bytes on the BSP when a TPM is configured.
pub fn measure_bytes(pcr: u32, kind: u32, data: &[u8], event: &[u8]) -> Result<(), Status> {
    if !active() {
        return Ok(());
    }
    measure_digest(pcr, kind, &hash(data), event, false)
}

pub(crate) fn measure_digest(
    pcr: u32,
    kind: u32,
    digest: &[u8; 32],
    event: &[u8],
    extend_only: bool,
) -> Result<(), Status> {
    measure_parts(pcr, kind, digest, &[event], extend_only)
}

pub(super) fn measure_parts(
    pcr: u32,
    kind: u32,
    digest: &[u8; 32],
    event: &[&[u8]],
    extend_only: bool,
) -> Result<(), Status> {
    let Some(state) = (unsafe { &mut *ptr::addr_of_mut!(TCG2) }).as_mut() else {
        return Ok(());
    };
    if pcr >= 24 {
        return Err(Status::INVALID_PARAMETER);
    }
    if kind != EV_NO_ACTION {
        const PCR_EXTEND: [u8; 33] = [
            0x80, 2, 0, 0, 0, 65, 0, 0, 1, 0x82, 0, 0, 0, 0, 0, 0, 0, 9, 0x40, 0, 0, 9, 0, 0, 0, 0,
            0, 0, 0, 0, 1, 0, 0x0b,
        ];
        let mut command = [0; 65];
        command[..33].copy_from_slice(&PCR_EXTEND);
        command[10..14].copy_from_slice(&pcr.to_be_bytes());
        command[33..].copy_from_slice(digest);
        checked_command(state.crb, &command, &mut [0; 24])?;
    }
    let digest = if kind == EV_NO_ACTION {
        &[0; 32]
    } else {
        digest
    };
    if !extend_only {
        state.log.append(pcr, kind, digest, event);
    }
    if state.log.full() {
        return Err(Status::VOLUME_FULL);
    }
    Ok(())
}

fn valid_this(this: *const Tcg2Protocol) -> bool {
    ptr::eq(this, &PROTOCOL) && unsafe { bs::boot_services_active() } && active()
}

unsafe extern "efiapi" fn get_capability(this: *const Tcg2Protocol, cap: *mut u8) -> Status {
    if !valid_this(this) || cap.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let (manufacturer, capacity) = {
        let state = unsafe { (*ptr::addr_of!(TCG2)).as_ref().unwrap() };
        (state.manufacturer, state.crb.capacity())
    };
    let supplied = unsafe { cap.read() } as usize;
    const FULL: usize = 36;
    const MINIMUM: usize = 28;
    if supplied < MINIMUM {
        unsafe { cap.write(MINIMUM as u8) };
        return Status::BUFFER_TOO_SMALL;
    }
    let mut bytes = [0; FULL];
    bytes[..5].copy_from_slice(&[FULL as u8, 1, 1, 1, 1]);
    bytes[8..12].copy_from_slice(&SHA256_BANK.to_le_bytes());
    bytes[12..16].copy_from_slice(&TCG2_FORMAT.to_le_bytes());
    bytes[16] = 1;
    bytes[18..20].copy_from_slice(&capacity.to_le_bytes());
    bytes[20..22].copy_from_slice(&capacity.to_le_bytes());
    bytes[24..28].copy_from_slice(&manufacturer.to_le_bytes());
    bytes[28..32].copy_from_slice(&1u32.to_le_bytes());
    bytes[32..36].copy_from_slice(&SHA256_BANK.to_le_bytes());
    let returned = supplied.min(FULL) & !3;
    bytes[0] = returned as u8;
    unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), cap, returned) };
    Status::SUCCESS
}

unsafe extern "efiapi" fn get_event_log(
    this: *const Tcg2Protocol,
    format: u32,
    location: *mut u64,
    last: *mut u64,
    truncated: *mut u8,
) -> Status {
    if !valid_this(this) || location.is_null() || last.is_null() || truncated.is_null() {
        return Status::INVALID_PARAMETER;
    }
    if format != TCG2_FORMAT {
        return Status::INVALID_PARAMETER;
    }
    let (start, end, full) = unsafe { (*ptr::addr_of_mut!(TCG2)).as_mut().unwrap().log.get() };
    unsafe {
        location.write(start);
        last.write(end);
        truncated.write(u8::from(full));
    }
    Status::SUCCESS
}

unsafe extern "efiapi" fn hash_log_extend_event(
    this: *const Tcg2Protocol,
    flags: u64,
    data: u64,
    length: u64,
    event: *const u8,
) -> Status {
    if !valid_this(this)
        || event.is_null()
        || data == 0
        || flags & !(EXTEND_ONLY | PE_COFF_IMAGE) != 0
    {
        return Status::INVALID_PARAMETER;
    }
    let size = unsafe { event.cast::<u32>().read_unaligned() } as usize;
    if size < 18 {
        return Status::INVALID_PARAMETER;
    }
    let header_size = unsafe { event.add(4).cast::<u32>().read_unaligned() };
    let version = unsafe { event.add(8).cast::<u16>().read_unaligned() };
    let pcr = unsafe { event.add(10).cast::<u32>().read_unaligned() };
    let kind = unsafe { event.add(14).cast::<u32>().read_unaligned() };
    if header_size != 14
        || version != 1
        || pcr >= 24
        || length > isize::MAX as u64
        || data.checked_add(length).is_none()
        || (event as usize).checked_add(size).is_none()
    {
        return Status::INVALID_PARAMETER;
    }
    let bytes = unsafe { slice::from_raw_parts(data as *const u8, length as usize) };
    let payload = unsafe { slice::from_raw_parts(event.add(18), size - 18) };
    let digest = if flags & PE_COFF_IMAGE != 0 {
        let Ok(pe) = PeImage::parse(bytes) else {
            return Status::UNSUPPORTED;
        };
        let Ok(digest) = authenticode_digest(&pe, DigestAlgorithm::Sha256) else {
            return Status::UNSUPPORTED;
        };
        digest
    } else {
        hash(bytes)
    };
    measure_digest(pcr, kind, &digest, payload, flags & EXTEND_ONLY != 0)
        .err()
        .unwrap_or(Status::SUCCESS)
}

unsafe extern "efiapi" fn submit_command(
    this: *const Tcg2Protocol,
    input_len: u32,
    input: *const u8,
    output_len: u32,
    output: *mut u8,
) -> Status {
    if !valid_this(this) || input.is_null() || output.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let crb = unsafe { (*ptr::addr_of!(TCG2)).as_ref().unwrap().crb };
    crb.submit(input, input_len as usize, output, output_len as usize)
        .err()
        .unwrap_or(Status::SUCCESS)
}

unsafe extern "efiapi" fn get_active_pcr_banks(
    this: *const Tcg2Protocol,
    banks: *mut u32,
) -> Status {
    if !valid_this(this) || banks.is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe { banks.write(SHA256_BANK) };
    Status::SUCCESS
}

unsafe extern "efiapi" fn set_active_pcr_banks(this: *const Tcg2Protocol, _banks: u32) -> Status {
    if !valid_this(this) {
        Status::INVALID_PARAMETER
    } else {
        Status::UNSUPPORTED
    }
}

unsafe extern "efiapi" fn get_result_of_set_active_pcr_banks(
    this: *const Tcg2Protocol,
    present: *mut u32,
    response: *mut u32,
) -> Status {
    if !valid_this(this) || present.is_null() || response.is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe {
        present.write(0);
        response.write(0);
    }
    Status::SUCCESS
}
