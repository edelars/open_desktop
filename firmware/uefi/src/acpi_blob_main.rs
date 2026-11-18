//! Freestanding binary that carries the stage-4 ACPI table blob (S4-31d).
//!
//! It is data only: nothing here runs in the guest (`_start` just halts, it exists so the link
//! has an entry). The whole content is the `.acpi_blob` section, evaluated by the compiler from
//! the `const fn` serializers (`od_firmware::acpi::blob`). `tools/build/
//! build_stage4_acpi_blob.py` extracts the section; it does not serialize anything.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[cfg(feature = "linux-nvme-acpi")]
use od_firmware::acpi::blob::{
    LINUX_NVME_BLOB_LEN as BLOB_LEN, linux_nvme_blob_bytes as blob_bytes,
};
#[cfg(not(feature = "linux-nvme-acpi"))]
use od_firmware::acpi::blob::{STAGE4_BLOB_LEN as BLOB_LEN, stage4_blob_bytes as blob_bytes};

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
