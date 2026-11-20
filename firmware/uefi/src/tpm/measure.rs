//! PC Client PFP 1.06 event payloads for this board and its boot path.

use super::tcg2;
#[cfg(target_os = "none")]
use crate::bs::memory::BootPool;
use crate::handoff::Handoff;
use crate::pe::{DigestAlgorithm, PeImage, Subsystem, authenticode_digest};
use crate::uefi::status::Status;
use core::{ptr, slice};

const EV_S_CRTM_VERSION: u32 = 8;
const EV_SEPARATOR: u32 = 4;
#[cfg(target_os = "none")]
const EV_ACTION: u32 = 5;
const EV_EFI_BOOT_SERVICES_APPLICATION: u32 = 0x8000_0003;
const EV_EFI_BOOT_SERVICES_DRIVER: u32 = 0x8000_0004;
const EV_EFI_RUNTIME_SERVICES_DRIVER: u32 = 0x8000_0005;
const EV_EFI_ACTION: u32 = 0x8000_0007;
const EV_EFI_PLATFORM_FIRMWARE_BLOB2: u32 = 0x8000_000a;
const EV_EFI_HANDOFF_TABLES2: u32 = 0x8000_000b;
const EV_EFI_VARIABLE_BOOT2: u32 = 0x8000_000c;
const EV_EFI_GPT_EVENT2: u32 = 0x8000_000d;

const ROM_BASE: u64 = 0xffc0_0000;
const ROM_DESCRIPTION: &[u8] = b"Open Desktop firmware ROM";
const HANDOFF_DESCRIPTION: &[u8] = b"Open Desktop board handoff";
// Open Desktop ODHANDOF table GUID, in EFI_GUID wire order.
const HANDOFF_GUID: [u8; 16] = [
    0xc2, 0x03, 0x92, 0xca, 0xa5, 0xd9, 0x6d, 0x57, 0xab, 0x50, 0x99, 0x67, 0xdc, 0x13, 0x92, 0x33,
];
const GLOBAL_VARIABLE_GUID: [u8; 16] = [
    0x61, 0xdf, 0xe4, 0x8b, 0xca, 0x93, 0xd2, 0x11, 0xaa, 0x0d, 0x00, 0xe0, 0x98, 0x03, 0x2b, 0x8c,
];

/// # Safety
/// The fixed, read-only firmware ROM window is mapped in the validated boot address space.
pub unsafe fn platform(handoff: &Handoff<'_>) -> Result<(), Status> {
    if !tcg2::active() {
        return Ok(());
    }
    const VERSION: &[u8] = b"O\0D\0F\0W\0I\0M\0G\x001\0\0\0";
    tcg2::measure_bytes(0, EV_S_CRTM_VERSION, VERSION, VERSION)?;

    let rom_size = crate::image::IMAGE_SIZE;
    let rom = unsafe { slice::from_raw_parts(ROM_BASE as *const u8, rom_size) };
    let mut blob = [0; 1 + ROM_DESCRIPTION.len() + 16];
    const RANGE_AT: usize = 1 + ROM_DESCRIPTION.len();
    blob[0] = ROM_DESCRIPTION.len() as u8;
    blob[1..RANGE_AT].copy_from_slice(ROM_DESCRIPTION);
    blob[RANGE_AT..RANGE_AT + 8].copy_from_slice(&ROM_BASE.to_le_bytes());
    blob[RANGE_AT + 8..].copy_from_slice(&(rom_size as u64).to_le_bytes());
    tcg2::measure_bytes(0, EV_EFI_PLATFORM_FIRMWARE_BLOB2, rom, &blob)?;

    const POINTERS_AT: usize = 1 + HANDOFF_DESCRIPTION.len();
    let mut pointers = [0; POINTERS_AT + 8 + 16 + 8];
    pointers[0] = HANDOFF_DESCRIPTION.len() as u8;
    pointers[1..POINTERS_AT].copy_from_slice(HANDOFF_DESCRIPTION);
    pointers[POINTERS_AT..POINTERS_AT + 8].copy_from_slice(&1u64.to_le_bytes());
    pointers[POINTERS_AT + 8..POINTERS_AT + 24].copy_from_slice(&HANDOFF_GUID);
    pointers[POINTERS_AT + 24..].copy_from_slice(&(handoff.bytes().as_ptr() as u64).to_le_bytes());
    tcg2::measure_bytes(1, EV_EFI_HANDOFF_TABLES2, handoff.bytes(), &pointers)
}

#[cfg(target_os = "none")]
fn separate() -> Result<(), Status> {
    let separated = unsafe {
        (*ptr::addr_of!(tcg2::TCG2))
            .as_ref()
            .is_none_or(|state| state.separated)
    };
    if separated {
        return Ok(());
    }
    let data = [0; 4];
    let digest = tcg2::hash(&data);
    for pcr in 0..8 {
        tcg2::measure_digest(pcr, EV_SEPARATOR, &digest, &data, false)?;
    }
    unsafe {
        (*ptr::addr_of_mut!(tcg2::TCG2)).as_mut().unwrap().separated = true;
    }
    Ok(())
}

/// # Safety
/// Call on the boot BSP without a Boot Services state borrow.
#[cfg(target_os = "none")]
pub unsafe fn ready_to_boot() -> Result<(), Status> {
    super::secure_boot::refresh()?;
    separate()?;
    let status = unsafe { crate::bs::event::signal_ready_to_boot() };
    if status != Status::SUCCESS {
        return Err(status);
    }
    let returned = b"Returned Ready to Boot";
    tcg2::measure_bytes(4, EV_ACTION, returned, returned)?;
    let gpt = unsafe {
        (*ptr::addr_of_mut!(tcg2::TCG2))
            .as_mut()
            .and_then(|state| state.pending_gpt.take())
    };
    if let Some(buffer) = gpt {
        let data = unsafe { slice::from_raw_parts(buffer.as_ptr(), buffer.len()) };
        tcg2::measure_bytes(5, EV_EFI_GPT_EVENT2, data, data)?;
    }
    Ok(())
}

pub fn action(pcr: u32, action: &[u8]) -> Result<(), Status> {
    tcg2::measure_bytes(pcr, EV_EFI_ACTION, action, action)
}

#[cfg(target_os = "none")]
pub(crate) fn booting_to(name: &[u16; 9]) -> Result<(), Status> {
    if !tcg2::active() {
        return Ok(());
    }
    let mut data = *b"Booting to Boot0000 Option";
    for (byte, unit) in data[11..19].iter_mut().zip(name) {
        *byte = *unit as u8;
    }
    action(4, &data)
}

#[cfg(target_os = "none")]
pub(crate) fn boot_variable(name: &[u16], value: &[u8]) -> Result<(), Status> {
    let before_separator = unsafe {
        (*ptr::addr_of!(tcg2::TCG2))
            .as_ref()
            .is_some_and(|state| !state.separated)
    };
    if !before_separator {
        return Ok(());
    }
    let name = name.strip_suffix(&[0]).ok_or(Status::INVALID_PARAMETER)?;
    let mut header = [0u8; 32];
    header[..16].copy_from_slice(&GLOBAL_VARIABLE_GUID);
    header[16..24].copy_from_slice(&(name.len() as u64).to_le_bytes());
    header[24..32].copy_from_slice(&(value.len() as u64).to_le_bytes());
    // This x64 firmware stores CHAR16 little-endian, as UEFI_VARIABLE_DATA expects.
    let name = unsafe { slice::from_raw_parts(name.as_ptr().cast::<u8>(), name.len() * 2) };
    let parts = [&header[..], name, value];
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part);
    }
    tcg2::measure_parts(
        1,
        EV_EFI_VARIABLE_BOOT2,
        &hash.finalize().into(),
        &parts,
        false,
    )
}

pub(crate) fn image(
    pe: &PeImage<'_>,
    base: u64,
    size: u64,
    path: &[u8],
    embedded: bool,
) -> Result<(), Status> {
    if !tcg2::active() {
        return Ok(());
    }
    let boot_attempt = unsafe {
        (*ptr::addr_of!(tcg2::TCG2))
            .as_ref()
            .is_some_and(|state| state.separated)
    };
    // Embedded pre-boot drivers are covered by the PCR0 digest of the complete ROM.
    if embedded && !boot_attempt && pe.subsystem() != Subsystem::Application {
        return Ok(());
    }
    let digest =
        authenticode_digest(pe, DigestAlgorithm::Sha256).map_err(|_| Status::LOAD_ERROR)?;
    let (default_pcr, kind) = match pe.subsystem() {
        Subsystem::Application => (4, EV_EFI_BOOT_SERVICES_APPLICATION),
        Subsystem::BootServiceDriver => (2, EV_EFI_BOOT_SERVICES_DRIVER),
        Subsystem::RuntimeDriver => (2, EV_EFI_RUNTIME_SERVICES_DRIVER),
    };
    let pcr = if boot_attempt { 4 } else { default_pcr };
    // LoadImage owns the BSP map mutation guard, so record the existing path
    // directly with the fixed-size metadata, without a new pool allocation.
    let mut out = [0; 32];
    out[..8].copy_from_slice(&base.to_le_bytes());
    out[8..16].copy_from_slice(&size.to_le_bytes());
    out[16..24].copy_from_slice(&pe.image_base().to_le_bytes());
    out[24..32].copy_from_slice(&(path.len() as u64).to_le_bytes());
    tcg2::measure_parts(pcr, kind, &digest, &[&out, path], false)
}

#[cfg(target_os = "none")]
pub(crate) fn gpt(header: &[u8], entries: &[u8], stride: usize) -> Result<(), Status> {
    if !tcg2::active() {
        return Ok(());
    }
    let count = entries.len() / stride;
    let size = (count * GPT_ENTRY_LEN)
        .checked_add(GPT_ENTRIES_AT)
        .ok_or(Status::BAD_BUFFER_SIZE)?;
    let payload = unsafe { BootPool::allocate(size)? };
    unsafe { gpt_payload(payload.as_ptr(), header, entries, stride) };
    let previous = unsafe {
        (*ptr::addr_of_mut!(tcg2::TCG2))
            .as_mut()
            .unwrap()
            .pending_gpt
            .replace(payload)
    };
    drop(previous);
    Ok(())
}

#[cfg(any(test, target_os = "none"))]
const GPT_HEADER_LEN: usize = 92;
#[cfg(any(test, target_os = "none"))]
const GPT_ENTRIES_AT: usize = GPT_HEADER_LEN + 8;
#[cfg(any(test, target_os = "none"))]
const GPT_ENTRY_LEN: usize = 128;

#[cfg(any(test, target_os = "none"))]
unsafe fn gpt_payload(out: *mut u8, header: &[u8], entries: &[u8], stride: usize) {
    // The validated GPT may store 128*2^n-byte slots. The PFP event contains
    // fixed UEFI_PARTITION_ENTRY records, one for each slot.
    unsafe {
        ptr::copy_nonoverlapping(header.as_ptr(), out, GPT_HEADER_LEN);
        out.add(56).write_bytes(0, 16); // DiskGUID
        out.add(GPT_HEADER_LEN)
            .cast::<u64>()
            .write_unaligned(((entries.len() / stride) as u64).to_le());
        for (index, entry) in entries.chunks_exact(stride).enumerate() {
            let out = out.add(GPT_ENTRIES_AT + index * GPT_ENTRY_LEN);
            ptr::copy_nonoverlapping(entry.as_ptr(), out, GPT_ENTRY_LEN);
            out.add(16).write_bytes(0, 16); // UniquePartitionGUID
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpt_event_keeps_sparse_slots_and_discards_stride_padding_and_guids() {
        let mut header = [0u8; GPT_HEADER_LEN];
        header[..8].copy_from_slice(b"EFI PART");
        header[56..72].fill(0x3a);
        header[80..84].copy_from_slice(&3u32.to_le_bytes());
        header[84..88].copy_from_slice(&256u32.to_le_bytes());
        let mut entries = [0u8; 3 * 256];
        entries[0] = 1;
        entries[16..32].fill(0xd2);
        entries[40] = 0x80;
        entries[128..256].fill(0xee);
        entries[512] = 2;
        let mut out = [0xff; GPT_ENTRIES_AT + 3 * GPT_ENTRY_LEN];
        unsafe { gpt_payload(out.as_mut_ptr(), &header, &entries, 256) };
        assert_eq!(&out[..8], b"EFI PART");
        assert_eq!(out[56..72], [0; 16]);
        assert_eq!(
            out[80..GPT_ENTRIES_AT],
            [3, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0]
        );
        let [first, second, third] = out[GPT_ENTRIES_AT..].as_chunks::<GPT_ENTRY_LEN>().0 else {
            panic!("three slots");
        };
        assert_eq!(first[0], 1);
        assert_eq!(first[16..32], [0; 16]);
        assert_eq!(first[40], 0x80);
        assert_eq!(*second, [0; GPT_ENTRY_LEN]);
        assert_eq!(third[0], 2);
        assert!(!out.contains(&0xee));
    }
}
