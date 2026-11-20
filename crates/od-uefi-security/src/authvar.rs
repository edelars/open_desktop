//! EFI_VARIABLE_AUTHENTICATION_2 decoding and detached input hashing, UEFI 2.11 §8.2.6.

use sha2::{Digest, Sha256};

use crate::{Guid, Name};

const TIMESTAMP_BYTES: usize = 16;
const CERT_HEADER_BYTES: usize = 24;
const CERT_REVISION_2_0: u16 = 0x0200;
const CERT_TYPE_EFI_GUID: u16 = 0x0ef1;
const EFI_CERT_TYPE_PKCS7_GUID: [u8; 16] = [
    0x9d, 0xd2, 0xaf, 0x4a, 0xdf, 0x68, 0xee, 0x49, 0x8a, 0xa9, 0x34, 0x7d, 0x37, 0x56, 0x65, 0xa7,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Timestamp {
    year: u16,
    month_through_second: [u8; 5],
}

impl Timestamp {
    pub fn from_bytes(bytes: &[u8; TIMESTAMP_BYTES]) -> Option<Self> {
        let [
            year_low,
            year_high,
            month,
            day,
            hour,
            minute,
            second,
            reserved @ ..,
        ] = *bytes;
        if reserved != [0; 9] {
            return None;
        }
        let year = u16::from_le_bytes([year_low, year_high]);
        let limit = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => 28 + u8::from(year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)),
            _ => return None,
        };
        if !(1900..=9999).contains(&year)
            || !(1..=limit).contains(&day)
            || hour > 23
            || minute > 59
            || second > 59
        {
            return None;
        }
        Some(Self {
            year,
            month_through_second: [month, day, hour, minute, second],
        })
    }

    pub fn to_bytes(self) -> [u8; TIMESTAMP_BYTES] {
        let mut bytes = [0; TIMESTAMP_BYTES];
        bytes[..2].copy_from_slice(&self.year.to_le_bytes());
        bytes[2..7].copy_from_slice(&self.month_through_second);
        bytes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DescriptorError {
    InvalidTimestamp,
    InvalidCertificateHeader,
}

#[derive(Clone, Copy, Debug)]
pub struct Descriptor<'a> {
    timestamp_bytes: &'a [u8; TIMESTAMP_BYTES],
    pub timestamp: Option<Timestamp>,
    pub signed_data: &'a [u8],
    pub value: &'a [u8],
}

impl<'a> Descriptor<'a> {
    pub fn decode(data: &'a [u8], append: bool) -> Result<Self, DescriptorError> {
        let timestamp_bytes = data
            .first_chunk()
            .ok_or(DescriptorError::InvalidTimestamp)?;
        let timestamp = match Timestamp::from_bytes(timestamp_bytes) {
            Some(value) => Some(value),
            None if append && *timestamp_bytes == [0; TIMESTAMP_BYTES] => None,
            None => return Err(DescriptorError::InvalidTimestamp),
        };

        let rest = &data[TIMESTAMP_BYTES..];
        let header: &[u8; CERT_HEADER_BYTES] = rest
            .first_chunk()
            .ok_or(DescriptorError::InvalidCertificateHeader)?;
        let cert_len = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
        if cert_len < CERT_HEADER_BYTES
            || header[4..6] != CERT_REVISION_2_0.to_le_bytes()
            || header[6..8] != CERT_TYPE_EFI_GUID.to_le_bytes()
            || header[8..] != EFI_CERT_TYPE_PKCS7_GUID
        {
            return Err(DescriptorError::InvalidCertificateHeader);
        }
        let (cert, value) = rest
            .split_at_checked(cert_len)
            .ok_or(DescriptorError::InvalidCertificateHeader)?;
        Ok(Self {
            timestamp_bytes,
            timestamp,
            signed_data: &cert[CERT_HEADER_BYTES..],
            value,
        })
    }

    pub fn sha256(&self, name: Name<'_>, vendor: Guid, attributes: u32) -> [u8; 32] {
        let mut sha256 = Sha256::new();
        sha256.update(name.as_bytes());
        sha256.update(vendor.data1.to_le_bytes());
        sha256.update(vendor.data2.to_le_bytes());
        sha256.update(vendor.data3.to_le_bytes());
        sha256.update(vendor.data4);
        sha256.update(attributes.to_le_bytes());
        sha256.update(self.timestamp_bytes);
        sha256.update(self.value);
        sha256.finalize().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_TIME: [u8; 16] = [0xea, 7, 11, 19, 21, 49, 28, 0, 0, 0, 0, 0, 0, 0, 0, 0];

    fn descriptor(time: [u8; 16], signature: &[u8], value: &[u8]) -> Vec<u8> {
        let mut bytes = time.to_vec();
        bytes.extend(((CERT_HEADER_BYTES + signature.len()) as u32).to_le_bytes());
        bytes.extend(CERT_REVISION_2_0.to_le_bytes());
        bytes.extend(CERT_TYPE_EFI_GUID.to_le_bytes());
        bytes.extend(EFI_CERT_TYPE_PKCS7_GUID);
        bytes.extend(signature);
        bytes.extend(value);
        bytes
    }

    #[test]
    fn decodes_the_auth_envelope_and_hashes_the_detached_input() {
        let data = descriptor(VALID_TIME, b"pkcs7", b"der payload");
        let decoded = Descriptor::decode(&data, true).unwrap();
        assert_eq!(decoded.timestamp.unwrap().to_bytes(), VALID_TIME);
        assert_eq!(decoded.signed_data, b"pkcs7");
        assert_eq!(decoded.value, b"der payload");
        let name = Name::new(b"d\0b\0x\0").unwrap();
        let vendor = Guid {
            data1: 0xd719_b2cb,
            data2: 0x3d3a,
            data3: 0x4596,
            data4: [0xa3, 0xbc, 0xda, 0xd0, 0x0e, 0x67, 0x65, 0x6f],
        };
        // Python hashlib over UTF-16LE name, uuid.bytes_le, little-endian attrs, timestamp, value.
        assert_eq!(
            decoded.sha256(name, vendor, 0x67),
            [
                0x03, 0xae, 0x29, 0x4b, 0x85, 0x82, 0x38, 0x2a, 0x9b, 0x63, 0x5d, 0xfc, 0xf9, 0x7c,
                0x6e, 0x56, 0xe8, 0xa1, 0xa1, 0x2d, 0x26, 0x34, 0x62, 0x42, 0x3c, 0x78, 0x18, 0x9f,
                0x23, 0x2f, 0x1a, 0xd7,
            ]
        );
        assert_ne!(
            decoded.sha256(name, vendor, 0x27),
            decoded.sha256(name, vendor, 0x67)
        );
    }

    #[test]
    fn validates_timestamp_fields_and_orders_years_numerically() {
        let mut earlier = VALID_TIME;
        earlier[..2].copy_from_slice(&2047u16.to_le_bytes());
        let mut later = earlier;
        later[..2].copy_from_slice(&2048u16.to_le_bytes());
        assert!(Timestamp::from_bytes(&earlier) < Timestamp::from_bytes(&later));
        later[2..4].copy_from_slice(&[2, 29]);
        assert!(Timestamp::from_bytes(&later).is_some());
        earlier[2..4].copy_from_slice(&[2, 29]);
        assert!(Timestamp::from_bytes(&earlier).is_none());
        for (offset, invalid) in [
            (2, 13),
            (3, 0),
            (4, 24),
            (5, 60),
            (6, 60),
            (7, 1),
            (8, 1),
            (12, 1),
            (14, 1),
            (15, 1),
        ] {
            let mut bytes = VALID_TIME;
            bytes[offset] = invalid;
            assert!(Timestamp::from_bytes(&bytes).is_none());
        }
        let data = descriptor([0; 16], b"", b"");
        assert_eq!(Descriptor::decode(&data, true).unwrap().timestamp, None);
        assert_eq!(
            Descriptor::decode(&data, false).unwrap_err(),
            DescriptorError::InvalidTimestamp
        );
    }

    #[test]
    fn rejects_truncation_invalid_length_and_wrong_certificate_type() {
        let data = descriptor(VALID_TIME, b"p7", b"");
        for length in 0..data.len() {
            assert!(Descriptor::decode(&data[..length], false).is_err());
        }
        for length in [0u32, 23, u32::MAX] {
            let mut bad = data.clone();
            bad[16..20].copy_from_slice(&length.to_le_bytes());
            assert_eq!(
                Descriptor::decode(&bad, false).unwrap_err(),
                DescriptorError::InvalidCertificateHeader
            );
        }
        for offset in [20, 22, 24] {
            let mut bad = data.clone();
            bad[offset] ^= 1;
            assert_eq!(
                Descriptor::decode(&bad, false).unwrap_err(),
                DescriptorError::InvalidCertificateHeader
            );
        }
    }
}
