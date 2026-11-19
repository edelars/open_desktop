//! Runtime-owned event registrations carried across ExitBootServices.
use super::{Entry, Event, EventNotify, MAX_EVENTS, Tpl};
use crate::bs::memory::allocate_pool_in;
use crate::mem::{map::MemoryType, pages::PageAllocator};
use crate::uefi::{guid::Guid, status::Status};
use core::{ffi::c_void, ptr, ptr::NonNull};

pub const EVT_RUNTIME: u32 = 0x4000_0000;
pub const EVT_SIGNAL_VIRTUAL_ADDRESS_CHANGE: u32 = 0x6000_0202;
pub const VIRTUAL_ADDRESS_CHANGE: Guid = Guid {
    data1: 0x13fa_7698,
    data2: 0xc831,
    data3: 0x49c7,
    data4: [0x87, 0xea, 0x8f, 0x43, 0xfc, 0xc2, 0x51, 0x96],
};

#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct RuntimeEventRecord {
    event: Event,
    event_type: u32,
    has_group: u32,
    group: Guid,
    notify: Option<EventNotify>,
    context: *mut c_void,
    notify_tpl: Tpl,
}

impl RuntimeEventRecord {
    const EMPTY: Self = Self {
        event: ptr::null_mut(),
        event_type: 0,
        has_group: 0,
        group: Guid {
            data1: 0,
            data2: 0,
            data3: 0,
            data4: [0; 8],
        },
        notify: None,
        context: ptr::null_mut(),
        notify_tpl: 0,
    };

    fn from_entry(entry: Entry, event_type: u32) -> Self {
        let mut record = Self {
            event: entry.event,
            event_type,
            ..Self::EMPTY
        };
        if let Some(group) = entry.group {
            record.has_group = 1;
            record.group = group;
        }
        if let Some(notify) = entry.notify {
            record.notify = Some(notify.callback);
            record.context = notify.context;
            record.notify_tpl = notify.tpl;
        }
        record
    }
}

#[repr(C)]
pub(crate) struct RuntimeEventTable {
    records: [RuntimeEventRecord; MAX_EVENTS],
}

pub(super) struct RuntimeEventRegistrations {
    table: Option<NonNull<RuntimeEventTable>>,
}

impl RuntimeEventRegistrations {
    pub(super) const fn new() -> Self {
        Self { table: None }
    }

    pub(super) fn root(&self) -> *mut RuntimeEventTable {
        self.table.map_or(ptr::null_mut(), NonNull::as_ptr)
    }

    // The BSP guard must own the allocator and the selected empty boot slot.
    pub(super) unsafe fn retain(
        &mut self,
        allocator: &mut PageAllocator<'_, '_>,
        index: usize,
        entry: Entry,
        event_type: u32,
    ) -> Result<(), Status> {
        let mut table = match self.table {
            Some(table) => table,
            None => {
                let storage = unsafe {
                    allocate_pool_in(
                        allocator,
                        MemoryType::RUNTIME_SERVICES_DATA,
                        size_of::<RuntimeEventTable>(),
                    )?
                };
                // SAFETY: the pool owns a non-null aligned payload large
                // enough for the records. The BSP is its sole initializer.
                let table = unsafe { NonNull::new_unchecked(storage.cast::<RuntimeEventTable>()) };
                unsafe {
                    table.as_ptr().write(RuntimeEventTable {
                        records: [RuntimeEventRecord::EMPTY; MAX_EVENTS],
                    });
                }
                self.table = Some(table);
                table
            }
        };
        // SAFETY: the retained slot corresponds to the prepared boot entry.
        unsafe {
            table.as_mut().records[index] = RuntimeEventRecord::from_entry(entry, event_type)
        };
        Ok(())
    }

    pub(super) fn forget(&mut self, index: usize) {
        if let Some(mut table) = self.table {
            // SAFETY: all pre-exit access is owned by the BSP state guard.
            unsafe { table.as_mut().records[index] = RuntimeEventRecord::EMPTY };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_layout_preserves_the_runtime_conversion_inputs() {
        assert_eq!(size_of::<RuntimeEventRecord>(), 56);
        assert_eq!(core::mem::offset_of!(RuntimeEventRecord, group), 16);
        assert_eq!(core::mem::offset_of!(RuntimeEventRecord, notify), 32);
        assert_eq!(core::mem::offset_of!(RuntimeEventRecord, context), 40);
        assert_eq!(core::mem::offset_of!(RuntimeEventRecord, notify_tpl), 48);
        assert_eq!(size_of::<RuntimeEventTable>(), 56 * MAX_EVENTS);
    }
}
