//! PE attribute certificate entries from the Microsoft PE Format specification.

use core::mem;

use super::PeImage;

const HEADER_BYTES: usize = 8;
const ALIGNMENT: usize = 8;
const REVISION_2: u16 = 0x0200;
const PKCS_SIGNED_DATA: u16 = 0x0002;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CertificateError {
    Length,
    Padding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WinCertificate<'a> {
    pub revision: u16,
    pub certificate_type: u16,
    /// Bytes inside the wire entry. This may include alignment padding counted by dwLength.
    pub data: &'a [u8],
}

impl WinCertificate<'_> {
    pub fn is_authenticode(&self) -> bool {
        self.revision == REVISION_2 && self.certificate_type == PKCS_SIGNED_DATA
    }
}

pub struct WinCertificates<'a> {
    remaining: &'a [u8],
}

impl<'a> PeImage<'a> {
    pub fn win_certificates(&self) -> WinCertificates<'a> {
        let remaining = self
            .certificate_file_range()
            .map_or(&[][..], |range| &self.file()[range]);
        WinCertificates { remaining }
    }
}

impl<'a> Iterator for WinCertificates<'a> {
    type Item = Result<WinCertificate<'a>, CertificateError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.is_empty() {
            return None;
        }
        let bytes = mem::take(&mut self.remaining);
        Some(decode_entry(bytes).map(|(entry, remaining)| {
            self.remaining = remaining;
            entry
        }))
    }
}

fn decode_entry(bytes: &[u8]) -> Result<(WinCertificate<'_>, &[u8]), CertificateError> {
    let header = bytes
        .first_chunk::<HEADER_BYTES>()
        .ok_or(CertificateError::Length)?;
    let length = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
    if length < HEADER_BYTES {
        return Err(CertificateError::Length);
    }
    let padded_length = length
        .checked_next_multiple_of(ALIGNMENT)
        .ok_or(CertificateError::Length)?;
    let (entry, remaining) = bytes
        .split_at_checked(padded_length)
        .ok_or(CertificateError::Length)?;
    if entry[length..].iter().any(|&byte| byte != 0) {
        return Err(CertificateError::Padding);
    }
    let revision = u16::from_le_bytes([header[4], header[5]]);
    let certificate_type = u16::from_le_bytes([header[6], header[7]]);
    Ok((
        WinCertificate {
            revision,
            certificate_type,
            data: &entry[HEADER_BYTES..length],
        },
        remaining,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_entries_with_counted_or_external_alignment_padding() {
        let table = [
            11, 0, 0, 0, 0, 2, 2, 0, 0x30, 0x01, 0xff, 0, 0, 0, 0, 0, 16, 0, 0, 0, 0, 1, 1, 0,
            0x30, 0x01, 0xee, 0, 0, 0, 0, 0,
        ];
        let mut certificates = WinCertificates { remaining: &table };
        let first = certificates.next().unwrap().unwrap();
        assert!(first.is_authenticode());
        assert_eq!(first.data, &[0x30, 0x01, 0xff]);
        let second = certificates.next().unwrap().unwrap();
        assert!(!second.is_authenticode());
        assert_eq!((second.revision, second.certificate_type), (0x0100, 1));
        assert_eq!(second.data, &[0x30, 0x01, 0xee, 0, 0, 0, 0, 0]);
        assert_eq!(certificates.next(), None);
    }

    #[test]
    fn refuses_a_short_header_bad_length_or_nonzero_external_padding() {
        for (entry, error) in [
            (&[0u8; 7][..], CertificateError::Length),
            (&[7, 0, 0, 0, 0, 2, 2, 0][..], CertificateError::Length),
            (
                &[0xff, 0xff, 0xff, 0xff, 0, 2, 2, 0][..],
                CertificateError::Length,
            ),
            (&[9, 0, 0, 0, 0, 2, 2, 0, 1][..], CertificateError::Length),
            (
                &[9, 0, 0, 0, 0, 2, 2, 0, 1, 2, 0, 0, 0, 0, 0, 0][..],
                CertificateError::Padding,
            ),
        ] {
            let mut certificates = WinCertificates { remaining: entry };
            assert_eq!(certificates.next(), Some(Err(error)));
            assert_eq!(certificates.next(), None);
        }
    }
}
