//! Transactional descriptor placement for a loader-owned image span.

use super::{
    AllocateAt, Entry, MemoryDescriptor, MemoryType, PAGE_BYTES, PHYSICAL_END, PageAllocator,
    PageError, PageOwner, address_range, allocation_attributes, byte_count,
};
use core::ops::Range;

/// Where an image may be placed when its ImageBase is not free.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ImagePlacement {
    /// Only at ImageBase. ImageBase 0 is never available (null page).
    Pinned,
    /// Any aligned range; the highest free one.
    Highest,
    /// Position-independent image with ImageBase 0 (Linux EFI stub): the lowest free
    /// range from `LOW_IMAGE_FLOOR`, aligned to `LOW_IMAGE_ALIGNMENT`.
    LowestAligned,
}

/// Nothing below 1 MiB is handed to a zero-based image: that area stays with the OS.
pub(crate) const LOW_IMAGE_FLOOR: u64 = 0x10_0000;
/// The kernel_alignment of an x86 Linux image (2 MiB).
pub(crate) const LOW_IMAGE_ALIGNMENT: u64 = 0x20_0000;

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
        placement: ImagePlacement,
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
        // ImageBase 0 is not a placement request for any image: nothing goes on the
        // null page even when the map reports it free. A pinned image then has no
        // valid range and the caller reports EFI_OUT_OF_RESOURCES.
        let preferred_fit = if preferred == 0 {
            Err(PageError::InvalidAddress)
        } else if preferred.is_multiple_of(alignment) {
            self.plan_allocation(AllocateAt::Address(preferred), bytes)
        } else {
            Err(PageError::InvalidAddress)
        };
        let (covered, span) = match preferred_fit {
            Ok(placement) => placement,
            Err(PageError::NotFound | PageError::InvalidAddress)
                if placement == ImagePlacement::LowestAligned =>
            {
                let step = alignment.max(LOW_IMAGE_ALIGNMENT);
                let base = self.find_lowest_fit(bytes, LOW_IMAGE_FLOOR, step)?;
                self.plan_allocation(AllocateAt::Address(base), bytes)?
            }
            Err(PageError::NotFound | PageError::InvalidAddress)
                if placement == ImagePlacement::Highest =>
            {
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
            allocator.plan_image(0x140000000, ImagePlacement::Pinned, 0x10000, &regions),
            Err(PageError::NotFound)
        ));
        let plan = allocator
            .plan_image(0x140000000, ImagePlacement::Highest, 0x10000, &regions)
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
            allocator.plan_image(0x70000, ImagePlacement::Pinned, PAGE_BYTES, &partial),
            Err(PageError::OutOfDescriptors)
        ));
        assert!(matches!(
            allocator.plan_image(0x70000, ImagePlacement::Pinned, PAGE_BYTES, &full),
            Err(PageError::MapKeyExhausted)
        ));
        assert_eq!(allocator.map().descriptors(), [before]);
        assert_eq!(allocator.map().key(), u64::MAX - 1);
        allocator.map.map_key -= 1;
        let prepared = allocator
            .plan_image(0x70000, ImagePlacement::Pinned, PAGE_BYTES, &full)
            .unwrap();
        allocator.commit_image(prepared);
        allocator.release_image(0x70000, 4).unwrap();
        assert_eq!(allocator.map().descriptors(), [before]);
        assert_eq!(allocator.map().key(), u64::MAX);
    }

    fn code_range(pages: u64) -> [ImageMemoryRange; 1] {
        [ImageMemoryRange {
            pages,
            memory_type: MemoryType::LOADER_CODE,
        }]
    }

    #[test]
    fn image_base_zero_is_never_placed_on_the_null_page() {
        let mut descriptors = [MemoryDescriptor::EMPTY; 8];
        let mut tags = [PageOwner::Initial; 8];
        // Page zero is free in this map.
        let map = MemoryMap::test_conventional(&mut descriptors, 0, 0x98000);
        let allocator = PageAllocator::new(map, &mut tags).unwrap();
        let ranges = code_range(4);
        let highest = allocator.plan_image(0, ImagePlacement::Highest, PAGE_BYTES, &ranges);
        assert_eq!(highest.unwrap().span, 0x94000..0x98000);
        let aligned = allocator.plan_image(0, ImagePlacement::Highest, 0x10000, &ranges);
        assert_eq!(aligned.unwrap().span, 0x90000..0x94000);
        // A pinned image with ImageBase 0 has no range at all: LoadImage then
        // returns EFI_OUT_OF_RESOURCES instead of reaching the null-page check.
        assert!(matches!(
            allocator.plan_image(0, ImagePlacement::Pinned, PAGE_BYTES, &ranges),
            Err(PageError::InvalidAddress)
        ));
        // A pinned image keeps the exact address it asked for, or fails.
        let pinned = allocator.plan_image(0x2000, ImagePlacement::Pinned, PAGE_BYTES, &ranges);
        assert_eq!(pinned.unwrap().span, 0x2000..0x6000);
        assert!(matches!(
            allocator.plan_image(0x2000_0000, ImagePlacement::Pinned, PAGE_BYTES, &ranges),
            Err(PageError::NotFound)
        ));
    }

    /// 128 MiB board map of FIRMWARE_BOOT.md section 2: 1 MiB of low RAM, handoff
    /// and boot stack up to 0x150000, the 4 MiB firmware shadow at 0x200000, then
    /// boot page tables (N = 0x50 pages here; the real N is board dependent).
    fn board_128_mib<'a, 'b>(
        descriptors: &'a mut [MemoryDescriptor],
        tags: &'b mut [PageOwner],
    ) -> PageAllocator<'a, 'b> {
        let map = MemoryMap::test_conventional(descriptors, 0, 0x800_0000);
        let mut allocator = PageAllocator::new(map, tags).unwrap();
        for (address, pages, memory_type) in [
            (0x10_0000, 0x50, MemoryType::RESERVED),
            (0x20_0000, 0x400, MemoryType::BOOT_SERVICES_CODE),
            (0x60_0000, 0x50, MemoryType::BOOT_SERVICES_DATA),
        ] {
            allocator
                .allocate_pages(AllocateAt::Address(address), memory_type, pages)
                .unwrap();
        }
        allocator
    }

    #[test]
    fn zero_based_position_independent_image_takes_the_lowest_2_mib_aligned_range() {
        let mut descriptors = [MemoryDescriptor::EMPTY; 16];
        let mut tags = [PageOwner::Initial; 16];
        let allocator = board_128_mib(&mut descriptors, &mut tags);
        // The Ubuntu 7.0.0-38 EFI stub: SizeOfImage 0x10f0000, section alignment 0x1000.
        let ranges = code_range(0x10f0);
        let plan = allocator
            .plan_image(0, ImagePlacement::LowestAligned, PAGE_BYTES, &ranges)
            .unwrap();
        // Page 0 is free but below 1 MiB; 0x150000..0x200000 is too small and not
        // 2 MiB aligned; the shadow and tables occupy up to 0x650000.
        assert_eq!(plan.span, 0x80_0000..0x18f_0000);
        assert!(plan.span.start >= LOW_IMAGE_FLOOR);
        assert_eq!(plan.span.start % LOW_IMAGE_ALIGNMENT, 0);
        // The kernel decompressor still finds its working window above the image.
        let window = plan.span.end.next_multiple_of(LOW_IMAGE_ALIGNMENT);
        assert_eq!(window, 0x1a0_0000);
        assert!(window >= 0x100_0000);
        assert_eq!(0x800_0000 - window, 0x660_0000);
        assert!(0x800_0000 - window >= 0x44c_1000);
        // An image with a relocation directory keeps the highest placement.
        let highest = allocator
            .plan_image(0, ImagePlacement::Highest, PAGE_BYTES, &ranges)
            .unwrap();
        assert_eq!(highest.span, 0x800_0000 - 0x10f_0000..0x800_0000);
    }

    #[test]
    fn lowest_placement_fails_without_a_big_enough_aligned_range() {
        let mut descriptors = [MemoryDescriptor::EMPTY; 16];
        let mut tags = [PageOwner::Initial; 16];
        let allocator = board_128_mib(&mut descriptors, &mut tags);
        assert!(matches!(
            allocator.plan_image(
                0,
                ImagePlacement::LowestAligned,
                PAGE_BYTES,
                &code_range(0x8000)
            ),
            Err(PageError::NotFound)
        ));
    }
}
