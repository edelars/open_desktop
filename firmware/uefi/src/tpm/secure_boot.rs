//! PFP 1.06 r52, §3.3.4.8. Measure the committed policy and enrolled image authority.

use super::{event_log::SECURE_BOOT_CAPACITY, tcg2};
use crate::{bs::memory::BootPool, uefi::status::Status, var::nvram::Nvram};
use core::{ptr, slice};
use od_uefi_security::Guid;
use od_uefi_security::protected::{
    GLOBAL_VARIABLE, IMAGE_CAPACITY, IMAGE_SECURITY_DATABASE, Image, PolicyVariable,
};
use sha2::{Digest, Sha256};

const PCR: u32 = 7;
const EV_EFI_VARIABLE_DRIVER_CONFIG: u32 = 0x8000_0001;
const EV_EFI_VARIABLE_AUTHORITY: u32 = 0x8000_00e0;
const DB: &[u8] = b"d\0b\0";
const CONFIG_COUNT: usize = 9;
// Smallest Event2 for an authority: framing, variable header/name and EFI_SIGNATURE_DATA.
const AUTHORITY_LIMIT: usize = SECURE_BOOT_CAPACITY / (50 + 32 + DB.len() + 17);
const SCRATCH_BYTES: usize = IMAGE_CAPACITY + AUTHORITY_LIMIT * 32;

struct Measurements {
    scratch: BootPool,
    config: [Option<[u8; 32]>; CONFIG_COUNT],
    authorities: usize,
    failed: Option<Status>,
    epoch: Option<u64>,
}

// Only the BSP uses this state. Measurements use the separate TCG2 state and do not call back
// into boot services or firmware clients.
static mut MEASUREMENTS: Option<Measurements> = None;

/// # Safety
/// Call once on the BSP after installing TCG2 and before starting EFI images.
pub unsafe fn initialize() -> Result<(), Status> {
    let mut scratch = unsafe { BootPool::allocate(SCRATCH_BYTES)? };
    scratch
        .zeroed_prefix(SCRATCH_BYTES)
        .ok_or(Status::OUT_OF_RESOURCES)?;
    unsafe {
        ptr::addr_of_mut!(MEASUREMENTS).write(Some(Measurements {
            scratch,
            config: [None; CONFIG_COUNT],
            authorities: 0,
            failed: None,
            epoch: None,
        }));
    }
    refresh()
}

/// Refreshes only changed configuration events; no allocation or callback occurs.
pub(crate) fn refresh() -> Result<(), Status> {
    let Some(state) = (unsafe { &mut *ptr::addr_of_mut!(MEASUREMENTS) }).as_mut() else {
        return Ok(());
    };
    let base = crate::bs::image_policy_mmio().ok_or(Status::DEVICE_ERROR)?;
    let nvram = unsafe { Nvram::at(base as usize) };
    state.ensure_config(nvram)
}

/// # Safety
/// A successful Auth2 commit just occurred on the BSP before ExitBootServices.
pub unsafe extern "efiapi" fn variables_updated() {
    // A failed measurement cannot roll back the device's committed Auth2 request.
    // Preserve its result, but prevent further image launches and boot exit.
    let _ = refresh();
}

/// Verifies a complete image, recording its enrolled authority on first use.
pub(crate) fn authorize(nvram: Nvram, bytes: &[u8]) -> Result<(), Status> {
    let Some(state) = (unsafe { &mut *ptr::addr_of_mut!(MEASUREMENTS) }).as_mut() else {
        return nvram.authorize_image(bytes, None).map(|_| ());
    };
    state.ensure_config(nvram)?;
    let scratch = unsafe { slice::from_raw_parts_mut(state.scratch.as_ptr(), SCRATCH_BYTES) };
    let (data, authorities) = scratch.split_at_mut(IMAGE_CAPACITY);
    let verified = nvram.authorize_image(bytes, Some(data));
    if nvram.policy_epoch().ok() != state.epoch {
        state.failed = Some(Status::DEVICE_ERROR);
        return Err(Status::DEVICE_ERROR);
    }
    let Some(length) = verified? else {
        return Ok(());
    };
    let payload = &data[..length];
    let header = event_header(IMAGE_SECURITY_DATABASE, DB, length);
    let parts = [&header[..], DB, payload];
    let digest = digest_parts(&parts);
    let used = state.authorities * digest.len();
    if authorities[..used]
        .chunks_exact(digest.len())
        .any(|old| old == digest)
    {
        return Ok(());
    }
    let slot = authorities
        .get_mut(used..used + digest.len())
        .ok_or(Status::VOLUME_FULL)?;
    if let Err(status) = tcg2::measure_parts(PCR, EV_EFI_VARIABLE_AUTHORITY, &digest, &parts, false)
    {
        state.failed = Some(status);
        return Err(status);
    }
    slot.copy_from_slice(&digest);
    state.authorities += 1;
    Ok(())
}

impl Measurements {
    fn ensure_config(&mut self, nvram: Nvram) -> Result<(), Status> {
        if let Some(status) = self.failed {
            return Err(status);
        }
        let result = self.measure_config(nvram);
        if let Err(status) = result {
            self.failed = Some(status);
        }
        result
    }

    fn measure_config(&mut self, nvram: Nvram) -> Result<(), Status> {
        let epoch = nvram.policy_epoch()?;
        if let Some(previous) = self.epoch {
            if epoch == previous {
                return Ok(());
            }
            // The intermediate policy of an unobserved Auth2 commit cannot be reconstructed.
            if epoch.checked_sub(previous) != Some(1) {
                return Err(Status::DEVICE_ERROR);
            }
        }
        let data = unsafe { slice::from_raw_parts_mut(self.scratch.as_ptr(), IMAGE_CAPACITY) };
        let length = nvram.read(data)?;
        if nvram.policy_epoch()? != epoch {
            return Err(Status::DEVICE_ERROR);
        }
        let image = Image::new(&data[..length]).map_err(|_| Status::DEVICE_ERROR)?;
        let mut values = [&[][..]; 6];
        for decoded in image.records() {
            let (record, _) = decoded.map_err(|_| Status::DEVICE_ERROR)?;
            if let Some(key) = record.policy_variable() {
                values[key as usize] = record.value;
            }
        }
        let [pk, kek, db, dbx, dbt, dbr] = values;
        let enabled = !pk.is_empty() && !image.flags.secure_boot_disabled;
        let mode = [u8::from(enabled)];
        let inactive = [0u8];
        let globals = GLOBAL_VARIABLE;
        let database = IMAGE_SECURITY_DATABASE;
        let variables: [(Guid, &[u8], &[u8]); CONFIG_COUNT] = [
            (globals, b"S\0e\0c\0u\0r\0e\0B\0o\0o\0t\0", &mode),
            (globals, b"P\0K\0", pk),
            (globals, b"K\0E\0K\0", kek),
            (database, DB, db),
            (database, b"d\0b\0x\0", dbx),
            (database, b"d\0b\0t\0", dbt),
            (database, b"d\0b\0r\0", dbr),
            (globals, b"A\0u\0d\0i\0t\0M\0o\0d\0e\0", &inactive),
            (globals, b"D\0e\0p\0l\0o\0y\0e\0d\0M\0o\0d\0e\0", &inactive),
        ];
        for (index, (guid, name, value)) in variables.into_iter().enumerate() {
            let last = &mut self.config[index];
            if matches!(index, 5 | 6) && last.is_none() && value.is_empty() {
                continue;
            }
            let header = event_header(guid, name, value.len());
            let parts = [&header[..], name, value];
            let digest = digest_parts(&parts);
            if *last != Some(digest) {
                tcg2::measure_parts(PCR, EV_EFI_VARIABLE_DRIVER_CONFIG, &digest, &parts, false)?;
                *last = Some(digest);
            }
        }
        self.epoch = Some(epoch);
        Ok(())
    }
}

fn event_header(guid: Guid, name: &[u8], value_len: usize) -> [u8; 32] {
    let mut header = [0; 32];
    header[..4].copy_from_slice(&guid.data1.to_le_bytes());
    header[4..6].copy_from_slice(&guid.data2.to_le_bytes());
    header[6..8].copy_from_slice(&guid.data3.to_le_bytes());
    header[8..16].copy_from_slice(&guid.data4);
    header[16..24].copy_from_slice(&((name.len() / 2) as u64).to_le_bytes());
    header[24..].copy_from_slice(&(value_len as u64).to_le_bytes());
    header
}

fn digest_parts(parts: &[&[u8]]) -> [u8; 32] {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update(part);
    }
    digest.finalize().into()
}
