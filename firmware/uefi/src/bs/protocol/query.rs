//! Enumerates protocol bindings and opens through UEFI 2.11 §7.3.

use super::{MAX_OPEN_RECORDS, MAX_PROTOCOL_BINDINGS, ProtocolRegistry};
use crate::bs::{memory::allocate_pool_in, misc, state};
use crate::mem::map::MemoryType;
use crate::uefi::{guid::Guid, status::Status, types::Handle};
use core::ffi::c_void;
use core::ptr;

pub const LOCATE_ALL_HANDLES: u32 = 0;
const LOCATE_BY_REGISTER_NOTIFY: u32 = 1;
pub const LOCATE_BY_PROTOCOL: u32 = 2;

#[derive(Clone, Copy)]
enum HandleSearch {
    All,
    Protocol(Guid),
}

// Callers provide a readable aligned GUID whenever the selected mode uses it.
unsafe fn decode_handle_search(
    search_type: u32,
    protocol: *const Guid,
    search_key: *mut c_void,
) -> Result<HandleSearch, Status> {
    match search_type {
        LOCATE_ALL_HANDLES => Ok(HandleSearch::All),
        LOCATE_BY_PROTOCOL if !protocol.is_null() => {
            Ok(HandleSearch::Protocol(unsafe { protocol.read() }))
        }
        LOCATE_BY_REGISTER_NOTIFY if !search_key.is_null() => Err(Status::UNSUPPORTED),
        _ => Err(Status::INVALID_PARAMETER),
    }
}

impl ProtocolRegistry {
    fn handles(&self, search: HandleSearch, output: &mut [Handle; MAX_PROTOCOL_BINDINGS]) -> usize {
        let mut count = 0;
        for binding in self.bindings.iter().flatten() {
            if let HandleSearch::Protocol(protocol) = search
                && protocol != binding.protocol
            {
                continue;
            }
            if !output[..count].contains(&binding.handle) {
                output[count] = binding.handle;
                count += 1;
            }
        }
        count
    }
}

/// # Safety
/// Call on the BSP. Required non-null pointer arguments refer to aligned
/// buffers valid for the reads or writes described by the ABI. Result buffers
/// are disjoint and outside firmware-private state.
pub unsafe extern "efiapi" fn locate_handle(
    search_type: u32,
    protocol: *const Guid,
    search_key: *mut c_void,
    buffer_size: *mut usize,
    buffer: *mut Handle,
) -> Status {
    unsafe {
        let search = match decode_handle_search(search_type, protocol, search_key) {
            Ok(search) => search,
            Err(status) => return status,
        };
        state::with(|state| {
            let mut handles = [ptr::null_mut(); MAX_PROTOCOL_BINDINGS];
            let count = state.protocols.handles(search, &mut handles);
            if count == 0 {
                return Status::NOT_FOUND;
            }
            if buffer_size.is_null() {
                return Status::INVALID_PARAMETER;
            }
            let bytes = size_of::<Handle>() * count;
            if buffer_size.read() < bytes {
                buffer_size.write(bytes);
                return Status::BUFFER_TOO_SMALL;
            }
            if buffer.is_null() {
                return Status::INVALID_PARAMETER;
            }
            misc::copy_mem(buffer.cast(), handles.as_ptr().cast(), bytes);
            buffer_size.write(bytes);
            Status::SUCCESS
        })
    }
}

/// # Safety
/// Call on the BSP. Selected input pointers are readable and aligned. The
/// outputs are aligned, writable and disjoint from each other and from
/// firmware-private state.
pub unsafe extern "efiapi" fn locate_handle_buffer(
    search_type: u32,
    protocol: *const Guid,
    search_key: *mut c_void,
    handle_count: *mut usize,
    buffer: *mut *mut Handle,
) -> Status {
    if handle_count.is_null() || buffer.is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe {
        let search = match decode_handle_search(search_type, protocol, search_key) {
            Ok(search) => search,
            Err(status) => return status,
        };
        state::with(|state| {
            let mut handles = [ptr::null_mut(); MAX_PROTOCOL_BINDINGS];
            let count = state.protocols.handles(search, &mut handles);
            if count == 0 {
                return Status::NOT_FOUND;
            }
            let bytes = size_of::<Handle>() * count;
            let allocation =
                match allocate_pool_in(state.allocator, MemoryType::BOOT_SERVICES_DATA, bytes) {
                    Ok(allocation) => allocation,
                    Err(status) => return status,
                };
            misc::copy_mem(allocation, handles.as_ptr().cast(), bytes);
            buffer.write(allocation.cast());
            handle_count.write(count);
            Status::SUCCESS
        })
    }
}

/// # Safety
/// Call on the BSP. Both outputs are aligned, writable and disjoint from each
/// other and from firmware-private state.
pub unsafe extern "efiapi" fn protocols_per_handle(
    handle: Handle,
    buffer: *mut *mut *mut Guid,
    count: *mut usize,
) -> Status {
    if handle.is_null() || buffer.is_null() || count.is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe {
        state::with(|state| {
            let protocols = state
                .protocols
                .bindings
                .iter()
                .flatten()
                .filter(|binding| binding.handle == handle);
            let protocol_count = protocols.clone().count();
            if protocol_count == 0 {
                return Status::INVALID_PARAMETER;
            }
            let bytes = protocol_count * (size_of::<*mut Guid>() + size_of::<Guid>());
            let allocation =
                match allocate_pool_in(state.allocator, MemoryType::BOOT_SERVICES_DATA, bytes) {
                    Ok(allocation) => allocation,
                    Err(status) => return status,
                };
            // Keep the GUID copies in the same allocation as the pointer array
            // so callers can retain this snapshot across database changes.
            let pointers = allocation.cast::<*mut Guid>();
            let guids = pointers.add(protocol_count).cast::<Guid>();
            for (index, binding) in protocols.enumerate() {
                let guid = guids.add(index);
                guid.write(binding.protocol);
                pointers.add(index).write(guid);
            }
            buffer.write(pointers);
            count.write(protocol_count);
            Status::SUCCESS
        })
    }
}

/// UEFI 2.11 §7.3.11 EFI_OPEN_PROTOCOL_INFORMATION_ENTRY.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct OpenProtocolInformationEntry {
    pub agent_handle: Handle,
    pub controller_handle: Handle,
    pub attributes: u32,
    pub open_count: u32,
}

impl OpenProtocolInformationEntry {
    const EMPTY: Self = Self {
        agent_handle: ptr::null_mut(),
        controller_handle: ptr::null_mut(),
        attributes: 0,
        open_count: 0,
    };
}

/// # Safety
/// Call on the BSP. A non-null GUID is aligned and readable. The output
/// locations are aligned and writable, are disjoint from each other and
/// are outside firmware-private state.
pub unsafe extern "efiapi" fn open_protocol_information(
    handle: Handle,
    protocol: *const Guid,
    buffer: *mut *mut OpenProtocolInformationEntry,
    count: *mut usize,
) -> Status {
    if handle.is_null() || protocol.is_null() || buffer.is_null() || count.is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe {
        let protocol = protocol.read();
        state::with(|state| {
            let Some(binding) = state.protocols.binding_index(handle, protocol) else {
                return Status::NOT_FOUND;
            };
            let mut entries = [OpenProtocolInformationEntry::EMPTY; MAX_OPEN_RECORDS];
            let mut entry_count = 0;
            for open in state.protocols.opens.iter().flatten() {
                if open.binding == binding {
                    entries[entry_count] = OpenProtocolInformationEntry {
                        agent_handle: open.agent,
                        controller_handle: open.controller,
                        attributes: open.attributes,
                        open_count: open.count,
                    };
                    entry_count += 1;
                }
            }
            let bytes = size_of::<OpenProtocolInformationEntry>() * entry_count;
            let allocation =
                match allocate_pool_in(state.allocator, MemoryType::BOOT_SERVICES_DATA, bytes) {
                    Ok(allocation) => allocation,
                    Err(status) => return status,
                };
            misc::copy_mem(allocation, entries.as_ptr().cast(), bytes);
            buffer.write(allocation.cast());
            count.write(entry_count);
            Status::SUCCESS
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_search_deduplicates_bindings_and_filters_by_guid() {
        let mut registry = ProtocolRegistry::new();
        let marker = ptr::null_mut();
        let first = Guid {
            data1: 0x7e5d_4321,
            data2: 0x614f,
            data3: 0x4019,
            data4: [0xaf, 0x12, 0, 0, 0, 0, 0, 1],
        };
        let second = Guid {
            data1: first.data1 + 1,
            ..first
        };
        let original = registry.install(marker, first, marker).unwrap();
        registry.install(original, second, marker).unwrap();
        let later = registry.install(marker, first, marker).unwrap();

        let mut output = [marker; MAX_PROTOCOL_BINDINGS];
        assert_eq!(registry.handles(HandleSearch::All, &mut output), 2);
        assert_eq!(output[..2], [original, later]);
        assert_eq!(
            registry.handles(HandleSearch::Protocol(second), &mut output),
            1
        );
        assert_eq!(output[0], original);
        assert_eq!(registry.uninstall(original, first, marker), Ok(()));
        assert_eq!(
            registry.handles(HandleSearch::Protocol(first), &mut output),
            1
        );
        assert_eq!(output[0], later);
    }

    #[test]
    fn search_only_reads_pointers_selected_by_its_mode() {
        let unused_protocol = ptr::without_provenance::<Guid>(1);
        let unused_key = ptr::without_provenance_mut::<c_void>(1);
        // SAFETY: none of these calls selects a non-null GUID input for reading.
        unsafe {
            assert!(matches!(
                decode_handle_search(LOCATE_ALL_HANDLES, unused_protocol, unused_key),
                Ok(HandleSearch::All)
            ));
            assert_eq!(
                decode_handle_search(LOCATE_BY_REGISTER_NOTIFY, unused_protocol, unused_key).err(),
                Some(Status::UNSUPPORTED)
            );
            assert_eq!(
                decode_handle_search(LOCATE_BY_PROTOCOL, ptr::null(), unused_key).err(),
                Some(Status::INVALID_PARAMETER)
            );
        }
    }
}
