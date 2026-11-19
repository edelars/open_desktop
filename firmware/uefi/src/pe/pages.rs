//! Hardware page boundaries for a validated PE image.

use super::parser::MAX_SECTIONS;
use super::{PeImage, Section};
use crate::paging::PAGE_BYTES;

const SECTION_EXECUTE: u32 = 0x2000_0000;
const SECTION_WRITE: u32 = 0x8000_0000;
pub(crate) const MAX_REGIONS: usize = MAX_SECTIONS * 2 + 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Permissions {
    ReadOnly,
    ReadWrite,
    Execute,
    WriteExecute,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PageRun {
    pub(crate) first: u32,
    pub(crate) end: u32,
    pub(crate) permissions: Permissions,
}

impl Permissions {
    pub(crate) fn writable(self) -> bool {
        matches!(self, Self::ReadWrite | Self::WriteExecute)
    }

    pub(crate) fn executable(self) -> bool {
        matches!(self, Self::Execute | Self::WriteExecute)
    }
}

impl PageRun {
    const EMPTY: Self = Self {
        first: 0,
        end: 0,
        permissions: Permissions::ReadOnly,
    };
}

pub(crate) struct PageLayout {
    runs: [PageRun; MAX_REGIONS],
    used: usize,
}

impl PageLayout {
    /// Headers and holes are read-only. Sections that share a hardware page
    /// contribute their combined permissions.
    pub(crate) fn for_image(image: &PeImage<'_>) -> Option<Self> {
        let entry = u64::from(image.entry_rva());
        if !image.sections().any(|section| {
            let rva = u64::from(section.virtual_address);
            section.characteristics & SECTION_EXECUTE != 0
                && rva <= entry
                && entry < rva + extent(&section)
        }) {
            return None;
        }
        let pages = u64::from(image.image_size()).div_ceil(PAGE_BYTES) as u32;
        let mut layout = Self {
            runs: [PageRun::EMPTY; MAX_REGIONS],
            used: 0,
        };
        let mut page = 0;
        while page < pages {
            let mut end = pages;
            let mut flags = 0;
            for section in image.sections() {
                let size = extent(&section);
                if size == 0 {
                    continue;
                }
                let rva = u64::from(section.virtual_address);
                let first = (rva / PAGE_BYTES) as u32;
                let after = (rva + size).div_ceil(PAGE_BYTES) as u32;
                if first > page {
                    end = end.min(first);
                } else if after > page {
                    flags |= section.characteristics & (SECTION_EXECUTE | SECTION_WRITE);
                    end = end.min(after);
                }
            }

            let permissions = match flags {
                0 => Permissions::ReadOnly,
                SECTION_WRITE => Permissions::ReadWrite,
                SECTION_EXECUTE => Permissions::Execute,
                _ => Permissions::WriteExecute,
            };
            if layout.used > 0 && layout.runs[layout.used - 1].permissions == permissions {
                layout.runs[layout.used - 1].end = end;
            } else {
                // Each nonempty section adds at most two distinct boundaries.
                layout.runs[layout.used] = PageRun {
                    first: page,
                    end,
                    permissions,
                };
                layout.used += 1;
            }
            page = end;
        }
        Some(layout)
    }

    pub(crate) fn runs(&self) -> &[PageRun] {
        &self.runs[..self.used]
    }
}

fn extent(section: &Section<'_>) -> u64 {
    u64::from(section.virtual_size).max(section.file_data.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pe::test_image::{OPTIONAL_START, RELOCATIONS, SECTIONS, fixture, put_u32};

    #[test]
    fn header_and_reloc_stay_read_only_with_separate_code_and_bss() {
        let bytes = fixture(10);
        let image = PeImage::parse(&bytes).unwrap();
        let layout = PageLayout::for_image(&image).unwrap();
        assert_eq!(
            layout
                .runs()
                .iter()
                .map(|r| (r.first, r.end, r.permissions))
                .collect::<Vec<_>>(),
            [
                (0, 1, Permissions::ReadOnly),
                (1, 2, Permissions::Execute),
                (2, 3, Permissions::ReadOnly),
                (3, 4, Permissions::ReadWrite),
            ],
        );
    }

    #[test]
    fn sections_sharing_a_hardware_page_receive_combined_permissions() {
        let mut bytes = fixture(10);
        put_u32(&mut bytes, OPTIONAL_START + 16, 0x200);
        put_u32(&mut bytes, OPTIONAL_START + 32, 0x200);
        put_u32(&mut bytes, OPTIONAL_START + 56, 0xc00);
        put_u32(&mut bytes, SECTIONS + 12, 0x200);
        put_u32(&mut bytes, SECTIONS + 40 + 12, 0x400);
        put_u32(&mut bytes, SECTIONS + 80 + 12, 0x600);
        put_u32(&mut bytes, RELOCATIONS, 0x400);
        let image = PeImage::parse(&bytes).unwrap();
        let layout = PageLayout::for_image(&image).unwrap();
        assert_eq!(
            layout.runs(),
            [PageRun {
                first: 0,
                end: 1,
                permissions: Permissions::WriteExecute
            }]
        );

        put_u32(&mut bytes, OPTIONAL_START + 16, 0x400);
        let image = PeImage::parse(&bytes).unwrap();
        assert!(PageLayout::for_image(&image).is_none());
    }
}
