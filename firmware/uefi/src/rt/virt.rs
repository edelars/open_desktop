//! UEFI virtual mappings of the runtime ranges in the final physical memory map.

use core::{marker::PhantomData, mem::size_of, ptr};

use crate::{
    mem::map::{BOOT_DESCRIPTOR_CAPACITY, DESCRIPTOR_VERSION, MemoryDescriptor, RUNTIME},
    paging::PAGE_BYTES,
    pe::RuntimeFixup,
    uefi::status::Status,
};

pub const OPTIONAL_POINTER: usize = 1;
pub(crate) const MAX_IMAGES: usize = 64;

#[repr(C)]
pub(crate) struct RuntimeRanges {
    count: usize,
    descriptors: [MemoryDescriptor; BOOT_DESCRIPTOR_CAPACITY],
}

impl RuntimeRanges {
    pub(crate) const fn new() -> Self {
        Self {
            count: 0,
            descriptors: [MemoryDescriptor::EMPTY; BOOT_DESCRIPTOR_CAPACITY],
        }
    }

    pub(crate) fn capture(&mut self, descriptors: &[MemoryDescriptor]) {
        self.count = 0;
        for &desc in descriptors {
            if desc.attribute & RUNTIME != 0 {
                self.descriptors[self.count] = desc;
                self.count += 1;
            }
        }
    }

    #[cfg(target_os = "uefi")]
    pub(super) fn descriptors(&self) -> &[MemoryDescriptor] {
        &self.descriptors[..self.count]
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct RuntimeImage {
    pub(crate) base: u64,
    pub(crate) size: u64,
    pub(crate) fixups: *const RuntimeFixup,
    pub(crate) fixup_count: usize,
}

impl RuntimeImage {
    pub(crate) const EMPTY: Self = Self {
        base: 0,
        size: 0,
        fixups: ptr::null(),
        fixup_count: 0,
    };
}

#[repr(transparent)]
pub(crate) struct RuntimeImages(pub(crate) [RuntimeImage; MAX_IMAGES]);

impl RuntimeImages {
    pub(crate) const fn new() -> Self {
        Self([RuntimeImage::EMPTY; MAX_IMAGES])
    }
}

/// A validated, borrowed view for use only during SetVirtualAddressMap.
pub struct VirtualMap<'a> {
    input: *const u8,
    count: usize,
    stride: usize,
    _input_lifetime: PhantomData<&'a [u8]>,
}

impl<'a> VirtualMap<'a> {
    /// # Safety
    /// The caller provides readable, stable descriptor storage for `'a`.
    /// Each version-1 field must be initialized. Padding and extension bytes
    /// need only be readable as part of the allocation and are never loaded.
    pub unsafe fn new(
        input: *const MemoryDescriptor,
        size: usize,
        stride: usize,
        version: u32,
        final_map: &[MemoryDescriptor],
    ) -> Result<Self, Status> {
        if input.is_null()
            || stride < size_of::<MemoryDescriptor>()
            || version != DESCRIPTOR_VERSION
            || size > isize::MAX as usize
            || size % stride != 0
            || (input as usize).checked_add(size).is_none()
        {
            return Err(Status::INVALID_PARAMETER);
        }

        let result = Self {
            input: input.cast(),
            count: size / stride,
            stride,
            _input_lifetime: PhantomData,
        };
        for index in 0..result.count {
            let desc = result.read(index);
            if desc.attribute & RUNTIME == 0 {
                continue;
            }
            if !final_map.iter().any(|old| {
                old.attribute & RUNTIME != 0
                    && old.physical_start == desc.physical_start
                    && old.number_of_pages == desc.number_of_pages
                    && old.memory_type == desc.memory_type
            }) {
                return Err(Status::NOT_FOUND);
            }
            let length = desc
                .number_of_pages
                .checked_mul(PAGE_BYTES)
                .filter(|length| *length != 0)
                .ok_or(Status::INVALID_PARAMETER)?;
            let end = desc
                .virtual_start
                .checked_add(length - 1)
                .ok_or(Status::INVALID_PARAMETER)?;
            if desc.virtual_start == 0
                || desc.virtual_start % PAGE_BYTES != 0
                || !canonical(desc.virtual_start)
                || !canonical(end)
                || (desc.virtual_start ^ end) >> 47 != 0
            {
                return Err(Status::INVALID_PARAMETER);
            }
            for previous_index in 0..index {
                let prev = result.read(previous_index);
                if prev.attribute & RUNTIME == 0 {
                    continue;
                }
                let prev_end = prev.virtual_start + (prev.number_of_pages * PAGE_BYTES - 1);
                if prev.physical_start == desc.physical_start
                    || (desc.virtual_start <= prev_end && prev.virtual_start <= end)
                {
                    return Err(Status::INVALID_PARAMETER);
                }
            }
        }
        for desc in final_map
            .iter()
            .filter(|desc| desc.attribute & RUNTIME != 0)
        {
            result
                .translate(desc.physical_start)
                .map_err(|_| Status::NO_MAPPING)?;
        }
        Ok(result)
    }

    fn read(&self, index: usize) -> MemoryDescriptor {
        debug_assert!(index < self.count);
        // SAFETY: the constructor bounded the stride and count, and the caller
        // supplies stable storage with all version-1 fields initialized.
        unsafe {
            let desc = self
                .input
                .add(index * self.stride)
                .cast::<MemoryDescriptor>();
            let mut value = MemoryDescriptor::EMPTY;
            value.memory_type = ptr::addr_of!((*desc).memory_type).read_unaligned();
            value.physical_start = ptr::addr_of!((*desc).physical_start).read_unaligned();
            value.virtual_start = ptr::addr_of!((*desc).virtual_start).read_unaligned();
            value.number_of_pages = ptr::addr_of!((*desc).number_of_pages).read_unaligned();
            value.attribute = ptr::addr_of!((*desc).attribute).read_unaligned();
            value
        }
    }

    pub fn translate(&self, physical: u64) -> Result<u64, Status> {
        self.translate_range(physical, 1)
    }

    pub fn translate_range(&self, physical: u64, length: u64) -> Result<u64, Status> {
        if length == 0 {
            return Err(Status::INVALID_PARAMETER);
        }
        for index in 0..self.count {
            let desc = self.read(index);
            if desc.attribute & RUNTIME == 0 {
                continue;
            }
            if let Some(offset) = physical.checked_sub(desc.physical_start)
                && let Some(end) = offset.checked_add(length)
                && end <= desc.number_of_pages * PAGE_BYTES
            {
                return Ok(desc.virtual_start + offset);
            }
        }
        Err(Status::NOT_FOUND)
    }

    pub fn convert_pointer(&self, disposition: usize, physical: usize) -> Result<usize, Status> {
        if disposition & !OPTIONAL_POINTER != 0 {
            return Err(Status::INVALID_PARAMETER);
        }
        if physical == 0 {
            return if disposition & OPTIONAL_POINTER != 0 {
                Ok(0)
            } else {
                Err(Status::INVALID_PARAMETER)
            };
        }
        self.translate(physical as u64)
            .map(|address| address as usize)
    }
}

fn canonical(address: u64) -> bool {
    ((address as i64) << 16 >> 16) as u64 == address
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::map::MemoryType;

    const fn descriptor(physical: u64, pages: u64) -> MemoryDescriptor {
        let mut desc = MemoryDescriptor::EMPTY;
        desc.memory_type = MemoryType::RUNTIME_SERVICES_DATA.raw();
        desc.physical_start = physical;
        desc.number_of_pages = pages;
        desc.attribute = RUNTIME;
        desc
    }

    const fn with_virtual(mut desc: MemoryDescriptor, address: u64) -> MemoryDescriptor {
        desc.virtual_start = address;
        desc
    }

    const FINAL: [MemoryDescriptor; 2] = [descriptor(0x1000, 2), descriptor(0x8000, 1)];

    fn new(input: &[MemoryDescriptor]) -> Result<VirtualMap<'_>, Status> {
        // SAFETY: the slice owns stable, initialized descriptor fields.
        unsafe {
            VirtualMap::new(
                input.as_ptr(),
                core::mem::size_of_val(input),
                size_of::<MemoryDescriptor>(),
                DESCRIPTOR_VERSION,
                &FINAL,
            )
        }
    }

    #[test]
    fn converts_pointers_at_extended_stride_in_both_canonical_halves() {
        #[repr(C)]
        struct Extended {
            desc: MemoryDescriptor,
            extra: [u8; 8],
        }

        let descs = [
            Extended {
                desc: with_virtual(FINAL[1], 0x800_0000),
                extra: [0xff; 8],
            },
            Extended {
                desc: with_virtual(FINAL[0], 0xffff_ffff_ffff_e000),
                extra: [0; 8],
            },
        ];
        // SAFETY: descs has the stride and initialized fields supplied to the call.
        let map = unsafe {
            VirtualMap::new(
                descs.as_ptr().cast(),
                size_of::<[Extended; 2]>(),
                size_of::<Extended>(),
                DESCRIPTOR_VERSION,
                &FINAL,
            )
        }
        .unwrap();
        assert_eq!(map.translate(0x2fff), Ok(u64::MAX));
        assert_eq!(map.translate(0x8123), Ok(0x800_0123));
        assert_eq!(map.translate(0x3000), Err(Status::NOT_FOUND));
        assert_eq!(map.convert_pointer(OPTIONAL_POINTER, 0), Ok(0));
        assert_eq!(map.convert_pointer(0, 0), Err(Status::INVALID_PARAMETER));
        assert_eq!(
            map.convert_pointer(2, 0x8000),
            Err(Status::INVALID_PARAMETER)
        );
    }

    #[test]
    fn rejects_missing_unknown_overlapping_and_noncanonical_runtime_ranges() {
        let first = with_virtual(FINAL[0], 0x4000);
        assert_eq!(new(&[first]).err(), Some(Status::NO_MAPPING));
        assert_eq!(
            new(&[first, with_virtual(descriptor(0xb000, 1), 0xa000)]).err(),
            Some(Status::NOT_FOUND)
        );
        assert_eq!(
            new(&[first, with_virtual(FINAL[1], 0x5000)]).err(),
            Some(Status::INVALID_PARAMETER)
        );
        assert_eq!(new(&[first, first]).err(), Some(Status::INVALID_PARAMETER));
        let crossing_hole = with_virtual(FINAL[0], 0x0000_7fff_ffff_f000);
        assert_eq!(
            new(&[crossing_hole, with_virtual(FINAL[1], 0x8000)]).err(),
            Some(Status::INVALID_PARAMETER)
        );
    }
}
