//! GUID-keyed Configuration Table publication (UEFI 2.11 §7.5.6).
use super::{event, memory::allocate_pool_in, state};
use crate::mem::{map::MemoryType, pages::PageAllocator};
use crate::uefi::{guid::Guid, status::Status, system_table::ConfigurationTable};
use core::{ffi::c_void, ptr, ptr::NonNull};

const MAX_CONFIGURATION_TABLES: usize = 32;
type Entries = [ConfigurationTable; MAX_CONFIGURATION_TABLES];

const EMPTY_ENTRY: ConfigurationTable = ConfigurationTable {
    vendor_guid: Guid {
        data1: 0,
        data2: 0,
        data3: 0,
        data4: [0; 8],
    },
    vendor_table: ptr::null_mut(),
};

pub(crate) struct ConfigurationTables {
    storage: Option<NonNull<Entries>>,
    count: usize,
}

impl ConfigurationTables {
    pub(crate) const fn new() -> Self {
        Self {
            storage: None,
            count: 0,
        }
    }

    // Caller owns the allocator and table registry through the BSP guard.
    unsafe fn update(
        &mut self,
        allocator: &mut PageAllocator<'_, '_>,
        guid: Guid,
        table: *mut c_void,
    ) -> Result<(), Status> {
        let mut entries = match self.storage {
            Some(entries) => entries,
            None if table.is_null() => return Err(Status::NOT_FOUND),
            None => {
                // Keep the array allocated after removing its last entry.
                // A later publication can reuse it without changing MapKey.
                let buffer = unsafe {
                    allocate_pool_in(
                        allocator,
                        MemoryType::RUNTIME_SERVICES_DATA,
                        size_of::<Entries>(),
                    )?
                };
                // SAFETY: the success payload is non-null, aligned, writable
                // and exclusively owned by the configuration table registry.
                let entries = unsafe { NonNull::new_unchecked(buffer.cast::<Entries>()) };
                unsafe {
                    entries
                        .as_ptr()
                        .write([EMPTY_ENTRY; MAX_CONFIGURATION_TABLES])
                };
                self.storage = Some(entries);
                entries
            }
        };
        // SAFETY: the BSP guard owns the initialized array. Callers do not
        // retain references to individual entries across service calls.
        update_entries(unsafe { entries.as_mut() }, &mut self.count, guid, table)
    }

    fn published(&self) -> (*mut ConfigurationTable, usize) {
        let address = if self.count == 0 {
            ptr::null_mut()
        } else {
            self.storage
                .map_or(ptr::null_mut(), |storage| storage.as_ptr().cast())
        };
        (address, self.count)
    }
}

fn update_entries(
    entries: &mut Entries,
    count: &mut usize,
    guid: Guid,
    table: *mut c_void,
) -> Result<(), Status> {
    let existing = entries[..*count]
        .iter()
        .position(|entry| entry.vendor_guid == guid);
    match (existing, table.is_null()) {
        (Some(index), false) => entries[index].vendor_table = table,
        (Some(index), true) => {
            entries.copy_within(index + 1..*count, index);
            *count -= 1;
            entries[*count] = EMPTY_ENTRY;
        }
        (None, false) if *count < entries.len() => {
            entries[*count] = ConfigurationTable {
                vendor_guid: guid,
                vendor_table: table,
            };
            *count += 1;
        }
        (None, false) => return Err(Status::OUT_OF_RESOURCES),
        (None, true) => return Err(Status::NOT_FOUND),
    }
    Ok(())
}

/// # Safety
/// Call on the BSP. The GUID is readable and aligned. An installed table
/// remains owned by its caller and lives as long as the entry is published.
pub unsafe extern "efiapi" fn install_configuration_table(
    guid: *const Guid,
    table: *mut c_void,
) -> Status {
    if guid.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let guid = unsafe { guid.read() };
    let status = unsafe {
        state::with(|state| {
            let Some(mut system) = state.system_table else {
                return Status::NOT_READY;
            };
            if let Err(status) = state.configuration.update(state.allocator, guid, table) {
                return status;
            }
            let (address, count) = state.configuration.published();
            system.as_mut().publish_configuration(address, count);
            state.events.signal_group(guid);
            Status::SUCCESS
        })
    };
    if status == Status::SUCCESS {
        unsafe { event::dispatch_pending() };
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guid(id: u32) -> Guid {
        Guid {
            data1: id,
            ..EMPTY_ENTRY.vendor_guid
        }
    }

    #[test]
    fn config_updates_reuse_the_slot_and_removal_compacts_the_array() {
        let mut entries = [EMPTY_ENTRY; MAX_CONFIGURATION_TABLES];
        let mut count = 0;
        let original = ptr::without_provenance_mut(1);
        let replaced = ptr::without_provenance_mut(2);
        update_entries(&mut entries, &mut count, guid(1), original).unwrap();
        update_entries(&mut entries, &mut count, guid(2), original).unwrap();
        update_entries(&mut entries, &mut count, guid(1), replaced).unwrap();
        assert_eq!(count, 2);
        assert_eq!(entries[0].vendor_table, replaced);
        assert_eq!(entries[1].vendor_table, original);

        update_entries(&mut entries, &mut count, guid(1), ptr::null_mut()).unwrap();
        assert_eq!(count, 1);
        assert_eq!(entries[0].vendor_guid, guid(2));
        assert_eq!(entries[0].vendor_table, original);
        assert!(entries[1].vendor_table.is_null());
        assert_eq!(
            update_entries(&mut entries, &mut count, guid(1), ptr::null_mut()),
            Err(Status::NOT_FOUND),
        );
    }

    #[test]
    fn config_capacity_does_not_prevent_replacement_or_mutate_on_failure() {
        let mut entries = [EMPTY_ENTRY; MAX_CONFIGURATION_TABLES];
        let mut count = 0;
        let original = ptr::without_provenance_mut(1);
        let replaced = ptr::without_provenance_mut(2);
        for index in 0..MAX_CONFIGURATION_TABLES {
            update_entries(&mut entries, &mut count, guid(index as u32), original).unwrap();
        }
        assert_eq!(
            update_entries(
                &mut entries,
                &mut count,
                guid(MAX_CONFIGURATION_TABLES as u32),
                replaced
            ),
            Err(Status::OUT_OF_RESOURCES),
        );
        assert_eq!(count, entries.len());
        assert!(entries.iter().all(|entry| entry.vendor_table == original));
        update_entries(&mut entries, &mut count, guid(0), replaced).unwrap();
        assert_eq!(count, entries.len());
        assert_eq!(entries[0].vendor_table, replaced);
    }
}
