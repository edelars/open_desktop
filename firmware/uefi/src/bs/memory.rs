//! UEFI 2.11 §7.2 memory services over the firmware's single page allocator.

use super::{misc, state};
use crate::mem::map::{DESCRIPTOR_VERSION, MemoryDescriptor, MemoryType};
use crate::mem::pages::{AllocateAt, PageAllocator, PageError, PoolError, PoolHeader, PoolMemory};
use crate::paging::{ActivePageTables, PAGE_BYTES};
use crate::uefi::status::Status;
use core::ffi::c_void;
use core::ptr;

/// An owned scratch allocation in Boot Services data memory.
#[cfg(target_os = "none")]
pub(crate) struct BootPool {
    pointer: ptr::NonNull<u8>,
    size: usize,
}

#[cfg(target_os = "none")]
impl BootPool {
    /// Call on the BSP with live Boot Services. The payload is uninitialized.
    pub(crate) unsafe fn allocate(size: usize) -> Result<Self, Status> {
        let mut pointer = ptr::null_mut();
        let status = unsafe { allocate_pool(MemoryType::BOOT_SERVICES_DATA, size, &mut pointer) };
        if status != Status::SUCCESS {
            return Err(status);
        }
        Ok(Self {
            pointer: ptr::NonNull::new(pointer.cast()).ok_or(Status::DEVICE_ERROR)?,
            size,
        })
    }

    pub(crate) fn as_ptr(&self) -> *mut u8 {
        self.pointer.as_ptr()
    }

    pub(crate) fn len(&self) -> usize {
        self.size
    }

    pub(crate) fn zeroed_prefix(&mut self, size: usize) -> Option<&mut [u8]> {
        if size > self.size {
            return None;
        }
        // Use the memory service's bounded chunks for large firmware fills.
        unsafe {
            misc::set_mem(self.as_ptr().cast(), size, 0);
            Some(core::slice::from_raw_parts_mut(self.as_ptr(), size))
        }
    }
}

#[cfg(target_os = "none")]
impl Drop for BootPool {
    fn drop(&mut self) {
        unsafe { free_pool(self.as_ptr().cast()) };
    }
}

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
                    if let Some(tables) = state.active_tables
                        // SAFETY: BSP guard held; the tables are the firmware's active ones.
                        && !unsafe {
                            publish_allocation(
                                &tables,
                                state.allocator,
                                base,
                                pages as u64,
                                memory_type,
                            )
                        }
                    {
                        return Status::DEVICE_ERROR;
                    }
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
                Ok(()) => {
                    // S7-31f: freed pages return to conventional memory, which is data (NX),
                    // whatever the allocation was.
                    if let Some(tables) = state.active_tables {
                        // SAFETY: BSP guard held; the span was owned by AllocatePages.
                        unsafe { publish_free(&tables, address, pages as u64) };
                    }
                    Status::SUCCESS
                }
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

/// UEFI 2.11, memory types: code is allocated as `EfiLoaderCode` or `EfiBootServicesCode`; every
/// other type holds data. The firmware's identity tables map RAM as data (NX); AllocatePages
/// makes the pages of a code allocation executable (S7-31f).
pub(crate) const fn is_code_type(memory_type: MemoryType) -> bool {
    matches!(
        memory_type,
        MemoryType::LOADER_CODE | MemoryType::BOOT_SERVICES_CODE
    )
}

/// Publish the permissions of a fresh AllocatePages allocation in the active tables (S7-31f):
/// the pages of a code allocation become executable and stay writable, so the loader can fill
/// them; data types keep the NX mapping the boot tables gave all RAM. Returns `false` (after
/// giving the pages back) when a page has no present identity leaf.
///
/// # Safety
/// BSP guard held; `tables` are the active tables; the span was just allocated.
pub(crate) unsafe fn publish_allocation(
    tables: &ActivePageTables,
    allocator: &mut PageAllocator<'_, '_>,
    base: u64,
    pages: u64,
    memory_type: MemoryType,
) -> bool {
    if !is_code_type(memory_type) {
        return true;
    }
    let span = base..base + pages * PAGE_BYTES;
    if !unsafe { tables.verify(span.clone(), false) } {
        let _ = allocator.free_pages(base, pages);
        return false;
    }
    unsafe {
        tables.set(span, true, true);
        tables.flush();
    }
    true
}

/// Freed pages return to conventional memory, which is data (NX), whatever the allocation was.
///
/// # Safety
/// BSP guard held; `tables` are the active tables; the span was just freed.
pub(crate) unsafe fn publish_free(tables: &ActivePageTables, address: u64, pages: u64) {
    let span = address..address + pages * PAGE_BYTES;
    if unsafe { tables.verify(span.clone(), false) } {
        unsafe {
            tables.set(span, true, false);
            tables.flush();
        }
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

    const PRESENT: u64 = 1;
    const WRITABLE: u64 = 1 << 1;
    const NO_EXECUTE: u64 = 1 << 63;

    #[test]
    fn code_types_are_exactly_loader_code_and_boot_services_code() {
        for raw in 0..=15u32 {
            assert_eq!(
                is_code_type(MemoryType::from_raw(raw)),
                matches!(raw, 1 | 3),
                "memory type {raw}"
            );
        }
        // Vendor types are data.
        assert!(!is_code_type(MemoryType::from_raw(0x7000_fead)));
    }

    /// S7-31f: AllocatePages of a code type makes its pages executable in the active tables;
    /// data types stay NX; freeing returns the pages to NX; other mappings are untouched.
    #[test]
    fn code_allocations_are_executable_and_data_allocations_are_not() {
        const ENTRIES: usize = 512;
        #[repr(align(4096))]
        struct Arena([u64; ENTRIES * 4]);
        let mut arena = Arena([0; ENTRIES * 4]);
        let root = arena.0.as_mut_ptr().expose_provenance() as u64;
        let tables = ActivePageTables::new(root, 4 * PAGE_BYTES);
        for table in 0..3 {
            arena.0[table * ENTRIES] = root + (table as u64 + 1) * PAGE_BYTES | PRESENT | WRITABLE;
        }
        // Identity leaves for the 16 pages [0x100000, 0x110000), mapped as data like boot RAM.
        let leaf_of = |page: u64| ENTRIES * 3 + ((page >> 12) as usize & (ENTRIES - 1));
        for page in (0x100000u64..0x110000).step_by(PAGE_BYTES as usize) {
            arena.0[leaf_of(page)] = page | PRESENT | WRITABLE | NO_EXECUTE;
        }
        let flags = |arena: &Arena, page: u64| arena.0[leaf_of(page)] & !0x000f_ffff_ffff_f000;
        let mut map = [MemoryDescriptor::EMPTY; 16];
        let mut owners = [PageOwner::Initial; 16];
        let map = MemoryMap::test_conventional(&mut map, 0x100000, 0x110000);
        let mut allocator = PageAllocator::new(map, &mut owners).unwrap();
        let allocate = |allocator: &mut PageAllocator<'_, '_>, memory_type, pages| {
            let base = allocator
                .allocate_pages(AllocateAt::Any, memory_type, pages)
                .unwrap();
            assert!(unsafe { publish_allocation(&tables, allocator, base, pages, memory_type) });
            base
        };
        let code = allocate(&mut allocator, MemoryType::LOADER_CODE, 2);
        let data = allocate(&mut allocator, MemoryType::LOADER_DATA, 1);
        let boot_code = allocate(&mut allocator, MemoryType::BOOT_SERVICES_CODE, 1);
        let boot_data = allocate(&mut allocator, MemoryType::BOOT_SERVICES_DATA, 1);
        // Code: present, writable, executable (XD clear). Data: unchanged, NX.
        let executable = PRESENT | WRITABLE;
        let data_flags = PRESENT | WRITABLE | NO_EXECUTE;
        for page in [code, code + PAGE_BYTES, boot_code] {
            assert_eq!(flags(&arena, page), executable, "page {page:#x}");
        }
        for page in [data, boot_data] {
            assert_eq!(flags(&arena, page), data_flags, "page {page:#x}");
        }
        // Pages nobody allocated keep the boot mapping.
        let untouched = (0x100000u64..0x110000)
            .step_by(PAGE_BYTES as usize)
            .filter(|page| ![code, code + PAGE_BYTES, data, boot_code, boot_data].contains(page));
        for page in untouched {
            assert_eq!(flags(&arena, page), data_flags, "page {page:#x}");
        }
        // Freeing a code allocation returns its pages to NX.
        allocator.free_pages(code, 2).unwrap();
        unsafe { publish_free(&tables, code, 2) };
        for page in [code, code + PAGE_BYTES] {
            assert_eq!(flags(&arena, page), data_flags, "page {page:#x}");
        }
        assert_eq!(flags(&arena, boot_code), executable);
        // A code allocation over a page with no present leaf is refused and given back.
        arena.0[leaf_of(0x10f000)] = 0;
        let base = allocator
            .allocate_pages(AllocateAt::Address(0x10f000), MemoryType::LOADER_CODE, 1)
            .unwrap();
        assert!(!unsafe {
            publish_allocation(&tables, &mut allocator, base, 1, MemoryType::LOADER_CODE)
        });
        assert!(
            allocator
                .map()
                .descriptors()
                .iter()
                .all(|descriptor| !(descriptor.physical_start == 0x10f000
                    && descriptor.memory_type == MemoryType::LOADER_CODE.raw()))
        );
    }

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
