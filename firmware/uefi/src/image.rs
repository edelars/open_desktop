//! Firmware image header v1 (`ODFWIMG1`, FIRMWARE_BOOT.md, section 3.2) and CRC-32 (section 6.6).
//!
//! Pure logic: checks a complete image in memory. The machine-side loader (S7-03, `od-machine`)
//! has its own implementation of the same contract; `tools/build/build_firmware.py` writes the
//! header and its tests check the same fields.

/// `ODFWIMG1` as a little-endian `u64`.
pub const IMAGE_MAGIC: u64 = 0x3147_4D49_5746_444F;
pub const IMAGE_MAGIC_BYTES: [u8; 8] = *b"ODFWIMG1";
pub const FORMAT_VERSION: u32 = 1;
pub const HEADER_SIZE: usize = 0x40;
pub const IMAGE_SIZE: usize = 0x0040_0000;
pub const LINK_BASE: u64 = 0x0020_0000;
pub const HANDOFF_MAJOR: u16 = 1;
/// Offset of the `image_crc32` field.
pub const CRC_OFFSET: usize = 0x30;

/// Decoded header fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageHeader {
    pub format_version: u32,
    pub header_size: u32,
    pub image_size: u64,
    pub link_base: u64,
    pub entry_offset: u64,
    pub handoff_major: u16,
    pub handoff_minor_max: u16,
    pub flags: u32,
    pub image_crc32: u32,
}

/// Why an image was rejected (one reason per check, in the order of `validate_image`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageError {
    /// The image is not exactly `IMAGE_SIZE` bytes.
    WrongSize {
        actual: usize,
    },
    BadMagic,
    BadFormatVersion {
        actual: u32,
    },
    BadHeaderSize {
        actual: u32,
    },
    BadImageSize {
        actual: u64,
    },
    BadLinkBase {
        actual: u64,
    },
    /// `header_size <= entry_offset < image_size` does not hold.
    EntryOutOfRange {
        actual: u64,
    },
    BadHandoffMajor {
        actual: u16,
    },
    NonZeroFlags {
        actual: u32,
    },
    NonZeroReserved,
    BadCrc {
        stored: u32,
        computed: u32,
    },
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

impl ImageHeader {
    /// Serialises the header to its 0x40 bytes (reserved bytes are zero).
    pub fn to_bytes(&self) -> [u8; HEADER_SIZE] {
        let mut bytes = [0u8; HEADER_SIZE];
        bytes[0x00..0x08].copy_from_slice(&IMAGE_MAGIC_BYTES);
        bytes[0x08..0x0C].copy_from_slice(&self.format_version.to_le_bytes());
        bytes[0x0C..0x10].copy_from_slice(&self.header_size.to_le_bytes());
        bytes[0x10..0x18].copy_from_slice(&self.image_size.to_le_bytes());
        bytes[0x18..0x20].copy_from_slice(&self.link_base.to_le_bytes());
        bytes[0x20..0x28].copy_from_slice(&self.entry_offset.to_le_bytes());
        bytes[0x28..0x2A].copy_from_slice(&self.handoff_major.to_le_bytes());
        bytes[0x2A..0x2C].copy_from_slice(&self.handoff_minor_max.to_le_bytes());
        bytes[0x2C..0x30].copy_from_slice(&self.flags.to_le_bytes());
        bytes[CRC_OFFSET..CRC_OFFSET + 4].copy_from_slice(&self.image_crc32.to_le_bytes());
        bytes
    }
}

/// CRC-32 IEEE 802.3, reflected: polynomial `0xEDB8_8320`, init and final XOR `0xFFFF_FFFF`.
pub fn crc32(bytes: &[u8]) -> u32 {
    crc32_update(0xFFFF_FFFF, bytes) ^ 0xFFFF_FFFF
}

fn crc32_update(mut state: u32, bytes: &[u8]) -> u32 {
    for &byte in bytes {
        state ^= u32::from(byte);
        for _ in 0..8 {
            state = if state & 1 != 0 {
                (state >> 1) ^ 0xEDB8_8320
            } else {
                state >> 1
            };
        }
    }
    state
}

/// CRC-32 of the whole image with the `image_crc32` field treated as zero.
pub fn image_crc32(image: &[u8]) -> u32 {
    let state = crc32_update(0xFFFF_FFFF, &image[..CRC_OFFSET]);
    let state = crc32_update(state, &[0; 4]);
    crc32_update(state, &image[CRC_OFFSET + 4..]) ^ 0xFFFF_FFFF
}

/// Checks a complete in-memory image against the v1 contract and returns its header.
pub fn validate_image(image: &[u8]) -> Result<ImageHeader, ImageError> {
    if image.len() != IMAGE_SIZE {
        return Err(ImageError::WrongSize {
            actual: image.len(),
        });
    }
    if image[..8] != IMAGE_MAGIC_BYTES {
        return Err(ImageError::BadMagic);
    }
    let header = ImageHeader {
        format_version: u32_at(image, 0x08),
        header_size: u32_at(image, 0x0C),
        image_size: u64_at(image, 0x10),
        link_base: u64_at(image, 0x18),
        entry_offset: u64_at(image, 0x20),
        handoff_major: u16_at(image, 0x28),
        handoff_minor_max: u16_at(image, 0x2A),
        flags: u32_at(image, 0x2C),
        image_crc32: u32_at(image, CRC_OFFSET),
    };
    if header.format_version != FORMAT_VERSION {
        return Err(ImageError::BadFormatVersion {
            actual: header.format_version,
        });
    }
    if header.header_size as usize != HEADER_SIZE {
        return Err(ImageError::BadHeaderSize {
            actual: header.header_size,
        });
    }
    if header.image_size != IMAGE_SIZE as u64 {
        return Err(ImageError::BadImageSize {
            actual: header.image_size,
        });
    }
    if header.link_base != LINK_BASE {
        return Err(ImageError::BadLinkBase {
            actual: header.link_base,
        });
    }
    if header.entry_offset < HEADER_SIZE as u64 || header.entry_offset >= header.image_size {
        return Err(ImageError::EntryOutOfRange {
            actual: header.entry_offset,
        });
    }
    if header.handoff_major != HANDOFF_MAJOR {
        return Err(ImageError::BadHandoffMajor {
            actual: header.handoff_major,
        });
    }
    if header.flags != 0 {
        return Err(ImageError::NonZeroFlags {
            actual: header.flags,
        });
    }
    if image[0x34..HEADER_SIZE].iter().any(|&byte| byte != 0) {
        return Err(ImageError::NonZeroReserved);
    }
    let computed = image_crc32(image);
    if computed != header.image_crc32 {
        return Err(ImageError::BadCrc {
            stored: header.image_crc32,
            computed,
        });
    }
    Ok(header)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_image() -> Vec<u8> {
        let mut image = vec![0u8; IMAGE_SIZE];
        let header = ImageHeader {
            format_version: FORMAT_VERSION,
            header_size: HEADER_SIZE as u32,
            image_size: IMAGE_SIZE as u64,
            link_base: LINK_BASE,
            entry_offset: 0x40,
            handoff_major: HANDOFF_MAJOR,
            handoff_minor_max: 3,
            flags: 0,
            image_crc32: 0,
        };
        image[..HEADER_SIZE].copy_from_slice(&header.to_bytes());
        image[0x40..0x48].copy_from_slice(&[0x90, 0x90, 0x90, 0x90, 0xF4, 0xEB, 0xFD, 0x00]);
        let crc = image_crc32(&image);
        image[CRC_OFFSET..CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
        image
    }

    #[test]
    fn crc32_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn magic_constant_matches_ascii() {
        assert_eq!(u64::from_le_bytes(IMAGE_MAGIC_BYTES), IMAGE_MAGIC);
    }

    #[test]
    fn accepts_valid_image() {
        let image = sample_image();
        let header = validate_image(&image).unwrap();
        assert_eq!(header.entry_offset, 0x40);
        assert_eq!(header.link_base, 0x0020_0000);
        assert_eq!(&header.to_bytes()[..], &image[..HEADER_SIZE]);
    }

    #[test]
    fn rejects_wrong_size() {
        let image = sample_image();
        assert_eq!(
            validate_image(&image[..IMAGE_SIZE - 1]),
            Err(ImageError::WrongSize {
                actual: IMAGE_SIZE - 1
            })
        );
        let mut longer = image;
        longer.push(0);
        assert_eq!(
            validate_image(&longer),
            Err(ImageError::WrongSize {
                actual: IMAGE_SIZE + 1
            })
        );
    }

    #[test]
    fn rejects_bad_magic_and_crc() {
        let mut image = sample_image();
        image[0] = b'X';
        assert_eq!(validate_image(&image), Err(ImageError::BadMagic));

        let mut image = sample_image();
        image[0x1000] ^= 1;
        assert!(matches!(
            validate_image(&image),
            Err(ImageError::BadCrc { .. })
        ));
    }

    #[test]
    fn rejects_entry_outside_image_and_in_header() {
        for entry in [0x3Fu64, IMAGE_SIZE as u64] {
            let mut image = sample_image();
            image[0x20..0x28].copy_from_slice(&entry.to_le_bytes());
            let crc = image_crc32(&image);
            image[CRC_OFFSET..CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
            assert_eq!(
                validate_image(&image),
                Err(ImageError::EntryOutOfRange { actual: entry })
            );
        }
    }

    #[test]
    fn rejects_nonzero_reserved_and_flags() {
        let mut image = sample_image();
        image[0x3F] = 1;
        assert_eq!(validate_image(&image), Err(ImageError::NonZeroReserved));

        let mut image = sample_image();
        image[0x2C] = 1;
        assert_eq!(
            validate_image(&image),
            Err(ImageError::NonZeroFlags { actual: 1 })
        );
    }

    /// Reference header shared with `tools/build/test_build_firmware.py` (`REFERENCE_HEADER_HEX`):
    /// both validators are pinned to the same bytes. entry_offset = 0x40, image_crc32 = 0x12345678.
    const REFERENCE_HEADER: [u8; HEADER_SIZE] = [
        0x4F, 0x44, 0x46, 0x57, 0x49, 0x4D, 0x47, 0x31, 0x01, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x03, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x78, 0x56, 0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00,
    ];

    /// CRC-32 of `sample_image()`; `tools/build/test_build_firmware.py` builds the same image in
    /// Python and expects the same value.
    const SAMPLE_IMAGE_CRC32: u32 = 0x782E_EE6C;

    #[test]
    fn header_matches_reference_bytes_shared_with_python() {
        let header = ImageHeader {
            format_version: FORMAT_VERSION,
            header_size: HEADER_SIZE as u32,
            image_size: IMAGE_SIZE as u64,
            link_base: LINK_BASE,
            entry_offset: 0x40,
            handoff_major: HANDOFF_MAJOR,
            handoff_minor_max: 3,
            flags: 0,
            image_crc32: 0x1234_5678,
        };
        assert_eq!(header.to_bytes(), REFERENCE_HEADER);
        assert_eq!(
            &REFERENCE_HEADER[CRC_OFFSET..CRC_OFFSET + 4],
            &[0x78, 0x56, 0x34, 0x12]
        );
        assert_eq!(&REFERENCE_HEADER[0x34..0x40], &[0u8; 12]);
    }

    #[test]
    fn sample_image_crc_matches_value_shared_with_python() {
        let image = sample_image();
        assert_eq!(image_crc32(&image), SAMPLE_IMAGE_CRC32);
        assert_eq!(
            u32::from_le_bytes(image[CRC_OFFSET..CRC_OFFSET + 4].try_into().unwrap()),
            SAMPLE_IMAGE_CRC32
        );
    }
}
