//! Page-backed pool allocations, sharing the page allocator's map and key.

use super::*;

const MAGIC: u64 = u64::from_le_bytes(*b"ODFWPOOL");
// Start above address zero even when the backing begins in physical page zero.
const HEADER_OFFSET: u64 = core::mem::align_of::<PoolHeader>() as u64;
const PAYLOAD_OFFSET: u64 = HEADER_OFFSET + core::mem::size_of::<PoolHeader>() as u64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PoolError {
    Pages(PageError),
    SizeOverflow,
    InvalidPointer,
    MemoryUnavailable,
    InvalidHeader,
}

impl PoolError {
    pub fn stop_detail(self) -> u64 {
        match self {
            Self::Pages(error) => error.stop_detail(),
            Self::SizeOverflow => 8,
            Self::InvalidPointer => 9,
            Self::MemoryUnavailable => 10,
            Self::InvalidHeader => 11,
        }
    }
}

impl From<PageError> for PoolError {
    fn from(value: PageError) -> Self {
        Self::Pages(value)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PoolHeader {
    magic: u64,
    base: u64,
    page_count: u64,
    size: u64,
}

/// Supplies a writable header within conventional or pool-owned memory.
/// Looking up a header leaves the backing unmodified.
pub trait PoolMemory {
    /// # Safety
    /// The address must be the header position within an exclusive range
    /// obtained from the allocator's trusted memory map.
    unsafe fn header_at(&mut self, address: u64) -> Option<&mut PoolHeader>;
}

impl PageAllocator<'_, '_> {
    pub fn allocate_pool(
        &mut self,
        memory: &mut impl PoolMemory,
        memory_type: MemoryType,
        size: u64,
    ) -> Result<u64, PoolError> {
        if memory_type == MemoryType::Reserved {
            return Err(PageError::InvalidType.into());
        }
        let attribute = allocation_attributes(memory_type)?;
        let page_count = backing_pages(size)?;
        let (covered, span) = self.plan_allocation(AllocateAt::Any, byte_count(page_count)?)?;
        // SAFETY: the plan covers only free, writable RAM. The header fits
        // within the first page, and no live allocation owns this range.
        let header = unsafe { memory.header_at(span.start + HEADER_OFFSET) }
            .ok_or(PoolError::MemoryUnavailable)?;
        self.replace(
            covered,
            MemoryDescriptor::for_range(memory_type, span.start, span.end, attribute),
            PageOwner::Pool,
        )?;
        *header = PoolHeader {
            magic: MAGIC,
            base: span.start,
            page_count,
            size,
        };
        Ok(span.start + PAYLOAD_OFFSET)
    }

    pub fn free_pool(
        &mut self,
        memory: &mut impl PoolMemory,
        address: u64,
    ) -> Result<(), PoolError> {
        let base = address
            .checked_sub(PAYLOAD_OFFSET)
            .ok_or(PoolError::InvalidPointer)?;
        let descriptors = self.map.descriptors();
        let index = descriptors
            .binary_search_by_key(&base, |descriptor| descriptor.physical_start)
            .map_err(|_| PoolError::InvalidPointer)?;
        if self.owners[index] != PageOwner::Pool {
            return Err(PoolError::InvalidPointer);
        }
        let descriptor = descriptors[index];
        // SAFETY: the descriptor and ownership prove this is the exact
        // header in a live pool allocation. The payload begins after it.
        let header = unsafe { memory.header_at(base + HEADER_OFFSET) }
            .ok_or(PoolError::MemoryUnavailable)?;
        if header.magic != MAGIC
            || header.base != base
            || header.page_count != descriptor.number_of_pages
            || backing_pages(header.size) != Ok(header.page_count)
        {
            return Err(PoolError::InvalidHeader);
        }
        self.release(index..index + 1, base..descriptor.end())?;
        Ok(())
    }
}

fn backing_pages(size: u64) -> Result<u64, PoolError> {
    let bytes = PAYLOAD_OFFSET
        .checked_add(size.max(1))
        .ok_or(PoolError::SizeOverflow)?;
    Ok(bytes.div_ceil(PAGE_BYTES))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u64 = 0;
    const END: u64 = 8 * PAGE_BYTES;
    const EMPTY_HEADER: PoolHeader = PoolHeader {
        magic: 0,
        base: 0,
        page_count: 0,
        size: 0,
    };

    struct Arena {
        headers: [PoolHeader; (END / PAGE_BYTES) as usize],
        accesses: usize,
        unavailable: bool,
    }

    impl Arena {
        fn new() -> Self {
            Self {
                headers: [EMPTY_HEADER; (END / PAGE_BYTES) as usize],
                accesses: 0,
                unavailable: false,
            }
        }
    }

    impl PoolMemory for Arena {
        unsafe fn header_at(&mut self, address: u64) -> Option<&mut PoolHeader> {
            self.accesses += 1;
            let base = address.checked_sub(HEADER_OFFSET)?;
            if self.unavailable || base % PAGE_BYTES != 0 {
                return None;
            }
            self.headers.get_mut((base / PAGE_BYTES) as usize)
        }
    }

    fn map(storage: &mut [MemoryDescriptor]) -> MemoryMap<'_> {
        storage[0] = MemoryDescriptor::for_range(MemoryType::Conventional, BASE, END, WB);
        MemoryMap {
            storage,
            used: 1,
            map_key: 1,
        }
    }

    #[test]
    fn size_boundaries_alignment_and_runtime_type_share_the_memory_map() {
        for (size, expected_pages) in [
            (0, 1),
            (1, 1),
            (8, 1),
            (PAGE_BYTES - PAYLOAD_OFFSET, 1),
            (PAGE_BYTES - PAYLOAD_OFFSET + 1, 2),
            (END - PAYLOAD_OFFSET, END / PAGE_BYTES),
        ] {
            let mut records = [MemoryDescriptor::EMPTY; 4];
            let mut owners = [PageOwner::Initial; 4];
            let mut allocator = PageAllocator::new(map(&mut records), &mut owners).unwrap();
            let mut arena = Arena::new();

            let payload = allocator
                .allocate_pool(&mut arena, MemoryType::RuntimeServicesData, size)
                .unwrap();
            let base = END - expected_pages * PAGE_BYTES;
            assert_eq!(payload, base + PAYLOAD_OFFSET);
            assert_eq!(payload % 8, 0);
            assert_eq!(allocator.map().key(), 2);
            let descriptor = allocator.map().descriptors().last().unwrap();
            assert_eq!(descriptor.attribute, WB | RUNTIME);
            assert_eq!(descriptor.number_of_pages, expected_pages);
            assert_eq!(arena.headers[(base / PAGE_BYTES) as usize].size, size);

            allocator.free_pool(&mut arena, payload).unwrap();
            assert_eq!(allocator.map().key(), 3);
            assert_eq!(
                allocator.map().descriptors(),
                [MemoryDescriptor::for_range(
                    MemoryType::Conventional,
                    BASE,
                    END,
                    WB
                )]
            );
        }
    }

    #[test]
    fn distinct_blocks_and_pages_reject_cross_free_without_header_access() {
        let mut records = [MemoryDescriptor::EMPTY; 8];
        let mut owners = [PageOwner::Initial; 8];
        let mut allocator = PageAllocator::new(map(&mut records), &mut owners).unwrap();
        let mut arena = Arena::new();

        let first = allocator
            .allocate_pool(&mut arena, MemoryType::LoaderData, 7)
            .unwrap();
        let second = allocator
            .allocate_pool(&mut arena, MemoryType::LoaderData, 8)
            .unwrap();
        let page = allocator
            .allocate_pages(AllocateAt::Any, MemoryType::LoaderData, 1)
            .unwrap();
        assert_eq!(second + PAGE_BYTES, first);
        assert_eq!(allocator.map().descriptors().len(), 4);
        assert_eq!(
            allocator.free_pages(second - PAYLOAD_OFFSET, 1),
            Err(PageError::NotFound)
        );
        assert_eq!(allocator.free_pages(page, 2), Err(PageError::NotFound));
        for invalid in [0, u64::MAX, first - 1, first + 8, page + PAYLOAD_OFFSET] {
            assert_eq!(
                allocator.free_pool(&mut arena, invalid),
                Err(PoolError::InvalidPointer)
            );
        }
        assert_eq!(arena.accesses, 2);
        assert_eq!(allocator.map().key(), 4);

        allocator.free_pool(&mut arena, second).unwrap();
        assert_eq!(
            allocator.free_pool(&mut arena, second),
            Err(PoolError::InvalidPointer)
        );
        assert_eq!(arena.accesses, 3);
        assert_eq!(allocator.map().key(), 5);
        allocator.free_pool(&mut arena, first).unwrap();
        allocator.free_pages(page, 1).unwrap();
        assert_eq!(allocator.map().descriptors().len(), 1);
    }

    #[test]
    fn validation_and_metadata_failures_leave_map_and_backing_intact() {
        let mut records = [MemoryDescriptor::EMPTY; 2];
        let mut owners = [PageOwner::Initial; 2];
        let mut allocator = PageAllocator::new(map(&mut records), &mut owners).unwrap();
        let mut arena = Arena::new();
        let pool_type = MemoryType::BootServicesData;

        assert_eq!(
            allocator.allocate_pool(&mut arena, pool_type, u64::MAX),
            Err(PoolError::SizeOverflow)
        );
        for ty in [
            MemoryType::Reserved,
            MemoryType::Conventional,
            MemoryType::MemoryMappedIo,
        ] {
            assert_eq!(
                allocator.allocate_pool(&mut arena, ty, 8),
                Err(PageError::InvalidType.into())
            );
        }
        assert_eq!(arena.accesses, 0);

        arena.unavailable = true;
        assert_eq!(
            allocator.allocate_pool(&mut arena, pool_type, 8),
            Err(PoolError::MemoryUnavailable)
        );
        assert_eq!(allocator.map().key(), 1);
        assert!(arena.headers.iter().all(|header| *header == EMPTY_HEADER));
        arena.unavailable = false;

        let payload = allocator.allocate_pool(&mut arena, pool_type, 8).unwrap();
        let base = payload - PAYLOAD_OFFSET;
        let header_index = (base / PAGE_BYTES) as usize;
        let header = arena.headers[header_index];
        let original = allocator.map().descriptors().to_vec();
        assert_eq!(
            allocator.allocate_pool(&mut arena, pool_type, 8),
            Err(PageError::OutOfDescriptors.into())
        );
        assert_eq!(allocator.map().descriptors(), original);
        assert_eq!(allocator.map().key(), 2);
        assert_eq!(arena.headers[header_index], header);
        assert_eq!(arena.headers[header_index - 1], EMPTY_HEADER);

        arena.headers[header_index].page_count += 1;
        assert_eq!(
            allocator.free_pool(&mut arena, payload),
            Err(PoolError::InvalidHeader)
        );
        assert_eq!(allocator.map().descriptors(), original);
        assert_eq!(allocator.map().key(), 2);

        arena.headers[header_index] = header;
        allocator.map.map_key = u64::MAX;
        assert_eq!(
            allocator.free_pool(&mut arena, payload),
            Err(PageError::MapKeyExhausted.into())
        );
        assert_eq!(allocator.map().descriptors(), original);
        assert_eq!(allocator.map().key(), u64::MAX);
        assert_eq!(arena.headers[header_index], header);
    }
}
