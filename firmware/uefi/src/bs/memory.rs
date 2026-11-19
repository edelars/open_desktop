//! UEFI 2.11 §7.2 memory services over the firmware's single page allocator.

use super::{misc, state};
use crate::mem::map::{DESCRIPTOR_VERSION, MemoryDescriptor, MemoryType};
use crate::mem::pages::{AllocateAt, PageAllocator, PageError, PoolError, PoolHeader, PoolMemory};
use crate::uefi::status::Status;
use core::ffi::c_void;
use core::ptr;

pub struct IdentityPoolMemory;

impl PoolMemory for IdentityPoolMemory {
    unsafe fn header_at(&mut self, address: u64) -> Option<&mut PoolHeader> {
        // SAFETY: the caller supplies an exclusive, aligned header position.
        // The active page tables identity-map this RAM and allow writes.
        Some(unsafe { &mut *ptr::with_exposed_provenance_mut::<PoolHeader>(address as usize) })
    }
}

/// # Safety
/// Call on the BSP. A non-null output points to writable, aligned storage
/// outside the private allocator state. The value is initialized for
/// AllocateMaxAddress and AllocateAddress.
pub unsafe extern "efiapi" fn allocate_pages(
    allocation_type: u32,
    memory_type: MemoryType,
    pages: usize,
    address: *mut u64,
) -> Status {
    if address.is_null() || pages == 0 || memory_type == MemoryType::RESERVED {
        return Status::INVALID_PARAMETER;
    }
    let placement = match allocation_type {
        0 => AllocateAt::Any,
        1 => AllocateAt::MaxAddress(unsafe { address.read() }),
        2 => AllocateAt::Address(unsafe { address.read() }),
        _ => return Status::INVALID_PARAMETER,
    };
    unsafe {
        state::with_map_mutation(|state| {
            match state
                .allocator
                .allocate_pages(placement, memory_type, pages as u64)
            {
                Ok(base) => {
                    address.write(base);
                    Status::SUCCESS
                }
                Err(PageError::NotFound) if placement != AllocateAt::Any => Status::NOT_FOUND,
                Err(PageError::InvalidPageCount) => Status::OUT_OF_RESOURCES,
                Err(error) => allocate_page_error(error),
            }
        })
    }
}

/// # Safety
/// Call on the BSP.
pub unsafe extern "efiapi" fn free_pages(address: u64, pages: usize) -> Status {
    unsafe {
        state::with_map_mutation(
            |state| match state.allocator.free_pages(address, pages as u64) {
                Ok(()) => Status::SUCCESS,
                Err(PageError::NotFound) => Status::NOT_FOUND,
                Err(error) => allocate_page_error(error),
            },
        )
    }
}

/// # Safety
/// Call on the BSP. Non-null scalar outputs must be writable and aligned.
/// The size is initialized, and a non-null map covers the stated capacity.
/// Output regions must not overlap each other or the firmware's private map.
pub unsafe extern "efiapi" fn get_memory_map(
    memory_map_size: *mut usize,
    memory_map: *mut MemoryDescriptor,
    map_key: *mut usize,
    descriptor_size: *mut usize,
    descriptor_version: *mut u32,
) -> Status {
    if memory_map_size.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let capacity = unsafe { memory_map_size.read() };
    unsafe {
        state::with(|state| {
            let map = state.allocator.map();
            let descriptors = map.descriptors();
            let required = core::mem::size_of_val(descriptors);
            if capacity >= required && memory_map.is_null() {
                return Status::INVALID_PARAMETER;
            }
            memory_map_size.write(required);
            if !descriptor_size.is_null() {
                descriptor_size.write(size_of::<MemoryDescriptor>());
            }
            if !descriptor_version.is_null() {
                descriptor_version.write(DESCRIPTOR_VERSION);
            }
            if capacity < required {
                return Status::BUFFER_TOO_SMALL;
            }
            misc::copy_mem(memory_map.cast(), descriptors.as_ptr().cast(), required);
            if !map_key.is_null() {
                map_key.write(map.key() as usize);
            }
            Status::SUCCESS
        })
    }
}

/// # Safety
/// Call on the BSP. A non-null output points to writable, aligned pointer
/// storage outside the private allocator state.
pub unsafe extern "efiapi" fn allocate_pool(
    memory_type: MemoryType,
    size: usize,
    buffer: *mut *mut c_void,
) -> Status {
    if buffer.is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe {
        state::with_map_mutation(|state| {
            match allocate_pool_in(state.allocator, memory_type, size) {
                Ok(address) => {
                    buffer.write(address);
                    Status::SUCCESS
                }
                Err(status) => status,
            }
        })
    }
}

/// Allocate under an already-held BSP service guard. The map must describe
/// identity-mapped and writable conventional RAM.
pub(super) unsafe fn allocate_pool_in(
    allocator: &mut PageAllocator<'_, '_>,
    memory_type: MemoryType,
    size: usize,
) -> Result<*mut c_void, Status> {
    match allocator.allocate_pool(&mut IdentityPoolMemory, memory_type, size as u64) {
        Ok(address) => Ok(ptr::with_exposed_provenance_mut(address as usize)),
        Err(
            PoolError::Pages(PageError::InvalidPageCount)
            | PoolError::SizeOverflow
            | PoolError::MemoryUnavailable,
        ) => Err(Status::OUT_OF_RESOURCES),
        Err(PoolError::Pages(error)) => Err(allocate_page_error(error)),
        Err(PoolError::InvalidPointer | PoolError::InvalidHeader) => Err(Status::INVALID_PARAMETER),
    }
}

/// # Safety
/// Call on the BSP.
pub unsafe extern "efiapi" fn free_pool(buffer: *mut c_void) -> Status {
    unsafe {
        state::with_map_mutation(|state| {
            match state
                .allocator
                .free_pool(&mut IdentityPoolMemory, buffer.addr() as u64)
            {
                Ok(()) => Status::SUCCESS,
                Err(PoolError::Pages(error)) => allocate_page_error(error),
                Err(_) => Status::INVALID_PARAMETER,
            }
        })
    }
}

fn allocate_page_error(error: PageError) -> Status {
    match error {
        PageError::InvalidAddress | PageError::InvalidPageCount | PageError::InvalidType => {
            Status::INVALID_PARAMETER
        }
        PageError::NotFound | PageError::OutOfDescriptors | PageError::MapKeyExhausted => {
            Status::OUT_OF_RESOURCES
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::map::MemoryMap;
    use crate::mem::pages::{PageAllocator, PageOwner};

    #[test]
    fn efi_memory_map_sizes_and_keys_follow_the_live_allocator() {
        static mut MAP: [MemoryDescriptor; 8] = [MemoryDescriptor::EMPTY; 8];
        static mut OWNERS: [PageOwner; 8] = [PageOwner::Initial; 8];

        // SAFETY: this test alone initializes and calls the host instance of
        // the BSP allocator. Outputs are live, disjoint buffers.
        unsafe {
            let map =
                MemoryMap::test_conventional(&mut *ptr::addr_of_mut!(MAP), 0x100000, 0x110000);
            state::initialize(
                PageAllocator::new(map, &mut *ptr::addr_of_mut!(OWNERS)).unwrap(),
                1_000_000_000,
                || 0,
            );

            let mut snapshot = [MemoryDescriptor::EMPTY; 8];
            let mut size = 0;
            let mut key = usize::MAX;
            let mut stride = 0;
            let mut version = 0;
            assert_eq!(
                get_memory_map(
                    &mut size,
                    snapshot.as_mut_ptr(),
                    &mut key,
                    &mut stride,
                    &mut version,
                ),
                Status::BUFFER_TOO_SMALL
            );
            assert_eq!((size, key, stride, version), (40, usize::MAX, 40, 1));
            assert_eq!(snapshot, [MemoryDescriptor::EMPTY; 8]);
            assert_eq!(
                get_memory_map(
                    &mut size,
                    ptr::null_mut(),
                    &mut key,
                    &mut stride,
                    &mut version
                ),
                Status::INVALID_PARAMETER
            );
            assert_eq!((size, key, stride, version), (40, usize::MAX, 40, 1));

            let mut page = 0x104fff;
            assert_eq!(
                state::with(|_| allocate_pages(1, MemoryType::LOADER_DATA, 1, &mut page)),
                Status::NOT_READY
            );
            assert_eq!(page, 0x104fff);
            assert_eq!(
                allocate_pages(0, MemoryType::from_raw(16), 1, &mut page),
                Status::INVALID_PARAMETER
            );
            assert_eq!(page, 0x104fff);
            assert_eq!(
                allocate_pages(0, MemoryType::LOADER_DATA, 0, &mut page),
                Status::INVALID_PARAMETER
            );
            assert_eq!(
                allocate_pages(0, MemoryType::LOADER_DATA, usize::MAX, &mut page),
                Status::OUT_OF_RESOURCES
            );
            assert_eq!(page, 0x104fff);
            let mut pool = ptr::null_mut();
            assert_eq!(
                allocate_pool(MemoryType::LOADER_DATA, 1 << 36, &mut pool),
                Status::OUT_OF_RESOURCES
            );
            assert!(pool.is_null());

            let oem_type = MemoryType::from_raw(0x7000_fead);
            assert_eq!(allocate_pages(1, oem_type, 1, &mut page), Status::SUCCESS);
            assert_eq!(page, 0x104000);
            assert_eq!(allocate_pages(2, oem_type, 1, &mut page), Status::NOT_FOUND);
            assert_eq!(page, 0x104000);
            assert_eq!(
                allocate_pool(MemoryType::LOADER_DATA, 4, ptr::null_mut()),
                Status::INVALID_PARAMETER
            );
            assert_eq!(free_pool(ptr::null_mut()), Status::INVALID_PARAMETER);

            size = size_of_val(&snapshot);
            assert_eq!(
                get_memory_map(
                    &mut size,
                    snapshot.as_mut_ptr(),
                    &mut key,
                    &mut stride,
                    &mut version,
                ),
                Status::SUCCESS
            );
            assert_eq!((size, key, stride, version), (120, 2, 40, 1));
            assert_eq!(snapshot[1].memory_type, oem_type.raw());
            assert_eq!(snapshot[1].physical_start, page);
            assert_eq!(snapshot[1].number_of_pages, 1);
            assert_eq!(snapshot[3..], [MemoryDescriptor::EMPTY; 5]);

            assert_eq!(free_pages(page + 1, 1), Status::INVALID_PARAMETER);
            assert_eq!(free_pages(page, 1), Status::SUCCESS);
            assert_eq!(free_pages(page, 1), Status::NOT_FOUND);
            assert_eq!(
                get_memory_map(
                    &mut size,
                    snapshot.as_mut_ptr(),
                    &mut key,
                    ptr::null_mut(),
                    ptr::null_mut(),
                ),
                Status::SUCCESS
            );
            assert_eq!((size, key), (40, 3));
            assert_eq!(snapshot[0].memory_type, MemoryType::CONVENTIONAL.raw());
            assert_eq!(snapshot[0].number_of_pages, 16);

            let mut conventional = 0;
            assert_eq!(
                allocate_pages(0, MemoryType::CONVENTIONAL, 1, &mut conventional),
                Status::SUCCESS
            );
            assert_eq!(conventional, 0x10f000);
            assert_eq!(
                allocate_pages(2, MemoryType::LOADER_DATA, 1, &mut conventional),
                Status::NOT_FOUND
            );
            let mut port_space = 0;
            assert_eq!(
                allocate_pages(
                    0,
                    MemoryType::MEMORY_MAPPED_IO_PORT_SPACE,
                    1,
                    &mut port_space
                ),
                Status::SUCCESS
            );
            assert_eq!(port_space, 0x10e000);
            size = size_of_val(&snapshot);
            assert_eq!(
                get_memory_map(
                    &mut size,
                    snapshot.as_mut_ptr(),
                    &mut key,
                    &mut stride,
                    &mut version
                ),
                Status::SUCCESS
            );
            assert_eq!((size, key), (120, 5));
            assert_eq!(
                snapshot[1].memory_type,
                MemoryType::MEMORY_MAPPED_IO_PORT_SPACE.raw()
            );
            assert_eq!(snapshot[2].memory_type, MemoryType::CONVENTIONAL.raw());
            assert_eq!(free_pages(conventional, 1), Status::SUCCESS);
            assert_eq!(free_pages(port_space, 1), Status::SUCCESS);
        }
    }
}
