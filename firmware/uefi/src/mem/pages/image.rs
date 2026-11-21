//! Transactional descriptor placement for a loader-owned image span.

use super::{
    AllocateAt, Entry, MemoryDescriptor, MemoryType, PAGE_BYTES, PHYSICAL_END, PageAllocator,
    PageError, PageOwner, address_range, allocation_attributes, byte_count,
};
use core::ops::Range;

#[derive(Clone, Copy)]
pub(crate) struct ImageMemoryRange {
    pub(crate) pages: u64,
    pub(crate) memory_type: MemoryType,
}

pub(crate) struct ImageMapPlan<'a> {
    pub(crate) span: Range<u64>,
    ranges: &'a [ImageMemoryRange],
    covered: Range<usize>,
    prefix: Option<Entry>,
    suffix: Option<Entry>,
    entries: usize,
    used: usize,
    key: u64,
}

impl PageAllocator<'_, '_> {
    pub(crate) fn plan_image<'a>(
        &self,
        preferred: u64,
        relocatable: bool,
        alignment: u64,
        ranges: &'a [ImageMemoryRange],
    ) -> Result<ImageMapPlan<'a>, PageError> {
        if alignment < PAGE_BYTES || !alignment.is_power_of_two() {
            return Err(PageError::InvalidAddress);
        }
        let mut pages = 0u64;
        let mut range_count = 0;
        let mut previous = None;
        for range in ranges {
            byte_count(range.pages)?;
            allocation_attributes(range.memory_type)?;
            pages = pages
                .checked_add(range.pages)
                .ok_or(PageError::InvalidPageCount)?;
            if previous != Some(range.memory_type) {
                range_count += 1;
                previous = Some(range.memory_type);
            }
        }
        let bytes = byte_count(pages)?;
        // ImageBase 0 on a movable image is not a placement request: never put
        // an image on the null page even when the map reports it free.
        let preferred_fit = if preferred == 0 && relocatable {
            Err(PageError::InvalidAddress)
        } else if preferred.is_multiple_of(alignment) {
            self.plan_allocation(AllocateAt::Address(preferred), bytes)
        } else {
            Err(PageError::InvalidAddress)
        };
        let (covered, span) = match preferred_fit {
            Ok(placement) => placement,
            Err(PageError::NotFound | PageError::InvalidAddress) if relocatable => {
                let base = if alignment == PAGE_BYTES {
                    self.find_highest_fit(bytes, PHYSICAL_END, core::convert::identity)?
                } else {
                    self.find_highest_fit(bytes, PHYSICAL_END, |address| {
                        address & !(alignment - 1)
                    })?
                };
                self.plan_allocation(AllocateAt::Address(base), bytes)?
            }
            Err(error) => return Err(error),
        };

        let first = self.entry(covered.start);
        let last = self.entry(covered.end - 1);
        let prefix = (first.descriptor.physical_start < span.start)
            .then(|| first.with_range(first.descriptor.physical_start, span.start));
        let suffix = (span.end < last.descriptor.end())
            .then(|| last.with_range(span.end, last.descriptor.end()));
        let entries = range_count + usize::from(prefix.is_some()) + usize::from(suffix.is_some());
        let used = self.map.used - covered.len() + entries;
        if used > self.map.storage.len() {
            return Err(PageError::OutOfDescriptors);
        }
        // Leave room for releasing the allocation with the next map mutation.
        if self.map.map_key.checked_add(2).is_none() {
            return Err(PageError::MapKeyExhausted);
        }
        Ok(ImageMapPlan {
            span,
            ranges,
            covered,
            prefix,
            suffix,
            entries,
            used,
            key: self.map.map_key + 1,
        })
    }

    /// Apply a plan from this allocator. The BSP keeps the map unchanged
    /// between planning and commit.
    pub(crate) fn commit_image(&mut self, plan: ImageMapPlan<'_>) {
        assert_eq!(self.map.map_key + 1, plan.key, "stale image map plan");
        let mut slots = self.move_tail(plan.covered, plan.entries, plan.used, plan.key);
        if let Some(entry) = plan.prefix {
            self.write_entry(slots.next().unwrap(), entry);
        }

        let mut base = plan.span.start;
        let mut next = None;
        let mut previous = None;
        for range in plan.ranges {
            if previous != Some(range.memory_type) {
                let index = slots.next().unwrap();
                let attribute =
                    allocation_attributes(range.memory_type).expect("preflighted image type");
                self.write_entry(
                    index,
                    Entry {
                        descriptor: MemoryDescriptor::for_range(
                            range.memory_type,
                            base,
                            base,
                            attribute,
                        ),
                        owner: PageOwner::Image,
                    },
                );
                previous = Some(range.memory_type);
                next = Some(index);
            }
            self.map.storage[next.unwrap()].number_of_pages += range.pages;
            base += range.pages * PAGE_BYTES;
        }
        if let Some(entry) = plan.suffix {
            self.write_entry(slots.next().unwrap(), entry);
        }
    }

    pub(crate) fn release_image(&mut self, base: u64, pages: u64) -> Result<(), PageError> {
        let span = address_range(base, byte_count(pages)?)?;
        let covered = self.covered_by(span.clone(), |index, _| {
            self.owners[index] == PageOwner::Image
        })?;
        self.release(covered, span)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::map::{MemoryMap, RUNTIME, WB};

    #[test]
    fn aligned_fallback_commits_typed_owned_pages_and_releases_the_whole_span() {
        let mut descriptors = [MemoryDescriptor::EMPTY; 8];
        let mut tags = [PageOwner::Initial; 8];
        let map = MemoryMap::test_conventional(&mut descriptors, 0x70000, 0x98000);
        let mut allocator = PageAllocator::new(map, &mut tags).unwrap();
        let data = ImageMemoryRange {
            pages: 1,
            memory_type: MemoryType::RUNTIME_SERVICES_DATA,
        };
        let code = ImageMemoryRange {
            pages: 1,
            memory_type: MemoryType::RUNTIME_SERVICES_CODE,
        };
        let regions = [data, data, code, data];
        assert!(matches!(
            allocator.plan_image(0x140000000, false, 0x10000, &regions),
            Err(PageError::NotFound)
        ));
        let plan = allocator
            .plan_image(0x140000000, true, 0x10000, &regions)
            .unwrap();
        assert_eq!(plan.span, 0x90000..0x94000);
        assert_eq!(allocator.map().key(), 1);
        allocator.commit_image(plan);
        assert_eq!(allocator.map().key(), 2);
        let map = allocator.map().descriptors();
        assert_eq!(map.len(), 5);
        assert_eq!(
            map[1],
            MemoryDescriptor::for_range(data.memory_type, 0x90000, 0x92000, WB | RUNTIME)
        );
        assert_eq!(
            map[2],
            MemoryDescriptor::for_range(code.memory_type, 0x92000, 0x93000, WB | RUNTIME)
        );
        assert_eq!(
            map[3],
            MemoryDescriptor::for_range(data.memory_type, 0x93000, 0x94000, WB | RUNTIME)
        );
        assert_eq!(allocator.free_pages(0x90000, 4), Err(PageError::NotFound));
        assert_eq!(
            allocator.release_image(0x90000, 5),
            Err(PageError::NotFound)
        );
        assert_eq!(allocator.map().key(), 2);
        allocator.release_image(0x90000, 4).unwrap();
        assert_eq!(allocator.map().key(), 3);
        assert_eq!(
            allocator.map().descriptors(),
            [MemoryDescriptor::for_range(
                MemoryType::CONVENTIONAL,
                0x70000,
                0x98000,
                WB
            )]
        );
    }

    #[test]
    fn lack_of_descriptor_or_release_key_capacity_has_no_side_effects() {
        let mut descriptors = [MemoryDescriptor::EMPTY; 1];
        let mut tags = [PageOwner::Initial; 1];
        let mut map = MemoryMap::test_conventional(&mut descriptors, 0x70000, 0x74000);
        map.map_key = u64::MAX - 1;
        let mut allocator = PageAllocator::new(map, &mut tags).unwrap();
        let full = [ImageMemoryRange {
            pages: 4,
            memory_type: MemoryType::LOADER_CODE,
        }];
        let partial = [ImageMemoryRange {
            pages: 1,
            memory_type: MemoryType::LOADER_CODE,
        }];
        let before = allocator.map().descriptors()[0];
        assert!(matches!(
            allocator.plan_image(0x70000, false, PAGE_BYTES, &partial),
            Err(PageError::OutOfDescriptors)
        ));
        assert!(matches!(
            allocator.plan_image(0x70000, false, PAGE_BYTES, &full),
            Err(PageError::MapKeyExhausted)
        ));
        assert_eq!(allocator.map().descriptors(), [before]);
        assert_eq!(allocator.map().key(), u64::MAX - 1);
        allocator.map.map_key -= 1;
        let prepared = allocator
            .plan_image(0x70000, false, PAGE_BYTES, &full)
            .unwrap();
        allocator.commit_image(prepared);
        allocator.release_image(0x70000, 4).unwrap();
        assert_eq!(allocator.map().descriptors(), [before]);
        assert_eq!(allocator.map().key(), u64::MAX);
    }

    #[test]
    fn image_base_zero_on_a_movable_image_never_lands_on_the_null_page() {
        let mut descriptors = [MemoryDescriptor::EMPTY; 8];
        let mut tags = [PageOwner::Initial; 8];
        let map = MemoryMap::test_conventional(&mut descriptors, 0, 0x98000);
        let allocator = PageAllocator::new(map, &mut tags).unwrap();
        let ranges = [ImageMemoryRange {
            pages: 4,
            memory_type: MemoryType::LOADER_CODE,
        }];
        // The aligned fallback is the highest fit, not page zero.
        let movable = allocator.plan_image(0, true, PAGE_BYTES, &ranges).unwrap();
        assert_eq!(movable.span, 0x94000..0x98000);
        let aligned = allocator.plan_image(0, true, 0x10000, &ranges).unwrap();
        assert_eq!(aligned.span, 0x90000..0x94000);
        // An image pinned to its base keeps the exact address it asked for.
        let pinned = allocator.plan_image(0, false, PAGE_BYTES, &ranges).unwrap();
        assert_eq!(pinned.span, 0..0x4000);
        assert!(matches!(
            allocator.plan_image(0x2000_0000, false, PAGE_BYTES, &ranges),
            Err(PageError::NotFound)
        ));
    }
}
