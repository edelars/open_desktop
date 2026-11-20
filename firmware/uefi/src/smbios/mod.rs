//! Publish the board's SMBIOS 3.9 entry point and structures through UEFI.

use core::{ptr, slice};

use crate::{
    handoff::Handoff,
    mem::map::MemoryType,
    paging::PAGE_BYTES,
    uefi::{status::Status, system_table::SystemTable},
};

pub mod entry;
mod types;

pub const TABLE_OFFSET: usize = 0x20;
pub const TABLE_CAPACITY: usize = PAGE_BYTES as usize - TABLE_OFFSET;

/// # Safety
/// Boot Services must be active and `system` must be their live EFI system
/// table. The firmware's page allocator controls the active identity map.
pub unsafe fn install(
    handoff: &Handoff<'_>,
    processor_id: [u32; 2],
    system: *mut SystemTable,
) -> Status {
    let boot = unsafe { &*(*system).boot_services };
    let mut address = 0;
    // AllocateAnyPages, retained across ExitBootServices until OS reclamation.
    let status = unsafe { (boot.allocate_pages)(0, MemoryType::ACPI_RECLAIM, 1, &mut address) };
    if status != Status::SUCCESS {
        return status;
    }
    let base = ptr::with_exposed_provenance_mut::<u8>(address as usize);
    // SAFETY: the allocated writable page is live and mapped at this physical address.
    let page = unsafe { slice::from_raw_parts_mut(base, PAGE_BYTES as usize) };
    let status = if let Some(used) = types::build(handoff, processor_id, &mut page[TABLE_OFFSET..])
    {
        let entry = entry::build(address + TABLE_OFFSET as u64, used as u32);
        page[..entry::ENTRY_LEN].copy_from_slice(&entry);
        unsafe { (boot.install_configuration_table)(&entry::SMBIOS3_TABLE_GUID, base.cast()) }
    } else {
        Status::OUT_OF_RESOURCES
    };
    if status != Status::SUCCESS {
        unsafe { (boot.free_pages)(address, 1) };
    }
    status
}
