//! Page allocation from the initial EFI map. Ownership tags are private to
//! the allocator and travel with each descriptor when intervals split.

use super::map::{MemoryDescriptor, MemoryMap, MemoryType, RUNTIME, WB};
use crate::paging::PAGE_BYTES;
use core::{convert::identity, ops::Range};

mod image;
mod pool;
pub(crate) use image::ImageMemoryRange;
pub use pool::{PoolError, PoolHeader, PoolMemory};

const PHYSICAL_END: u64 = 1 << 36;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AllocateAt {
    Any,
    MaxAddress(u64),
    Address(u64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageError {
    InvalidPageCount,
    InvalidAddress,
    InvalidType,
    NotFound,
    OutOfDescriptors,
    MapKeyExhausted,
}

impl PageError {
    pub fn stop_detail(self) -> u64 {
        match self {
            Self::InvalidPageCount => 1,
            Self::InvalidAddress => 2,
            Self::InvalidType => 3,
            Self::NotFound => 4,
            Self::OutOfDescriptors => 5,
            Self::MapKeyExhausted => 6,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageOwner {
    Initial,
    Pages,
    Pool,
    Image,
}

/// Takes ownership of the map and caller-provided tags for its entire buffer.
pub struct PageAllocator<'map, 'tags> {
    map: MemoryMap<'map>,
    owners: &'tags mut [PageOwner],
}

impl<'map, 'tags> PageAllocator<'map, 'tags> {
    pub fn new(map: MemoryMap<'map>, owners: &'tags mut [PageOwner]) -> Result<Self, PageError> {
        if owners.len() < map.storage.len() {
            return Err(PageError::OutOfDescriptors);
        }
        owners[..map.used].fill(PageOwner::Initial);
        Ok(Self { map, owners })
    }

    pub fn map(&self) -> &MemoryMap<'map> {
        &self.map
    }

    pub fn allocate_pages(
        &mut self,
        placement: AllocateAt,
        memory_type: MemoryType,
        count: u64,
    ) -> Result<u64, PageError> {
        let attribute = allocation_attributes(memory_type)?;
        let bytes = byte_count(count)?;
        let (covered, span) = self.plan_allocation(placement, bytes)?;
        self.replace(
            covered,
            MemoryDescriptor::for_range(memory_type, span.start, span.end, attribute),
            PageOwner::Pages,
        )?;
        Ok(span.start)
    }

    pub fn free_pages(&mut self, address: u64, count: u64) -> Result<(), PageError> {
        let span = address_range(address, byte_count(count)?)?;
        let covered = self.covered_by(span.clone(), |index, _| {
            self.owners[index] == PageOwner::Pages
        })?;
        self.release(covered, span)
    }

    fn plan_allocation(
        &self,
        placement: AllocateAt,
        bytes: u64,
    ) -> Result<(Range<usize>, Range<u64>), PageError> {
        let address = match placement {
            AllocateAt::Address(address) => address,
            AllocateAt::Any => self.find_highest_fit(bytes, PHYSICAL_END, identity)?,
            AllocateAt::MaxAddress(max) => {
                let end = max.saturating_add(1).min(PHYSICAL_END) & !(PAGE_BYTES - 1);
                self.find_highest_fit(bytes, end, identity)?
            }
        };
        let span = address_range(address, bytes)?;
        let covered = self.covered_by(span.clone(), |index, desc| {
            is_free(desc, self.owners[index])
        })?;
        Ok((covered, span))
    }

    fn release(&mut self, covered: Range<usize>, span: Range<u64>) -> Result<(), PageError> {
        self.replace(
            covered,
            MemoryDescriptor::for_range(MemoryType::CONVENTIONAL, span.start, span.end, WB),
            PageOwner::Initial,
        )
    }

    fn find_highest_fit(
        &self,
        bytes: u64,
        limit: u64,
        align_candidate: impl Fn(u64) -> u64,
    ) -> Result<u64, PageError> {
        let mut best = None;
        let mut run_start = None;
        let mut run_end = 0;
        for (index, &descriptor) in self.map.descriptors().iter().enumerate() {
            if !is_free(descriptor, self.owners[index]) {
                run_start = None;
                continue;
            }
            let start = run_start
                .filter(|_| run_end == descriptor.physical_start)
                .unwrap_or(descriptor.physical_start);
            run_end = descriptor.end();
            run_start = Some(start);
            if let Some(address) = limit.min(run_end).checked_sub(bytes).map(&align_candidate)
                && address >= start
            {
                best = Some(address);
            }
        }
        best.ok_or(PageError::NotFound)
    }

    fn covered_by(
        &self,
        span: Range<u64>,
        mut admits: impl FnMut(usize, MemoryDescriptor) -> bool,
    ) -> Result<Range<usize>, PageError> {
        let descriptors = self.map.descriptors();
        let first = descriptors.partition_point(|descriptor| descriptor.end() <= span.start);
        let mut next = first;
        let mut at = span.start;
        while at < span.end {
            let descriptor = *descriptors.get(next).ok_or(PageError::NotFound)?;
            if descriptor.physical_start > at || !admits(next, descriptor) {
                return Err(PageError::NotFound);
            }
            at = descriptor.end();
            next += 1;
        }
        Ok(first..next)
    }

    // Prepares both boundary fragments and coalesces adjacent compatible
    // descriptors before applying either array edit.
    fn replace(
        &mut self,
        mut covered: Range<usize>,
        replacement: MemoryDescriptor,
        owner: PageOwner,
    ) -> Result<(), PageError> {
        let first = self.entry(covered.start);
        let last = self.entry(covered.end - 1);
        let mut entries = [Entry::EMPTY; 3];
        let mut count = 0;
        if first.descriptor.physical_start < replacement.physical_start {
            entries[count] =
                first.with_range(first.descriptor.physical_start, replacement.physical_start);
            count += 1;
        }
        entries[count] = Entry {
            descriptor: replacement,
            owner,
        };
        count += 1;
        if replacement.end() < last.descriptor.end() {
            entries[count] = last.with_range(replacement.end(), last.descriptor.end());
            count += 1;
        }

        if let Some(index) = covered.start.checked_sub(1) {
            let previous = self.entry(index);
            if previous.can_join(entries[0]) {
                entries[0] = previous.with_range(
                    previous.descriptor.physical_start,
                    entries[0].descriptor.end(),
                );
                covered.start = index;
            }
        }
        if covered.end < self.map.used {
            let following = self.entry(covered.end);
            let final_entry = &mut entries[count - 1];
            if final_entry.can_join(following) {
                *final_entry = final_entry.with_range(
                    final_entry.descriptor.physical_start,
                    following.descriptor.end(),
                );
                covered.end += 1;
            }
        }

        let used = self.map.used - covered.len() + count;
        if used > self.map.storage.len() {
            return Err(PageError::OutOfDescriptors);
        }
        let key = self
            .map
            .map_key
            .checked_add(1)
            .ok_or(PageError::MapKeyExhausted)?;
        let destination = self.move_tail(covered, count, used, key);
        for (index, entry) in destination.zip(entries) {
            self.write_entry(index, entry);
        }
        Ok(())
    }

    fn move_tail(
        &mut self,
        covered: Range<usize>,
        count: usize,
        used: usize,
        key: u64,
    ) -> Range<usize> {
        let destination = covered.start + count;
        self.map
            .storage
            .copy_within(covered.end..self.map.used, destination);
        self.owners
            .copy_within(covered.end..self.map.used, destination);
        self.map.used = used;
        self.map.map_key = key;
        covered.start..destination
    }

    fn write_entry(&mut self, index: usize, entry: Entry) {
        self.map.storage[index] = entry.descriptor;
        self.owners[index] = entry.owner;
    }

    fn entry(&self, index: usize) -> Entry {
        Entry {
            descriptor: self.map.storage[index],
            owner: self.owners[index],
        }
    }
}

#[derive(Clone, Copy)]
struct Entry {
    descriptor: MemoryDescriptor,
    owner: PageOwner,
}

impl Entry {
    const EMPTY: Self = Self {
        descriptor: MemoryDescriptor::EMPTY,
        owner: PageOwner::Initial,
    };

    fn with_range(self, start: u64, end: u64) -> Self {
        Self {
            descriptor: self.descriptor.clipped(start, end),
            owner: self.owner,
        }
    }

    fn can_join(self, following: Self) -> bool {
        // Each pool allocation must remain a separate descriptor.
        self.owner != PageOwner::Pool
            && self.owner == following.owner
            && self.descriptor.memory_type == following.descriptor.memory_type
            && self.descriptor.attribute == following.descriptor.attribute
            && self.descriptor.end() == following.descriptor.physical_start
    }
}

fn allocation_attributes(memory_type: MemoryType) -> Result<u64, PageError> {
    match memory_type {
        MemoryType::RUNTIME_SERVICES_CODE | MemoryType::RUNTIME_SERVICES_DATA => Ok(WB | RUNTIME),
        MemoryType::MEMORY_MAPPED_IO | MemoryType::PERSISTENT | MemoryType::UNACCEPTED => {
            Err(PageError::InvalidType)
        }
        _ if (MemoryType::MAX_STANDARD..MemoryType::OEM_MIN).contains(&memory_type.raw()) => {
            Err(PageError::InvalidType)
        }
        _ => Ok(WB),
    }
}

fn is_free(descriptor: MemoryDescriptor, owner: PageOwner) -> bool {
    // Firmware may allocate a descriptor with the same type as free RAM.
    // The ownership tag keeps it out of the next allocation search.
    owner == PageOwner::Initial && descriptor.memory_type == MemoryType::CONVENTIONAL.raw()
}

fn byte_count(count: u64) -> Result<u64, PageError> {
    count
        .checked_mul(PAGE_BYTES)
        .filter(|&bytes| bytes != 0 && bytes <= PHYSICAL_END)
        .ok_or(PageError::InvalidPageCount)
}

fn address_range(address: u64, bytes: u64) -> Result<Range<u64>, PageError> {
    if address & (PAGE_BYTES - 1) != 0 {
        return Err(PageError::InvalidAddress);
    }
    let end = address
        .checked_add(bytes)
        .filter(|&end| end <= PHYSICAL_END)
        .ok_or(PageError::InvalidAddress)?;
    Ok(address..end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory(kind: MemoryType, start: u64, end: u64) -> MemoryDescriptor {
        MemoryDescriptor::for_range(kind, start, end, WB)
    }

    fn initial_map<'a>(
        storage: &'a mut [MemoryDescriptor],
        initial: &[MemoryDescriptor],
    ) -> MemoryMap<'a> {
        storage[..initial.len()].copy_from_slice(initial);
        MemoryMap {
            storage,
            used: initial.len(),
            map_key: 1,
        }
    }

    fn snapshot(
        allocator: &PageAllocator<'_, '_>,
    ) -> (Vec<MemoryDescriptor>, Vec<PageOwner>, usize, u64) {
        (
            allocator.map.storage.to_vec(),
            allocator.owners.to_vec(),
            allocator.map.used,
            allocator.map.key(),
        )
    }

    #[test]
    fn all_placement_modes_use_complete_pages_and_contiguous_conventional_ranges() {
        let mut records = [MemoryDescriptor::EMPTY; 12];
        let mut tags = [PageOwner::Pages; 12];
        let map = initial_map(
            &mut records,
            &[
                memory(MemoryType::CONVENTIONAL, 0x1000, 0x3000),
                memory(MemoryType::CONVENTIONAL, 0x3000, 0x5000),
                memory(MemoryType::MEMORY_MAPPED_IO, 0x5000, 0x6000),
                memory(MemoryType::CONVENTIONAL, 0x7000, 0xd000),
            ],
        );
        let mut allocator = PageAllocator::new(map, &mut tags).unwrap();
        assert_eq!(
            allocator.allocate_pages(AllocateAt::Any, MemoryType::LOADER_DATA, 1),
            Ok(0xc000)
        );
        assert_eq!(allocator.map().key(), 2);
        assert_eq!(
            allocator.allocate_pages(
                AllocateAt::MaxAddress(0x4ffe),
                MemoryType::RUNTIME_SERVICES_DATA,
                3,
            ),
            Ok(0x1000)
        );
        assert_eq!(
            allocator.map().descriptors()[0],
            MemoryDescriptor::for_range(
                MemoryType::RUNTIME_SERVICES_DATA,
                0x1000,
                0x4000,
                WB | RUNTIME,
            )
        );
        assert_eq!(
            allocator.allocate_pages(
                AllocateAt::Address(0x8000),
                MemoryType::BOOT_SERVICES_DATA,
                2
            ),
            Ok(0x8000)
        );
        assert_eq!(allocator.map().key(), 4);
        allocator.free_pages(0x1000, 3).unwrap();
        assert_eq!(
            allocator.map().descriptors()[0],
            memory(MemoryType::CONVENTIONAL, 0x1000, 0x5000),
        );
        assert_eq!(allocator.map().key(), 5);
    }

    #[test]
    fn partial_free_can_span_allocations_but_cannot_touch_firmware_reservations() {
        let mut records = [MemoryDescriptor::EMPTY; 10];
        let mut tags = [PageOwner::Pages; 10];
        let initial = [
            memory(MemoryType::BOOT_SERVICES_DATA, 0x1000, 0x2000),
            memory(MemoryType::CONVENTIONAL, 0x2000, 0xa000),
        ];
        let map = initial_map(&mut records, &initial);
        let mut allocator = PageAllocator::new(map, &mut tags).unwrap();
        allocator
            .allocate_pages(
                AllocateAt::Address(0x2000),
                MemoryType::BOOT_SERVICES_DATA,
                3,
            )
            .unwrap();
        allocator
            .allocate_pages(AllocateAt::Address(0x5000), MemoryType::LOADER_DATA, 4)
            .unwrap();
        assert_eq!(allocator.map().descriptors()[0], initial[0]);

        let before = snapshot(&allocator);
        assert_eq!(allocator.free_pages(0x1000, 2), Err(PageError::NotFound));
        assert_eq!(snapshot(&allocator), before);
        allocator.free_pages(0x4000, 3).unwrap();
        assert_eq!(
            allocator.map().descriptors()[1..4],
            [
                memory(MemoryType::BOOT_SERVICES_DATA, 0x2000, 0x4000),
                memory(MemoryType::CONVENTIONAL, 0x4000, 0x7000),
                memory(MemoryType::LOADER_DATA, 0x7000, 0x9000),
            ]
        );
        let before = snapshot(&allocator);
        assert_eq!(allocator.free_pages(0x4000, 1), Err(PageError::NotFound));
        assert_eq!(snapshot(&allocator), before);
        allocator.free_pages(0x2000, 2).unwrap();
        allocator.free_pages(0x7000, 2).unwrap();
        assert_eq!(allocator.map().descriptors(), initial);
        assert_eq!(allocator.map().key(), 6);
    }

    #[test]
    fn capacity_and_map_key_are_checked_before_either_buffer_is_changed() {
        let mut records = [MemoryDescriptor::EMPTY; 2];
        let mut too_few_tags = [PageOwner::Pages; 1];
        let map = initial_map(
            &mut records,
            &[memory(MemoryType::CONVENTIONAL, 0x1000, 0x5000)],
        );
        assert!(matches!(
            PageAllocator::new(map, &mut too_few_tags),
            Err(PageError::OutOfDescriptors)
        ));
        assert_eq!(too_few_tags, [PageOwner::Pages; 1]);

        let mut tags = [PageOwner::Pages; 2];
        let map = initial_map(
            &mut records,
            &[memory(MemoryType::CONVENTIONAL, 0x1000, 0x5000)],
        );
        let mut allocator = PageAllocator::new(map, &mut tags).unwrap();
        let before = snapshot(&allocator);
        assert_eq!(
            allocator.allocate_pages(AllocateAt::Address(0x2000), MemoryType::LOADER_DATA, 1),
            Err(PageError::OutOfDescriptors),
        );
        assert_eq!(snapshot(&allocator), before);

        allocator
            .allocate_pages(AllocateAt::Any, MemoryType::LOADER_DATA, 4)
            .unwrap();
        let before = snapshot(&allocator);
        assert_eq!(
            allocator.free_pages(0x2000, 1),
            Err(PageError::OutOfDescriptors)
        );
        assert_eq!(snapshot(&allocator), before);

        allocator.map.map_key = u64::MAX;
        let before = snapshot(&allocator);
        assert_eq!(
            allocator.free_pages(0x1000, 4),
            Err(PageError::MapKeyExhausted)
        );
        assert_eq!(snapshot(&allocator), before);
    }

    #[test]
    fn invalid_addresses_and_types_preserve_the_map_and_gaps() {
        let mut records = [MemoryDescriptor::EMPTY; 6];
        let mut tags = [PageOwner::Initial; 6];
        let map = initial_map(
            &mut records,
            &[
                memory(MemoryType::CONVENTIONAL, 0x1000, 0x3000),
                memory(MemoryType::CONVENTIONAL, 0x4000, 0x6000),
                memory(
                    MemoryType::CONVENTIONAL,
                    PHYSICAL_END - PAGE_BYTES * 2,
                    PHYSICAL_END,
                ),
            ],
        );
        let mut allocator = PageAllocator::new(map, &mut tags).unwrap();
        let before = snapshot(&allocator);
        for (mode, memory_type, count, expected) in [
            (
                AllocateAt::Any,
                MemoryType::RESERVED,
                0,
                PageError::InvalidPageCount,
            ),
            (
                AllocateAt::Any,
                MemoryType::RESERVED,
                u64::MAX,
                PageError::InvalidPageCount,
            ),
            (
                AllocateAt::Address(u64::MAX),
                MemoryType::RESERVED,
                1,
                PageError::InvalidAddress,
            ),
            (
                AllocateAt::Address(PHYSICAL_END),
                MemoryType::RESERVED,
                1,
                PageError::InvalidAddress,
            ),
            (
                AllocateAt::Address(0x1001),
                MemoryType::RESERVED,
                1,
                PageError::InvalidAddress,
            ),
            (
                AllocateAt::Address(0x2000),
                MemoryType::LOADER_DATA,
                3,
                PageError::NotFound,
            ),
            (
                AllocateAt::MaxAddress(0x1ffe),
                MemoryType::LOADER_DATA,
                1,
                PageError::NotFound,
            ),
            (
                AllocateAt::Any,
                MemoryType::PERSISTENT,
                1,
                PageError::InvalidType,
            ),
        ] {
            assert_eq!(
                allocator.allocate_pages(mode, memory_type, count),
                Err(expected)
            );
            assert_eq!(snapshot(&allocator), before);
        }
        assert_eq!(
            allocator.allocate_pages(AllocateAt::MaxAddress(u64::MAX), MemoryType::LOADER_DATA, 1),
            Ok(PHYSICAL_END - PAGE_BYTES)
        );
    }
}
