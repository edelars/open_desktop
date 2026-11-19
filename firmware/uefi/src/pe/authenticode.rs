//! Authenticode image hashing from Microsoft's March 21, 2008 specification.

use super::parser::{MAX_SECTIONS, PeImage};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DigestAlgorithm {
    Sha256,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DigestError {
    CertificateNotAtEnd,
    InvalidHashRange,
}

/// Hashes the PE headers, file-backed sections in file order, and the Authenticode tail.
pub fn authenticode_digest(
    image: &PeImage<'_>,
    algorithm: DigestAlgorithm,
) -> Result<[u8; 32], DigestError> {
    let file = image.file();
    let hash_end = match image.certificate_file_range() {
        Some(certificate) if certificate.end == file.len() => certificate.start,
        Some(_) => return Err(DigestError::CertificateNotAtEnd),
        None => file.len(),
    };
    let mut hash = match algorithm {
        DigestAlgorithm::Sha256 => Sha256::new(),
    };

    let headers = image.headers();
    let checksum = image.checksum_file_offset();
    hash.update(&headers[..checksum]);
    let after_checksum = checksum + 4;
    if let Some(security) = image.security_entry_file_offset() {
        hash.update(&headers[after_checksum..security]);
        hash.update(&headers[security + 8..]);
    } else {
        hash.update(&headers[after_checksum..]);
    }

    let mut raw_ranges = [(0, 0); MAX_SECTIONS];
    let mut count = 0;
    for section in image
        .sections()
        .filter(|section| !section.file_data.is_empty())
    {
        raw_ranges[count] = (section.file_offset, section.file_data.len());
        count += 1;
    }
    let raw_ranges = &mut raw_ranges[..count];
    raw_ranges.sort_unstable_by_key(|range| range.0);

    // Microsoft's SUM_OF_BYTES_HASHED counts bytes, so holes in the file
    // do not advance the tail start to the end of the final section.
    let mut bytes_hashed = headers.len();
    for &(offset, size) in raw_ranges.iter() {
        hash.update(&file[offset..offset + size]);
        bytes_hashed = bytes_hashed
            .checked_add(size)
            .ok_or(DigestError::InvalidHashRange)?;
    }
    let tail = file
        .get(bytes_hashed..hash_end)
        .ok_or(DigestError::InvalidHashRange)?;
    hash.update(tail);

    Ok(hash.finalize().into())
}

#[cfg(test)]
#[path = "authenticode_tests.rs"]
mod tests;
