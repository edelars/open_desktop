use super::*;

const PE_START: usize = 0x80;
const OPTIONAL_START: usize = PE_START + PE_HEADER_BYTES;
const OPTIONAL_BYTES: usize = OPTIONAL_PREFIX_BYTES + 16 * DIRECTORY_BYTES;
const SECTIONS: usize = OPTIONAL_START + OPTIONAL_BYTES;
const RELOCATIONS: usize =
    OPTIONAL_START + OPTIONAL_PREFIX_BYTES + BASE_RELOCATION_DIRECTORY * DIRECTORY_BYTES;
const SECURITY: usize =
    OPTIONAL_START + OPTIONAL_PREFIX_BYTES + SECURITY_DIRECTORY * DIRECTORY_BYTES;

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..][..2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..][..4].copy_from_slice(&value.to_le_bytes());
}

fn fixture(subsystem: u16) -> Vec<u8> {
    let mut file = vec![0u8; 0x600];
    file[..2].copy_from_slice(b"MZ");
    put_u32(&mut file, 0x3c, PE_START as u32);
    file[PE_START..][..4].copy_from_slice(b"PE\0\0");
    put_u16(&mut file, PE_START + 4, COFF_AMD64);
    put_u16(&mut file, PE_START + 6, 3);
    put_u16(&mut file, PE_START + 20, OPTIONAL_BYTES as u16);
    put_u16(&mut file, PE_START + 22, COFF_EXECUTABLE_IMAGE);
    put_u16(&mut file, OPTIONAL_START, PE32_PLUS);
    put_u32(&mut file, OPTIONAL_START + 16, 0x1000);
    file[OPTIONAL_START + 24..][..8].copy_from_slice(&0x1_4000_0000u64.to_le_bytes());
    put_u32(&mut file, OPTIONAL_START + 32, 0x1000);
    put_u32(&mut file, OPTIONAL_START + 36, 0x200);
    put_u32(&mut file, OPTIONAL_START + 56, 0x4000);
    put_u32(&mut file, OPTIONAL_START + 60, 0x200);
    put_u16(&mut file, OPTIONAL_START + 68, subsystem);
    put_u32(&mut file, OPTIONAL_START + 108, 16);
    put_u32(&mut file, RELOCATIONS, 0x2000);
    put_u32(&mut file, RELOCATIONS + 4, 0x18);
    for (index, (virtual_size, rva, raw_size, raw_offset, flags)) in [
        (0x180, 0x1000, 0x200, 0x200, 0x6000_0020),
        (0x18, 0x2000, 0x200, 0x400, 0x4200_0040),
        (0x5d0, 0x3000, 0, 0, 0xc000_0080),
    ]
    .into_iter()
    .enumerate()
    {
        let offset = SECTIONS + index * SECTION_HEADER_BYTES;
        put_u32(&mut file, offset + 8, virtual_size);
        put_u32(&mut file, offset + 12, rva);
        put_u32(&mut file, offset + 16, raw_size);
        put_u32(&mut file, offset + 20, raw_offset);
        put_u32(&mut file, offset + 36, flags);
    }
    file[0x200] = 0xc3;
    file
}

#[test]
fn parses_all_three_efi_subsystems_from_an_unaligned_slice() {
    for (subsystem, expected) in [
        (10, Subsystem::Application),
        (11, Subsystem::BootServiceDriver),
        (12, Subsystem::RuntimeDriver),
    ] {
        let mut backing = vec![0x55];
        backing.extend(fixture(subsystem));
        let image = PeImage::parse(&backing[1..]).unwrap();
        assert_eq!(image.subsystem(), expected);
        assert_eq!(image.image_base(), 0x1_4000_0000);
        assert_eq!(image.image_size(), 0x4000);
        assert_eq!(image.entry_rva(), 0x1000);
        assert_eq!(image.headers(), &backing[1..0x201]);
        assert_eq!(image.checksum_file_offset(), OPTIONAL_START + 64);
        assert_eq!(image.security_entry_file_offset(), Some(SECURITY));
        assert_eq!(image.base_relocations(), Some(0x2000..0x2018));
        assert_eq!(image.certificate_file_range(), None);

        let sections: Vec<_> = image.sections().collect();
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].virtual_address, 0x1000);
        assert_eq!(sections[0].virtual_size, 0x180);
        assert_eq!(sections[0].file_offset, 0x200);
        assert_eq!(sections[0].file_data, &backing[0x201..0x401]);
        assert_eq!(sections[0].file_data[0], 0xc3);
        assert_eq!(sections[0].characteristics, 0x6000_0020);
        assert_eq!(sections[2].virtual_size, 0x5d0);
        assert!(sections[2].file_data.is_empty());
    }
}

#[test]
fn reads_only_declared_optional_directories_and_keeps_raw_only_sections() {
    let mut file = fixture(10);
    put_u16(&mut file, PE_START + 20, OPTIONAL_PREFIX_BYTES as u16);
    put_u32(&mut file, OPTIONAL_START + 108, 0);
    put_u32(&mut file, SECTIONS + 8, 0);
    file.copy_within(
        SECTIONS..SECTIONS + 3 * SECTION_HEADER_BYTES,
        OPTIONAL_START + OPTIONAL_PREFIX_BYTES,
    );
    let image = PeImage::parse(&file).unwrap();
    assert_eq!(image.security_entry_file_offset(), None);
    assert_eq!(image.base_relocations(), None);
    assert_eq!(image.sections().next().unwrap().virtual_size, 0);
    assert_eq!(image.sections().next().unwrap().file_data.len(), 0x200);
}

#[test]
fn handles_low_alignment_only_when_file_and_memory_placement_match() {
    let mut file = fixture(10);
    put_u32(&mut file, OPTIONAL_START + 32, 0x200);
    put_u32(&mut file, OPTIONAL_START + 16, 0x200);
    put_u32(&mut file, SECTIONS + 12, 0x200);
    put_u32(&mut file, SECTIONS + SECTION_HEADER_BYTES + 12, 0x400);
    put_u32(&mut file, RELOCATIONS, 0x400);
    assert_eq!(
        PeImage::parse(&file).unwrap().base_relocations(),
        Some(0x400..0x418)
    );

    put_u32(&mut file, SECTIONS + 12, 0x800);
    assert_eq!(
        PeImage::parse(&file).unwrap_err(),
        Error::InvalidSectionFileRange
    );
}

#[test]
fn certificate_is_a_file_span_and_unconsumed_directories_are_opaque() {
    let mut file = fixture(10);
    file.resize(0x4018, 0);
    put_u32(&mut file, SECURITY, 0x4000);
    put_u32(&mut file, SECURITY + 4, 0x18);
    let global_pointer = OPTIONAL_START + OPTIONAL_PREFIX_BYTES + 8 * DIRECTORY_BYTES;
    put_u32(&mut file, global_pointer, 0x1000);
    let image = PeImage::parse(&file).unwrap();
    assert_eq!(image.certificate_file_range(), Some(0x4000..0x4018));
    assert_eq!(image.image_size(), 0x4000);
}

#[test]
fn classifies_invalid_and_unsupported_signatures_and_fields() {
    let mut file = fixture(10);
    for (offset, value, expected) in [
        (0, 0, Error::InvalidSignature),
        (PE_START, 0, Error::InvalidSignature),
        (PE_START + 4, 0x14c, Error::UnsupportedMachine),
        (PE_START + 22, 0, Error::NotExecutable),
        (PE_START + 6, 0, Error::InvalidSectionCount),
        (PE_START + 6, 97, Error::InvalidSectionCount),
        (PE_START + 20, 111, Error::InvalidOptionalHeader),
        (OPTIONAL_START, 0x10b, Error::UnsupportedFormat),
        (OPTIONAL_START + 68, 3, Error::UnsupportedSubsystem),
    ] {
        let original = read_u16(&file, offset);
        put_u16(&mut file, offset, value);
        assert_eq!(PeImage::parse(&file).unwrap_err(), expected, "{offset:#x}");
        put_u16(&mut file, offset, original);
    }
}

#[test]
fn rejects_truncation_of_each_image_area() {
    let file = fixture(10);
    for len in [0, 63, PE_START, OPTIONAL_START - 1, SECTIONS - 1] {
        assert_eq!(PeImage::parse(&file[..len]).unwrap_err(), Error::Truncated);
    }
    for len in [SECTIONS + 3 * SECTION_HEADER_BYTES - 1, 0x1ff] {
        assert_eq!(
            PeImage::parse(&file[..len]).unwrap_err(),
            Error::InvalidHeaderSize
        );
    }
    assert_eq!(
        PeImage::parse(&file[..0x5ff]).unwrap_err(),
        Error::InvalidSectionFileRange
    );
}

#[test]
fn rejects_inconsistent_offsets_overlaps_and_alignment() {
    let mut file = fixture(10);
    for (offset, value, expected) in [
        (0x3c, 0x3f, Error::InvalidSignature),
        (0x3c, u32::MAX, Error::Truncated),
        (OPTIONAL_START + 108, u32::MAX, Error::InvalidOptionalHeader),
        (OPTIONAL_START + 32, 0, Error::InvalidAlignment),
        (OPTIONAL_START + 32, 0x400, Error::InvalidAlignment),
        (OPTIONAL_START + 36, 0x300, Error::InvalidAlignment),
        (OPTIONAL_START + 24, 0x1000, Error::InvalidImageSize),
        (OPTIONAL_START + 56, 0, Error::InvalidImageSize),
        (OPTIONAL_START + 56, 0x8000_1000, Error::InvalidImageSize),
        (OPTIONAL_START + 60, 0, Error::InvalidHeaderSize),
        (OPTIONAL_START + 16, 0x4000, Error::InvalidEntryPoint),
        (SECTIONS + 8, u32::MAX, Error::InvalidSectionImageRange),
        (SECTIONS + 12, 0x200, Error::InvalidSectionImageRange),
        (SECTIONS + 12, 0x4000, Error::InvalidSectionImageRange),
        (SECTIONS + 20, 0x201, Error::InvalidSectionFileRange),
        (SECTIONS + 20, 0xffff_fe00, Error::InvalidSectionFileRange),
        (SECTIONS + 20, 0x400, Error::OverlappingSectionFileRanges),
        (SECTIONS + 12, 0x2000, Error::OverlappingSectionImageRanges),
        (RELOCATIONS, 0x3000, Error::InvalidBaseRelocationDirectory),
        (RELOCATIONS, 0x3ff0, Error::InvalidBaseRelocationDirectory),
        (RELOCATIONS + 4, 0, Error::InvalidBaseRelocationDirectory),
        (
            RELOCATIONS + 4,
            u32::MAX,
            Error::InvalidBaseRelocationDirectory,
        ),
    ] {
        let original = read_u32(&file, offset);
        put_u32(&mut file, offset, value);
        assert_eq!(PeImage::parse(&file).unwrap_err(), expected, "{offset:#x}");
        put_u32(&mut file, offset, original);
    }
}

#[test]
fn rejects_misaligned_overlapping_and_truncated_certificate_spans() {
    let mut file = fixture(10);
    file.resize(0x618, 0);
    for (offset, size) in [
        (0x600, 0),
        (0, 8),
        (0x601, 8),
        (0x600, 7),
        (0x600, 0x20),
        (0x100, 8),
        (0x400, 8),
        (0xffff_fff8, 0x10),
    ] {
        put_u32(&mut file, SECURITY, offset);
        put_u32(&mut file, SECURITY + 4, size);
        assert_eq!(
            PeImage::parse(&file).unwrap_err(),
            Error::InvalidCertificateDirectory
        );
    }
}
