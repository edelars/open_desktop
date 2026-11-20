//! Freestanding binary that carries the stage-4 ACPI table blob (S4-31d).
//!
//! It is data only: nothing here runs in the guest (`_start` just halts, it exists so the link
//! has an entry). The whole content is the `.acpi_blob` section, evaluated by the compiler from
//! the `const fn` serializers (`od_firmware::acpi::blob`). `tools/build/
//! build_stage4_acpi_blob.py` extracts the section; it does not serialize anything.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[cfg(all(feature = "linux-nvme-acpi", not(feature = "handoff-acpi")))]
use od_firmware::acpi::blob::{
    LINUX_NVME_BLOB_LEN as BLOB_LEN, linux_nvme_blob_bytes as blob_bytes,
};
#[cfg(not(any(feature = "linux-nvme-acpi", feature = "handoff-acpi")))]
use od_firmware::acpi::blob::{STAGE4_BLOB_LEN as BLOB_LEN, stage4_blob_bytes as blob_bytes};

#[cfg(feature = "handoff-acpi")]
const HANDOFF_BLOB: ([u8; od_firmware::acpi::blob::BUILD_CAPACITY], usize) = {
    use od_firmware::{acpi::blob::build_blob, handoff::Handoff};
    let handoff = include_bytes!(env!("OD_FW_ACPI_HANDOFF"));
    let (entries, count) = match Handoff::acpi_blob_devices(handoff) {
        Ok(devices) => devices,
        Err(_) => panic!("invalid ACPI handoff block"),
    };
    let mut bytes = [0; od_firmware::acpi::blob::BUILD_CAPACITY];
    let len = match build_blob(entries.split_at(count).0, &mut bytes) {
        Ok(len) => len,
        Err(_) => panic!("cannot build ACPI tables for this handoff block"),
    };
    (bytes, len)
};

#[cfg(feature = "handoff-acpi")]
const BLOB_LEN: usize = HANDOFF_BLOB.1;

#[cfg(feature = "handoff-acpi")]
const fn blob_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    bytes.copy_from_slice(HANDOFF_BLOB.0.split_at(BLOB_LEN).0);
    bytes
}

/// The blob. `no_mangle` + `used` keep it in the object file whatever the optimizer sees.
#[unsafe(no_mangle)]
#[used]
#[unsafe(link_section = ".acpi_blob")]
pub static ACPI_BLOB: [u8; BLOB_LEN] = blob_bytes::<BLOB_LEN>();

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
