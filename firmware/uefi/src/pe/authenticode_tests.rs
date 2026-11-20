use super::*;
use crate::pe::test_image::{
    OPTIONAL_START, SECTION_HEADER_BYTES, SECTIONS, SECURITY, fixture, put_u16, put_u32,
};
use sha2::{Digest, Sha256};

const CHECKSUM: usize = OPTIONAL_START + 64;
const HEADERS_END: usize = 0x200;

fn digest(file: &[u8]) -> Result<[u8; 32], DigestError> {
    authenticode_digest(&PeImage::parse(file).unwrap(), DigestAlgorithm::Sha256)
}

fn digest_segments(segments: &[&[u8]]) -> [u8; 32] {
    let mut hash = Sha256::new();
    for segment in segments {
        hash.update(segment);
    }
    hash.finalize().into()
}

#[test]
fn excludes_checksum_and_certificate_but_keeps_raw_padding_and_overlay() {
    let mut unsigned = fixture(10);
    unsigned.extend_from_slice(b"overlay!");
    let expected = digest_segments(&[
        &unsigned[..CHECKSUM],
        &unsigned[CHECKSUM + 4..SECURITY],
        &unsigned[SECURITY + 8..],
    ]);
    assert_eq!(digest(&unsigned), Ok(expected));

    let mut signed = unsigned;
    let certificate_offset = signed.len();
    put_u32(&mut signed, SECURITY, certificate_offset as u32);
    put_u32(&mut signed, SECURITY + 4, 16);
    signed.extend_from_slice(&[0x77; 16]);
    put_u32(&mut signed, CHECKSUM, 0xa5a5_4242);
    assert_eq!(digest(&signed), Ok(expected));
    signed[certificate_offset] ^= 0xff;
    assert_eq!(digest(&signed), Ok(expected));

    for offset in [0x50, 0x390, certificate_offset - 1] {
        signed[offset] ^= 0xff;
        assert_ne!(digest(&signed), Ok(expected), "file offset {offset:#x}");
        signed[offset] ^= 0xff;
    }
}

#[test]
fn hashes_sections_in_file_offset_order() {
    let mut reordered = fixture(10);
    reordered[0x400..0x600].fill(0x65);
    let second = SECTIONS + SECTION_HEADER_BYTES;
    let third = second + SECTION_HEADER_BYTES;
    let first_header = reordered[SECTIONS..second].to_vec();
    reordered.copy_within(second..third, SECTIONS);
    reordered[second..third].copy_from_slice(&first_header);

    let headers = [
        &reordered[..CHECKSUM],
        &reordered[CHECKSUM + 4..SECURITY],
        &reordered[SECURITY + 8..HEADERS_END],
    ];
    let text = &reordered[0x200..0x400];
    let relocations = &reordered[0x400..0x600];
    assert_eq!(
        digest(&reordered),
        Ok(digest_segments(&[
            headers[0],
            headers[1],
            headers[2],
            text,
            relocations,
        ]))
    );
    assert_ne!(
        digest(&reordered),
        Ok(digest_segments(&[
            headers[0],
            headers[1],
            headers[2],
            relocations,
            text,
        ]))
    );
}

#[test]
fn starts_tail_at_bytes_hashed_even_when_raw_ranges_have_a_gap() {
    let mut file = fixture(10);
    file.resize(0x800, 0);
    put_u32(&mut file, SECTIONS + SECTION_HEADER_BYTES + 20, 0x600);
    file[0x400..0x600].fill(0x9a);
    file[0x600..0x800].fill(0x7d);
    let expected = digest_segments(&[
        &file[..CHECKSUM],
        &file[CHECKSUM + 4..SECURITY],
        &file[SECURITY + 8..HEADERS_END],
        &file[0x200..0x400],
        &file[0x600..0x800],
        &file[0x600..],
    ]);
    assert_eq!(digest(&file), Ok(expected));
    file[0x4ef] ^= 0xff;
    assert_eq!(digest(&file), Ok(expected));
    file[0x6ef] ^= 0xff;
    assert_ne!(digest(&file), Ok(expected));
}

#[test]
fn keeps_optional_header_bytes_when_security_entry_is_absent() {
    for (count, shrink) in [(0, true), (4, true), (4, false)] {
        let mut file = fixture(10);
        put_u32(&mut file, OPTIONAL_START + 108, count);
        if shrink {
            let optional_bytes = 112 + count as usize * 8;
            put_u16(&mut file, OPTIONAL_START - 4, optional_bytes as u16);
            file.copy_within(
                SECTIONS..SECTIONS + 3 * SECTION_HEADER_BYTES,
                OPTIONAL_START + optional_bytes,
            );
        }
        let original = digest_segments(&[&file[..CHECKSUM], &file[CHECKSUM + 4..]]);
        assert_eq!(digest(&file), Ok(original));
        // With a short optional header this can be part of a section name.
        file[SECURITY] ^= 0x10;
        assert_ne!(digest(&file), Ok(original));
    }
}

#[test]
fn rejects_certificate_before_end_of_file() {
    let mut file = fixture(10);
    file.resize(0x620, 0);
    put_u32(&mut file, SECURITY, 0x600);
    put_u32(&mut file, SECURITY + 4, 0x10);
    assert_eq!(digest(&file), Err(DigestError::CertificateNotAtEnd));
}
