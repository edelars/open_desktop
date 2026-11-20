//! UEFI TCG2 protocol for the fixed CRB and the board's active PCR banks.

use core::{ffi::c_void, ptr, slice};

use crate::bs;
use crate::mem::map::MemoryType;
use crate::pe::PeImage;
use crate::uefi::{guid::Guid, status::Status};

use super::banks::{
    EventDigests, MAX_TAGGED_BYTES, PcrBanks, SHA256, SHA256_BANK, SHA384, bank_bit,
};
use super::crb_driver::Crb;
use super::event_log::{CAPACITY, EV_NO_ACTION, EventLog, SECURE_BOOT_CAPACITY};

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
    supported_banks: PcrBanks,
    active_banks: PcrBanks,
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

pub(super) fn active_banks() -> Option<PcrBanks> {
    unsafe {
        (*ptr::addr_of!(TCG2))
            .as_ref()
            .map(|state| state.active_banks)
    }
}

pub struct InitializedTpm {
    crb: Crb,
    manufacturer: u32,
    supported_banks: PcrBanks,
    active_banks: PcrBanks,
}

/// Initializes the handoff-validated CRB. A TPM in failure mode is unavailable.
///
/// # Safety
/// Only the BSP uses the aperture, which is mapped as device memory.
pub unsafe fn initialize(base: u64) -> Result<Option<InitializedTpm>, Status> {
    let crb = unsafe { Crb::open(base)? };
    initialize_tpm(crb)
}

/// Publishes TCG2 for a successfully initialized TPM.
///
/// # Safety
/// Boot Services are initialized on the BSP.
pub unsafe fn install(tpm: InitializedTpm, secure_boot_capable: bool) -> Result<(), Status> {
    if active() {
        return Err(Status::ALREADY_STARTED);
    }
    let InitializedTpm {
        crb,
        manufacturer,
        supported_banks,
        active_banks,
    } = tpm;
    let capacity = if secure_boot_capable {
        SECURE_BOOT_CAPACITY
    } else {
        CAPACITY
    };
    let main_log = allocate_log(MemoryType::BOOT_SERVICES_DATA, capacity)?;
    let final_events = match allocate_log(MemoryType::RUNTIME_SERVICES_DATA, capacity) {
        Ok(log) => log,
        Err(status) => {
            unsafe { bs::memory::free_pool(main_log.cast()) };
            return Err(status);
        }
    };
    let log = unsafe { EventLog::new(main_log, final_events, capacity, active_banks) };
    let status = unsafe {
        bs::config_table::install_configuration_table(&FINAL_EVENTS_GUID, final_events.cast())
    };
    if status == Status::SUCCESS {
        unsafe {
            ptr::addr_of_mut!(TCG2).write(Some(Tcg2 {
                crb,
                manufacturer,
                supported_banks,
                active_banks,
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

fn allocate_log(ty: MemoryType, capacity: usize) -> Result<*mut u8, Status> {
    let mut buffer: *mut c_void = ptr::null_mut();
    let status = unsafe { bs::memory::allocate_pool(ty, capacity, &mut buffer) };
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

fn startup_succeeded(response: &[u8]) -> Result<bool, Status> {
    match response {
        [0x80, 1, 0, 0, 0, 10, 0, 0, 0, 0] => Ok(true),
        [0x80, 1, 0, 0, 0, 10, 0, 0, 1, 1] => Ok(false), // TPM_RC_FAILURE
        _ => Err(Status::DEVICE_ERROR),
    }
}

#[cfg(test)]
#[test]
fn startup_distinguishes_tpm_failure_from_invalid_responses() {
    let mut response = [0x80, 1, 0, 0, 0, 10, 0, 0, 0, 0];
    assert_eq!(startup_succeeded(&response), Ok(true));
    response[8..].copy_from_slice(&[1, 1]);
    assert_eq!(startup_succeeded(&response), Ok(false));
    assert_eq!(startup_succeeded(&response[..9]), Err(Status::DEVICE_ERROR));
    response[1] = 2;
    assert_eq!(startup_succeeded(&response), Err(Status::DEVICE_ERROR));
    response[1] = 1;
    response[9] = 0;
    assert_eq!(startup_succeeded(&response), Err(Status::DEVICE_ERROR));
}

#[inline(never)]
fn start_tpm(crb: Crb) -> Result<bool, Status> {
    const STARTUP: [u8; 12] = [0x80, 1, 0, 0, 0, 12, 0, 0, 1, 0x44, 0, 0];
    let mut response = [0; 10];
    let size = crb.submit(
        STARTUP.as_ptr(),
        STARTUP.len(),
        response.as_mut_ptr(),
        response.len(),
    )?;
    startup_succeeded(&response[..size])
}

fn parse_bank_allocation(response: &[u8]) -> Result<u32, Status> {
    if response.get(10..15) != Some(&[0, 0, 0, 0, 5]) {
        return Err(Status::DEVICE_ERROR);
    }
    let Some(count) = response.get(15..19) else {
        return Err(Status::DEVICE_ERROR);
    };
    let count = u32::from_be_bytes(count.try_into().unwrap());
    if !(1..=2).contains(&count) || response.len() != 19 + count as usize * 6 {
        return Err(Status::UNSUPPORTED);
    }
    let mut seen = 0;
    let mut active = 0;
    for selection in response[19..].chunks_exact(6) {
        if selection[0] != 0 || selection[2] != 3 {
            return Err(Status::UNSUPPORTED);
        }
        let bit = bank_bit(selection[1]).ok_or(Status::UNSUPPORTED)?;
        if seen & bit != 0 {
            return Err(Status::UNSUPPORTED);
        }
        seen |= bit;
        match &selection[3..] {
            [0, 0, 0] => {}
            [0xff, 0xff, 0xff] => active |= bit,
            _ => return Err(Status::UNSUPPORTED),
        }
    }
    if active == 0 {
        return Err(Status::UNSUPPORTED);
    }
    Ok(active)
}

fn parse_supported_banks(response: &[u8], first: u8) -> Result<(u32, bool), Status> {
    let Some(header) = response.get(10..19) else {
        return Err(Status::DEVICE_ERROR);
    };
    let count = u32::from_be_bytes(header[5..9].try_into().unwrap());
    let more = header[0] == 1;
    if header[0] > 1
        || header[1..5] != [0; 4]
        || count > 2
        || (more && count == 0)
        || response.len() != 19 + count as usize * 6
    {
        return Err(Status::DEVICE_ERROR);
    }
    let mut supported = 0;
    for entry in response[19..].chunks_exact(6) {
        let algorithm = entry[1];
        if entry[0] != 0 || algorithm > SHA384 {
            return Ok((supported, false));
        }
        let bit = bank_bit(algorithm).ok_or(Status::DEVICE_ERROR)?;
        if algorithm < first || bit <= supported {
            return Err(Status::DEVICE_ERROR);
        }
        supported |= bit;
    }
    Ok((supported, more && supported == SHA256_BANK))
}

#[cfg(test)]
#[test]
fn capabilities_distinguish_available_and_allocated_pcr_banks() {
    use super::banks::{SHA256_BANK, SHA384_BANK};

    let mut pcrs = [0; 31];
    pcrs[14] = 5;
    pcrs[18] = 2;
    pcrs[19..].copy_from_slice(&[0, 11, 3, 0xff, 0xff, 0xff, 0, 12, 3, 0xff, 0xff, 0xff]);
    assert_eq!(parse_bank_allocation(&pcrs), Ok(SHA256_BANK | SHA384_BANK));
    pcrs[22..25].fill(0);
    assert_eq!(parse_bank_allocation(&pcrs), Ok(SHA384_BANK));
    pcrs[22] = 1;
    assert_eq!(parse_bank_allocation(&pcrs), Err(Status::UNSUPPORTED));
    pcrs[22..25].fill(0xff);
    pcrs[26] = 11;
    assert_eq!(parse_bank_allocation(&pcrs), Err(Status::UNSUPPORTED));
    pcrs[18] = 1;
    assert_eq!(parse_bank_allocation(&pcrs[..25]), Ok(SHA256_BANK));
    assert_eq!(parse_bank_allocation(&pcrs[..24]), Err(Status::UNSUPPORTED));

    let mut algs = [0; 31];
    algs[10] = 1;
    algs[18] = 2;
    algs[20] = 11;
    algs[26] = 12;
    assert_eq!(
        parse_supported_banks(&algs, SHA256),
        Ok((SHA256_BANK | SHA384_BANK, false))
    );
    algs[26] = 16; // The next implemented algorithm may be NULL.
    assert_eq!(
        parse_supported_banks(&algs, SHA256),
        Ok((SHA256_BANK, false))
    );
    assert_eq!(
        parse_supported_banks(&algs[..30], SHA256),
        Err(Status::DEVICE_ERROR)
    );
    algs[18] = 1;
    assert_eq!(
        parse_supported_banks(&algs[..25], SHA256),
        Ok((SHA256_BANK, true))
    );
    algs[20] = 12;
    assert_eq!(
        parse_supported_banks(&algs[..25], SHA384),
        Ok((SHA384_BANK, false))
    );
}

fn initialize_tpm(crb: Crb) -> Result<Option<InitializedTpm>, Status> {
    if !start_tpm(crb)? {
        return Ok(None);
    }
    const SELF_TEST: [u8; 11] = [0x80, 1, 0, 0, 0, 11, 0, 0, 1, 0x43, 1];
    const GET_PCRS: [u8; 22] = [
        0x80, 1, 0, 0, 0, 22, 0, 0, 1, 0x7a, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 1,
    ];
    // SHA-256 and SHA-384 are consecutive algorithm IDs.
    const GET_HASH_ALGS: [u8; 22] = [
        0x80, 1, 0, 0, 0, 22, 0, 0, 1, 0x7a, 0, 0, 0, 0, 0, 0, 0, 0x0b, 0, 0, 0, 2,
    ];
    const GET_MANUFACTURER: [u8; 22] = [
        0x80, 1, 0, 0, 0, 22, 0, 0, 1, 0x7a, 0, 0, 0, 6, 0, 0, 1, 5, 0, 0, 0, 1,
    ];
    let mut response = [0; 32];
    checked_command(crb, &SELF_TEST, &mut response)?;
    let size = checked_command(crb, &GET_PCRS, &mut response)?;
    let active_banks = parse_bank_allocation(&response[..size])?;
    let size = checked_command(crb, &GET_HASH_ALGS, &mut response)?;
    let (mut supported_banks, query_sha384) = parse_supported_banks(&response[..size], SHA256)?;
    if query_sha384 {
        let mut next = GET_HASH_ALGS;
        next[17] = SHA384;
        let size = checked_command(crb, &next, &mut response)?;
        supported_banks |= parse_supported_banks(&response[..size], SHA384)?.0;
    }
    if active_banks & !supported_banks != 0 {
        return Err(Status::UNSUPPORTED);
    }
    let size = checked_command(crb, &GET_MANUFACTURER, &mut response)?;
    let capability = u32::from_be_bytes(response[11..15].try_into().unwrap());
    let property_count = u32::from_be_bytes(response[15..19].try_into().unwrap());
    let property = u32::from_be_bytes(response[19..23].try_into().unwrap());
    if size != 27 || capability != 6 || property_count != 1 || property != 0x0000_0105 {
        return Err(Status::DEVICE_ERROR);
    }
    Ok(Some(InitializedTpm {
        crb,
        manufacturer: u32::from_be_bytes(response[23..27].try_into().unwrap()),
        supported_banks: PcrBanks::from_bitmap(supported_banks).ok_or(Status::UNSUPPORTED)?,
        active_banks: PcrBanks::from_bitmap(active_banks).ok_or(Status::UNSUPPORTED)?,
    }))
}

/// Measures existing immutable bytes on the BSP when a TPM is configured.
pub fn measure_bytes(pcr: u32, kind: u32, data: &[u8], event: &[u8]) -> Result<(), Status> {
    let Some(banks) = active_banks() else {
        return Ok(());
    };
    let digests = EventDigests::hash_parts(banks, &[data], None);
    measure_parts(pcr, kind, &digests, &[event], false)
}

pub(super) fn measure_parts(
    pcr: u32,
    kind: u32,
    digests: &EventDigests,
    event: &[&[u8]],
    extend_only: bool,
) -> Result<(), Status> {
    let Some(state) = (unsafe { &mut *ptr::addr_of_mut!(TCG2) }).as_mut() else {
        return Ok(());
    };
    if pcr >= 24 {
        return Err(Status::INVALID_PARAMETER);
    }
    let zero;
    let digests = if kind == EV_NO_ACTION {
        zero = EventDigests::zero(state.active_banks);
        &zero
    } else {
        const PCR_EXTEND: [u8; 31] = [
            0x80, 2, 0, 0, 0, 0, 0, 0, 1, 0x82, 0, 0, 0, 0, 0, 0, 0, 9, 0x40, 0, 0, 9, 0, 0, 0, 0,
            0, 0, 0, 0, 0,
        ];
        let mut command = [0; 31 + MAX_TAGGED_BYTES];
        command[..31].copy_from_slice(&PCR_EXTEND);
        command[10..14].copy_from_slice(&pcr.to_be_bytes());
        command[27..31].copy_from_slice(&digests.count().to_be_bytes());
        let length = 31 + digests.write_tagged::<true>(&mut command[31..]);
        command[2..6].copy_from_slice(&(length as u32).to_be_bytes());
        checked_command(state.crb, &command[..length], &mut [0; 24])?;
        digests
    };
    if !extend_only {
        state.log.append(pcr, kind, digests, event);
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
    let (manufacturer, capacity, supported_banks, active_banks) = {
        let state = unsafe { (*ptr::addr_of!(TCG2)).as_ref().unwrap() };
        (
            state.manufacturer,
            state.crb.capacity(),
            state.supported_banks,
            state.active_banks,
        )
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
    bytes[8..12].copy_from_slice(&(supported_banks as u32).to_le_bytes());
    bytes[12..16].copy_from_slice(&TCG2_FORMAT.to_le_bytes());
    bytes[16] = 1;
    bytes[18..20].copy_from_slice(&capacity.to_le_bytes());
    bytes[20..22].copy_from_slice(&capacity.to_le_bytes());
    bytes[24..28].copy_from_slice(&manufacturer.to_le_bytes());
    bytes[28..32].copy_from_slice(&(supported_banks.algorithms().len() as u32).to_le_bytes());
    bytes[32..36].copy_from_slice(&(active_banks as u32).to_le_bytes());
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
    let banks = active_banks().expect("valid TCG2 protocol");
    let digests = if flags & PE_COFF_IMAGE != 0 {
        let Ok(pe) = PeImage::parse(bytes) else {
            return Status::UNSUPPORTED;
        };
        let Ok(digests) = EventDigests::hash_image(banks, &pe) else {
            return Status::UNSUPPORTED;
        };
        digests
    } else {
        EventDigests::hash_parts(banks, &[bytes], None)
    };
    measure_parts(pcr, kind, &digests, &[payload], flags & EXTEND_ONLY != 0)
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
    unsafe { banks.write(active_banks().expect("valid TCG2 protocol") as u32) };
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
