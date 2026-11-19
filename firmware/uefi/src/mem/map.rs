//! Initial EFI memory descriptors after activating the firmware's own page tables.

use crate::handoff::{MAX_HANDOFF_REGIONS, region_kind::*};
use crate::paging::{BootPaging, PAGE_BYTES, PagingError, ShadowSections};

// The shadow can add two splits and the boot RAM reservation can add four.
pub const MAX_INITIAL_DESCRIPTORS: usize = MAX_HANDOFF_REGIONS + 6;
pub const DESCRIPTOR_VERSION: u32 = 1;

const UC: u64 = 1;
pub(super) const WB: u64 = 1 << 3;
pub(super) const RUNTIME: u64 = 1 << 63;

/// The UEFI 2.11 §7.2.1 32-bit memory type, including OEM and OS loader values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct MemoryType(u32);

impl MemoryType {
    pub const RESERVED: Self = Self(0);
    pub const LOADER_CODE: Self = Self(1);
    pub const LOADER_DATA: Self = Self(2);
    pub const BOOT_SERVICES_CODE: Self = Self(3);
    pub const BOOT_SERVICES_DATA: Self = Self(4);
    pub const RUNTIME_SERVICES_CODE: Self = Self(5);
    pub const RUNTIME_SERVICES_DATA: Self = Self(6);
    pub const CONVENTIONAL: Self = Self(7);
    pub const UNUSABLE: Self = Self(8);
    pub const ACPI_RECLAIM: Self = Self(9);
    pub const ACPI_NVS: Self = Self(10);
    pub const MEMORY_MAPPED_IO: Self = Self(11);
    pub const MEMORY_MAPPED_IO_PORT_SPACE: Self = Self(12);
    pub const PAL_CODE: Self = Self(13);
    pub const PERSISTENT: Self = Self(14);
    pub const UNACCEPTED: Self = Self(15);
    pub const MAX_STANDARD: u32 = 16;
    pub const OEM_MIN: u32 = 0x7000_0000;

    pub const fn from_raw(value: u32) -> Self {
        Self(value)
    }

    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// UEFI 2.11 (section 7.2.3) x64 EFI_MEMORY_DESCRIPTOR, version 1.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct MemoryDescriptor {
    pub memory_type: u32,
    padding: u32,
    pub physical_start: u64,
    pub virtual_start: u64,
    pub number_of_pages: u64,
    pub attribute: u64,
}

impl MemoryDescriptor {
    pub const EMPTY: Self = Self {
        memory_type: 0,
        padding: 0,
        physical_start: 0,
        virtual_start: 0,
        number_of_pages: 0,
        attribute: 0,
    };

    pub(super) fn for_range(memory_type: MemoryType, start: u64, end: u64, attribute: u64) -> Self {
        Self {
            memory_type: memory_type.raw(),
            padding: 0,
            physical_start: start,
            virtual_start: 0,
            number_of_pages: (end - start) / PAGE_BYTES,
            attribute,
        }
    }

    pub(super) fn end(self) -> u64 {
        self.physical_start + self.number_of_pages * PAGE_BYTES
    }

    pub(super) fn clipped(self, start: u64, end: u64) -> Self {
        Self {
            physical_start: start,
            number_of_pages: (end - start) / PAGE_BYTES,
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryMapError {
    TablesNotActive,
    InvalidPlacement,
    Paging(PagingError),
    BufferTooSmall { required: usize },
}

pub struct MemoryMap<'a> {
    pub(super) storage: &'a mut [MemoryDescriptor],
    pub(super) used: usize,
    pub(super) map_key: u64,
}

impl<'a> MemoryMap<'a> {
    /// Validates all ranges and capacity before writing a single descriptor.
    /// The caller invokes this after the switch, passing the installed root.
    pub fn from_active_paging(
        paging: &BootPaging<'_, '_>,
        active_cr3: u64,
        shadow: ShadowSections,
        buffer: &'a mut [MemoryDescriptor],
    ) -> Result<Self, MemoryMapError> {
        if active_cr3 != paging.root() {
            return Err(MemoryMapError::TablesNotActive);
        }
        paging
            .validate_shadow_sections(shadow)
            .map_err(MemoryMapError::Paging)?;
        let mut required = 0;
        describe(paging, shadow, |_| required += 1)?;
        if required > buffer.len() {
            return Err(MemoryMapError::BufferTooSmall { required });
        }
        let mut next = 0;
        describe(paging, shadow, |descriptor| {
            buffer[next] = descriptor;
            next += 1;
        })?;
        Ok(Self {
            storage: buffer,
            used: required,
            map_key: 1,
        })
    }

    pub fn descriptors(&self) -> &[MemoryDescriptor] {
        &self.storage[..self.used]
    }

    pub const fn key(&self) -> u64 {
        self.map_key
    }

    #[cfg(test)]
    pub(crate) fn test_conventional(
        storage: &'a mut [MemoryDescriptor],
        start: u64,
        end: u64,
    ) -> Self {
        storage[0] = MemoryDescriptor::for_range(MemoryType::CONVENTIONAL, start, end, WB);
        Self {
            storage,
            used: 1,
            map_key: 1,
        }
    }
}

fn describe(
    paging: &BootPaging<'_, '_>,
    shadow: ShadowSections,
    mut emit: impl FnMut(MemoryDescriptor),
) -> Result<(), MemoryMapError> {
    let mut range = |start, end, memory_type: MemoryType, attribute| {
        if start < end {
            emit(MemoryDescriptor::for_range(
                memory_type,
                start,
                end,
                attribute,
            ));
        }
    };
    let root = paging.root();
    let guard = paging.guard_page();
    let stack_top = paging.stack_top();
    let mut placed = false;
    for region in paging.handoff().regions() {
        let start = region.base;
        let end = start + region.length;
        let (kind, attr) = match region.kind {
            RAM if start <= root && stack_top <= end => {
                range(start, root, MemoryType::CONVENTIONAL, WB);
                range(root, guard, MemoryType::BOOT_SERVICES_DATA, WB);
                range(guard, guard + PAGE_BYTES, MemoryType::RESERVED, 0);
                range(
                    guard + PAGE_BYTES,
                    stack_top,
                    MemoryType::BOOT_SERVICES_DATA,
                    WB,
                );
                range(stack_top, end, MemoryType::CONVENTIONAL, WB);
                placed = true;
                continue;
            }
            FIRMWARE_SHADOW => {
                range(
                    start,
                    shadow.read_only_start,
                    MemoryType::BOOT_SERVICES_CODE,
                    WB,
                );
                range(
                    shadow.read_only_start,
                    shadow.writable_start,
                    MemoryType::BOOT_SERVICES_DATA,
                    WB,
                );
                range(
                    shadow.writable_start,
                    end,
                    MemoryType::BOOT_SERVICES_DATA,
                    WB,
                );
                continue;
            }
            RAM | BOOT_PAGE_TABLES => (MemoryType::CONVENTIONAL, WB),
            BOOT_STACK => (MemoryType::BOOT_SERVICES_DATA, WB),
            HANDOFF => (MemoryType::RESERVED, WB),
            ROM => (MemoryType::RESERVED, UC),
            RESERVED => (MemoryType::RESERVED, 0),
            MMIO | FRAMEBUFFER => (MemoryType::MEMORY_MAPPED_IO, UC),
            NVRAM => (MemoryType::MEMORY_MAPPED_IO, UC | RUNTIME),
            ACPI_TABLES => (MemoryType::ACPI_RECLAIM, WB),
            ACPI_NVS => (MemoryType::ACPI_NVS, WB),
            kind => {
                return Err(MemoryMapError::Paging(PagingError::UnknownRegionKind(kind)));
            }
        };
        range(start, end, kind, attr);
    }
    if !placed {
        return Err(MemoryMapError::InvalidPlacement);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, offset_of, size_of};

    #[test]
    fn uefi_memory_descriptor_has_the_x64_v1_layout() {
        assert_eq!(DESCRIPTOR_VERSION, 1);
        assert_eq!(size_of::<MemoryDescriptor>(), 40);
        assert_eq!(align_of::<MemoryDescriptor>(), 8);
        assert_eq!(offset_of!(MemoryDescriptor, memory_type), 0);
        assert_eq!(offset_of!(MemoryDescriptor, padding), 4);
        assert_eq!(offset_of!(MemoryDescriptor, physical_start), 8);
        assert_eq!(offset_of!(MemoryDescriptor, virtual_start), 16);
        assert_eq!(offset_of!(MemoryDescriptor, number_of_pages), 24);
        assert_eq!(offset_of!(MemoryDescriptor, attribute), 32);
    }
}
