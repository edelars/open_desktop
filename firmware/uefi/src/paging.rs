//! Builds the firmware's four-level identity map in a caller-supplied RAM reservation.

use crate::handoff::{Handoff, region_kind::*};

mod active;
pub(crate) use active::ActivePageTables;

pub const PAGE_BYTES: u64 = 4096;
const ENTRIES: usize = 512;
const STACK_BYTES: u64 = 256 * 1024;
const LOW_RAM_END: u64 = 1024 * 1024;
const PRESENT: u64 = 1;
const WRITABLE: u64 = 1 << 1;
const NO_EXECUTE: u64 = 1 << 63;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PagingError {
    MissingBootPageTables,
    NoContiguousRam,
    MissingFirmwareShadow,
    InvalidShadowBoundaries,
    WrongStorageLength,
    TableCapacity,
    UnknownRegionKind(u32),
}

impl PagingError {
    pub fn stop_detail(self) -> u64 {
        match self {
            Self::MissingBootPageTables => 1,
            Self::NoContiguousRam => 2,
            Self::MissingFirmwareShadow => 3,
            Self::InvalidShadowBoundaries => 4,
            Self::WrongStorageLength => 5,
            Self::TableCapacity => 6,
            Self::UnknownRegionKind(kind) => 7 | (u64::from(kind) << 32),
        }
    }
}

/// Placement determined before an operand byte or page-table entry is written.
pub struct BootPaging<'view, 'bytes> {
    handoff: &'view Handoff<'bytes>,
    root: u64,
    table_bytes: u64,
}

/// Page-aligned starts of the read-only data and writable portions of the shadow image.
/// The image header shares the first executable page with the entry point.
#[derive(Clone, Copy)]
pub struct ShadowSections {
    pub read_only_start: u64,
    pub writable_start: u64,
}

impl<'view, 'bytes> BootPaging<'view, 'bytes> {
    /// The caller supplies a handoff returned by `Handoff::parse_for_boot`.
    pub fn plan(handoff: &'view Handoff<'bytes>) -> Result<Self, PagingError> {
        let table_bytes = handoff
            .regions()
            .find(|region| region.kind == BOOT_PAGE_TABLES)
            .ok_or(PagingError::MissingBootPageTables)?
            .length;
        let reservation_bytes = table_bytes + PAGE_BYTES + STACK_BYTES;
        let root = handoff
            .regions()
            .filter(|region| region.kind == RAM)
            .find_map(|region| {
                let start = region.base.max(LOW_RAM_END);
                (start + reservation_bytes <= region.base + region.length).then_some(start)
            })
            .ok_or(PagingError::NoContiguousRam)?;
        Ok(Self {
            handoff,
            root,
            table_bytes,
        })
    }

    pub fn root(&self) -> u64 {
        self.root
    }

    pub fn table_words(&self) -> usize {
        // The handoff limits the physical address space to 36 bits. Both firmware
        // and its host tests use a 64-bit target.
        (self.table_bytes / 8) as usize
    }

    pub fn guard_page(&self) -> u64 {
        self.root + self.table_bytes
    }

    pub fn stack_top(&self) -> u64 {
        self.guard_page() + PAGE_BYTES + STACK_BYTES
    }

    pub(crate) fn handoff(&self) -> &Handoff<'bytes> {
        self.handoff
    }

    pub(crate) fn validate_shadow_sections(
        &self,
        shadow: ShadowSections,
    ) -> Result<(), PagingError> {
        let image = self
            .handoff
            .regions()
            .find(|region| region.kind == FIRMWARE_SHADOW)
            .ok_or(PagingError::MissingFirmwareShadow)?;
        if shadow.read_only_start % PAGE_BYTES != 0
            || shadow.writable_start % PAGE_BYTES != 0
            || shadow.read_only_start <= image.base
            || shadow.writable_start < shadow.read_only_start
            || shadow.writable_start > image.base + image.length
        {
            return Err(PagingError::InvalidShadowBoundaries);
        }
        Ok(())
    }

    /// The supplied storage must represent exactly the planned range at `root()`.
    /// A failure can leave partially prepared entries. The caller must keep the current CR3.
    pub fn build(&self, words: &mut [u64], shadow: ShadowSections) -> Result<usize, PagingError> {
        if words.len() != self.table_words() {
            return Err(PagingError::WrongStorageLength);
        }
        self.validate_shadow_sections(shadow)?;

        let mut tables = Tables {
            words,
            root: self.root,
            used_pages: 0,
        };
        tables.allocate()?;
        for region in self.handoff.regions() {
            let base_flags = match region.kind {
                RESERVED => continue,
                ROM | HANDOFF => PRESENT | NO_EXECUTE,
                RAM | MMIO | NVRAM | FRAMEBUFFER | BOOT_STACK | BOOT_PAGE_TABLES | ACPI_TABLES
                | ACPI_NVS | FIRMWARE_SHADOW => PRESENT | WRITABLE | NO_EXECUTE,
                kind => return Err(PagingError::UnknownRegionKind(kind)),
            };
            let mut page = region.base;
            let end = region.base + region.length;
            while page < end {
                if page != self.guard_page() {
                    let flags = if region.kind == FIRMWARE_SHADOW && page < shadow.read_only_start {
                        PRESENT
                    } else if region.kind == FIRMWARE_SHADOW && page < shadow.writable_start {
                        PRESENT | NO_EXECUTE
                    } else {
                        base_flags
                    };
                    tables.map(page, flags)?;
                }
                page += PAGE_BYTES;
            }
        }
        Ok(tables.used_pages)
    }
}

struct Tables<'a> {
    words: &'a mut [u64],
    root: u64,
    used_pages: usize,
}

impl Tables<'_> {
    fn allocate(&mut self) -> Result<usize, PagingError> {
        if self.used_pages == self.words.len() / ENTRIES {
            return Err(PagingError::TableCapacity);
        }
        let next = self.used_pages;
        let start = next * ENTRIES;
        self.words[start..start + ENTRIES].fill(0);
        self.used_pages += 1;
        Ok(next)
    }

    fn map(&mut self, page: u64, flags: u64) -> Result<(), PagingError> {
        let mut table = 0;
        for shift in [39, 30, 21] {
            let slot = table * ENTRIES + ((page >> shift) as usize & (ENTRIES - 1));
            let entry = self.words[slot];
            if entry == 0 {
                let next = self.allocate()?;
                let address = self.root + next as u64 * PAGE_BYTES;
                self.words[slot] = address | PRESENT | WRITABLE;
                table = next;
            } else {
                let address = entry & !(PAGE_BYTES - 1);
                table = ((address - self.root) / PAGE_BYTES) as usize;
            }
        }
        let slot = table * ENTRIES + ((page >> 12) as usize & (ENTRIES - 1));
        self.words[slot] = page | flags;
        Ok(())
    }
}
