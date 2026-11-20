//! Publish the shared ACPI serializer's payload into the firmware board reservations.

use core::ptr;

use crate::{
    handoff::Handoff,
    uefi::{guid::Guid, status::Status, system_table::SystemTable},
};

use super::blob::{
    ACPI_RECLAIM_END, ACPI_REGION_BASE, ACPI_REGION_END, BUILD_CAPACITY, FACS_ADDRESS, absent,
    build_segments,
};
use super::manifest::device_type;

pub const EFI_ACPI_20_TABLE_GUID: Guid = Guid {
    data1: 0x8868_e871,
    data2: 0xe4f1,
    data3: 0x11d3,
    data4: [0xbc, 0x22, 0x00, 0x80, 0xc7, 0x3c, 0x88, 0x81],
};

/// # Safety
/// Boot Services must be active, and `system` must point to their live table.
/// `handoff` was parsed for boot and its ACPI reclaim/NVS pages, when present,
/// are identity-mapped and writable. Only the BSP invokes this installer.
pub unsafe fn install(
    handoff: &Handoff<'_>,
    system: *mut SystemTable,
    tpm_available: bool,
) -> Status {
    match handoff.acpi_fixed_hardware() {
        Ok(None) => return Status::SUCCESS,
        Ok(Some(_)) => {}
        Err(_) => return Status::LOAD_ERROR,
    }
    let (mut entries, count) = handoff.device_entries();
    if !tpm_available && handoff.tpm_crb_base().is_some() {
        for entry in &mut entries[..count] {
            if entry.device_type == device_type::TPM_CRB {
                *entry = absent(device_type::TPM_CRB);
                break;
            }
        }
    }
    let mut blob = [0; BUILD_CAPACITY];
    let Ok(segments) = build_segments::<false>(&entries[..count], &mut blob) else {
        return Status::LOAD_ERROR;
    };
    let payload = &blob[segments.payload..segments.facs];
    let facs = &blob[segments.facs..segments.end];
    if payload.len() as u64 > ACPI_RECLAIM_END - ACPI_REGION_BASE
        || facs.len() as u64 > ACPI_REGION_END - FACS_ADDRESS
    {
        return Status::OUT_OF_RESOURCES;
    }

    // Both copies use the validated regions. The EFI table becomes visible
    // only after the RSDP, tables, and FACS bytes are in place.
    unsafe {
        let rsdp = ptr::with_exposed_provenance_mut(ACPI_REGION_BASE as usize);
        crate::bs::misc::copy_mem(rsdp, payload.as_ptr().cast(), payload.len());
        crate::bs::misc::copy_mem(
            ptr::with_exposed_provenance_mut(FACS_ADDRESS as usize),
            facs.as_ptr().cast(),
            facs.len(),
        );
        let boot = (*system).boot_services;
        ((*boot).install_configuration_table)(&EFI_ACPI_20_TABLE_GUID, rsdp)
    }
}
