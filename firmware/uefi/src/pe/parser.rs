//! Bounds-checked PE32+ image layout for the x64 UEFI loader.
//! Wire fields follow Microsoft PE Format, "PE Format" and "Section Table".

use core::ops::Range;

const DOS_HEADER_BYTES: usize = 64;
const PE_HEADER_BYTES: usize = 24;
const OPTIONAL_PREFIX_BYTES: usize = 112;
const SECTION_HEADER_BYTES: usize = 40;
const DIRECTORY_BYTES: usize = 8;
pub(super) const MAX_SECTIONS: usize = 96;
const COFF_AMD64: u16 = 0x8664;
const COFF_EXECUTABLE_IMAGE: u16 = 0x0002;
const PE32_PLUS: u16 = 0x020b;
const SECURITY_DIRECTORY: usize = 4;
const BASE_RELOCATION_DIRECTORY: usize = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Truncated,
    InvalidSignature,
    NotExecutable,
    UnsupportedFormat,
    UnsupportedMachine,
    UnsupportedSubsystem,
    InvalidOptionalHeader,
    InvalidAlignment,
    InvalidImageSize,
    InvalidHeaderSize,
    InvalidSectionCount,
    InvalidSectionFileRange,
    InvalidSectionImageRange,
    OverlappingSectionFileRanges,
    OverlappingSectionImageRanges,
    InvalidEntryPoint,
    InvalidBaseRelocationDirectory,
    InvalidCertificateDirectory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Subsystem {
    Application,
    BootServiceDriver,
    RuntimeDriver,
}

#[derive(Clone, Debug)]
pub struct PeImage<'a> {
    file: &'a [u8],
    section_table: &'a [[u8; SECTION_HEADER_BYTES]],
    subsystem: Subsystem,
    image_base: u64,
    image_size: u32,
    entry_rva: u32,
    headers_size: usize,
    checksum_file_offset: usize,
    security_entry_file_offset: Option<usize>,
    section_alignment: u32,
    file_alignment: u32,
    base_relocations: Option<Range<u32>>,
    certificates: Option<Range<usize>>,
}

#[derive(Clone, Copy, Debug)]
pub struct Section<'a> {
    pub virtual_address: u32,
    pub virtual_size: u32,
    pub characteristics: u32,
    pub file_offset: usize,
    pub file_data: &'a [u8],
}

#[derive(Clone, Copy)]
struct SectionHeader {
    virtual_size: u32,
    virtual_address: u32,
    raw_size: u32,
    raw_offset: u32,
    characteristics: u32,
}

impl SectionHeader {
    fn parse(bytes: &[u8; SECTION_HEADER_BYTES]) -> Self {
        Self {
            virtual_size: read_u32(bytes, 8),
            virtual_address: read_u32(bytes, 12),
            raw_size: read_u32(bytes, 16),
            raw_offset: read_u32(bytes, 20),
            characteristics: read_u32(bytes, 36),
        }
    }

    fn image_range(self) -> Option<Range<u32>> {
        let size = self.virtual_size.max(self.raw_size);
        Some(self.virtual_address..self.virtual_address.checked_add(size)?)
    }

    fn file_range(self) -> Option<Range<usize>> {
        let offset = self.raw_offset as usize;
        Some(offset..offset.checked_add(self.raw_size as usize)?)
    }
}

impl<'a> PeImage<'a> {
    pub fn parse(file: &'a [u8]) -> Result<Self, Error> {
        let dos = file.get(..DOS_HEADER_BYTES).ok_or(Error::Truncated)?;
        if !dos.starts_with(b"MZ") {
            return Err(Error::InvalidSignature);
        }
        let pe_start = read_u32(dos, 0x3c) as usize;
        if pe_start < DOS_HEADER_BYTES {
            return Err(Error::InvalidSignature);
        }
        let pe = file.get(pe_start..).ok_or(Error::Truncated)?;
        let coff = pe.get(..PE_HEADER_BYTES).ok_or(Error::Truncated)?;
        if !coff.starts_with(b"PE\0\0") {
            return Err(Error::InvalidSignature);
        }
        if read_u16(coff, 4) != COFF_AMD64 {
            return Err(Error::UnsupportedMachine);
        }
        if read_u16(coff, 22) & COFF_EXECUTABLE_IMAGE == 0 {
            return Err(Error::NotExecutable);
        }
        let section_count = usize::from(read_u16(coff, 6));
        if !(1..=MAX_SECTIONS).contains(&section_count) {
            return Err(Error::InvalidSectionCount);
        }
        let optional_size = usize::from(read_u16(coff, 20));
        let optional_end = PE_HEADER_BYTES + optional_size;
        let optional = pe
            .get(PE_HEADER_BYTES..optional_end)
            .ok_or(Error::Truncated)?;
        let magic = optional.get(..2).ok_or(Error::InvalidOptionalHeader)?;
        if read_u16(magic, 0) != PE32_PLUS {
            return Err(Error::UnsupportedFormat);
        }
        let directories = optional
            .get(OPTIONAL_PREFIX_BYTES..)
            .ok_or(Error::InvalidOptionalHeader)?;
        let directory_count = read_u32(optional, 108) as usize;
        let (directory_slots, _) = directories.as_chunks::<DIRECTORY_BYTES>();
        let directories = directory_slots
            .get(..directory_count)
            .ok_or(Error::InvalidOptionalHeader)?;
        let subsystem = match read_u16(optional, 68) {
            10 => Subsystem::Application,
            11 => Subsystem::BootServiceDriver,
            12 => Subsystem::RuntimeDriver,
            _ => return Err(Error::UnsupportedSubsystem),
        };
        let section_alignment = read_u32(optional, 32);
        let file_alignment = read_u32(optional, 36);
        if !(512..=65_536).contains(&file_alignment)
            || !file_alignment.is_power_of_two()
            || !section_alignment.is_power_of_two()
            || section_alignment < file_alignment
            || (section_alignment < 4096 && section_alignment != file_alignment)
        {
            return Err(Error::InvalidAlignment);
        }
        let image_base = read_u64(optional, 24);
        let image_size = read_u32(optional, 56);
        if !image_base.is_multiple_of(65_536)
            || image_size == 0
            || image_size > 0x8000_0000
            || image_size & (section_alignment - 1) != 0
            || image_base.checked_add(u64::from(image_size)).is_none()
        {
            return Err(Error::InvalidImageSize);
        }
        let headers_size = read_u32(optional, 60);
        let section_end = optional_end + section_count * SECTION_HEADER_BYTES;
        let table_end = pe_start
            .checked_add(section_end)
            .ok_or(Error::InvalidHeaderSize)?;
        if headers_size & (file_alignment - 1) != 0
            || (headers_size as usize) < table_end
            || (headers_size as usize) > file.len()
            || headers_size > image_size
        {
            return Err(Error::InvalidHeaderSize);
        }
        let (section_table, _) = pe[optional_end..section_end].as_chunks::<SECTION_HEADER_BYTES>();
        let entry_rva = read_u32(optional, 16);
        if entry_rva >= image_size {
            return Err(Error::InvalidEntryPoint);
        }
        let optional_start = pe_start + PE_HEADER_BYTES;
        let mut image = Self {
            file,
            section_table,
            subsystem,
            image_base,
            image_size,
            entry_rva,
            headers_size: headers_size as usize,
            checksum_file_offset: optional_start + 64,
            security_entry_file_offset: directories.get(SECURITY_DIRECTORY).map(|_| {
                optional_start + OPTIONAL_PREFIX_BYTES + SECURITY_DIRECTORY * DIRECTORY_BYTES
            }),
            section_alignment,
            file_alignment,
            base_relocations: None,
            certificates: None,
        };
        image.validate_sections(headers_size)?;
        image.base_relocations = image.validate_base_relocations(directories)?;
        image.certificates = image.validate_certificates(directories)?;
        Ok(image)
    }

    pub fn subsystem(&self) -> Subsystem {
        self.subsystem
    }

    pub fn image_base(&self) -> u64 {
        self.image_base
    }

    pub fn image_size(&self) -> u32 {
        self.image_size
    }

    pub fn entry_rva(&self) -> u32 {
        self.entry_rva
    }

    pub(super) fn file(&self) -> &'a [u8] {
        self.file
    }

    pub fn headers(&self) -> &'a [u8] {
        &self.file[..self.headers_size]
    }

    pub fn sections(&self) -> impl ExactSizeIterator<Item = Section<'a>> + '_ {
        self.section_headers().map(|header| {
            let file_data = if header.raw_size == 0 {
                &[][..]
            } else {
                &self.file[header.file_range().unwrap()]
            };
            Section {
                virtual_address: header.virtual_address,
                virtual_size: header.virtual_size,
                characteristics: header.characteristics,
                file_offset: header.raw_offset as usize,
                file_data,
            }
        })
    }

    pub fn checksum_file_offset(&self) -> usize {
        self.checksum_file_offset
    }

    pub fn security_entry_file_offset(&self) -> Option<usize> {
        self.security_entry_file_offset
    }

    pub fn base_relocations(&self) -> Option<Range<u32>> {
        self.base_relocations.clone()
    }

    pub fn certificate_file_range(&self) -> Option<Range<usize>> {
        self.certificates.clone()
    }

    fn section_headers(&self) -> impl ExactSizeIterator<Item = SectionHeader> + '_ {
        self.section_table.iter().map(SectionHeader::parse)
    }

    fn validate_sections(&self, headers_size: u32) -> Result<(), Error> {
        let section_mask = self.section_alignment - 1;
        let file_mask = self.file_alignment - 1;
        for (index, section) in self.section_headers().enumerate() {
            let image = section
                .image_range()
                .ok_or(Error::InvalidSectionImageRange)?;
            if image.is_empty()
                || image.start < headers_size
                || image.start & section_mask != 0
                || image.end > self.image_size
            {
                return Err(Error::InvalidSectionImageRange);
            }
            let raw = section.file_range().ok_or(Error::InvalidSectionFileRange)?;
            if !raw.is_empty()
                && (section.raw_offset & file_mask != 0
                    || section.raw_size & file_mask != 0
                    || section.raw_offset < headers_size
                    || raw.end > self.file.len()
                    || (self.section_alignment < 4096 && section.raw_offset != image.start))
            {
                return Err(Error::InvalidSectionFileRange);
            }

            for earlier in self.section_headers().take(index) {
                // Earlier entries have already passed both range checks.
                if overlaps(&image, &earlier.image_range().unwrap()) {
                    return Err(Error::OverlappingSectionImageRanges);
                }
                if section.raw_size != 0
                    && earlier.raw_size != 0
                    && overlaps(&raw, &earlier.file_range().unwrap())
                {
                    return Err(Error::OverlappingSectionFileRanges);
                }
            }
        }
        Ok(())
    }

    fn validate_base_relocations(
        &self,
        directories: &[[u8; DIRECTORY_BYTES]],
    ) -> Result<Option<Range<u32>>, Error> {
        let Some((start, size)) = directory(directories, BASE_RELOCATION_DIRECTORY) else {
            return Ok(None);
        };
        let end = directory_end(start, size).ok_or(Error::InvalidBaseRelocationDirectory)?;
        let in_file_backed_section = self.section_headers().any(|section| {
            section.virtual_address <= start && end <= section.virtual_address + section.raw_size
        });
        if !in_file_backed_section {
            return Err(Error::InvalidBaseRelocationDirectory);
        }
        Ok(Some(start..end))
    }

    fn validate_certificates(
        &self,
        directories: &[[u8; DIRECTORY_BYTES]],
    ) -> Result<Option<Range<usize>>, Error> {
        let Some((start, size)) = directory(directories, SECURITY_DIRECTORY) else {
            return Ok(None);
        };
        let end = directory_end(start, size).ok_or(Error::InvalidCertificateDirectory)?;
        let certificate = start as usize..end as usize;
        if !start.is_multiple_of(8)
            || !size.is_multiple_of(8)
            || certificate.start < self.headers_size
            || certificate.end > self.file.len()
            || self.section_headers().any(|section| {
                section.raw_size != 0 && overlaps(&certificate, &section.file_range().unwrap())
            })
        {
            return Err(Error::InvalidCertificateDirectory);
        }
        Ok(Some(certificate))
    }
}

fn directory(directories: &[[u8; DIRECTORY_BYTES]], index: usize) -> Option<(u32, u32)> {
    let fields = directories.get(index)?;
    let start = read_u32(fields, 0);
    let size = read_u32(fields, 4);
    (start != 0 || size != 0).then_some((start, size))
}

fn directory_end(start: u32, size: u32) -> Option<u32> {
    if start == 0 || size == 0 {
        return None;
    }
    start.checked_add(size)
}

fn overlaps<T: Ord>(left: &Range<T>, right: &Range<T>) -> bool {
    left.start < right.end && right.start < left.end
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..][..2].try_into().unwrap())
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..][..4].try_into().unwrap())
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..][..8].try_into().unwrap())
}

#[cfg(test)]
#[path = "parser_tests.rs"]
mod tests;
