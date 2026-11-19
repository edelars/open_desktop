//! Borrowed EFI_SIGNATURE_LIST decoder, UEFI 2.11 §32.4.1.

const GUID_BYTES: usize = 16;
const LIST_HEADER_BYTES: usize = GUID_BYTES + 3 * 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignatureType {
    X509,
    Sha256,
    X509Sha256,
}

impl SignatureType {
    pub const fn guid_bytes(self) -> [u8; GUID_BYTES] {
        match self {
            Self::X509 => [
                0xa1, 0x59, 0xc0, 0xa5, 0xe4, 0x94, 0xa7, 0x4a, 0x87, 0xb5, 0xab, 0x15, 0x5c, 0x2b,
                0xf0, 0x72,
            ],
            Self::Sha256 => [
                0x26, 0x16, 0xc4, 0xc1, 0x4c, 0x50, 0x92, 0x40, 0xac, 0xa9, 0x41, 0xf9, 0x36, 0x93,
                0x43, 0x28,
            ],
            Self::X509Sha256 => [
                0x92, 0xa4, 0xd2, 0x3b, 0xc0, 0x96, 0x79, 0x40, 0xb4, 0x20, 0xfc, 0xf9, 0x8e, 0xf1,
                0x03, 0xed,
            ],
        }
    }

    fn from_guid(bytes: &[u8]) -> Option<Self> {
        [Self::X509, Self::Sha256, Self::X509Sha256]
            .into_iter()
            .find(|kind| bytes == kind.guid_bytes())
    }

    fn valid_entry_size(self, size: usize) -> bool {
        match self {
            Self::X509 => size > GUID_BYTES,
            Self::Sha256 => size == GUID_BYTES + 32,
            Self::X509Sha256 => size == GUID_BYTES + 32 + 16,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListError {
    InvalidLayout,
    UnsupportedType,
}

#[derive(Clone, Copy, Debug)]
pub struct Signature<'a> {
    bytes: &'a [u8],
}

impl Signature<'_> {
    /// Complete EFI_SIGNATURE_DATA, including the owner GUID for event logging.
    pub fn bytes(&self) -> &[u8] {
        self.bytes
    }

    pub fn data(&self) -> &[u8] {
        &self.bytes[GUID_BYTES..]
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SignatureList<'a> {
    pub kind: SignatureType,
    data: &'a [u8],
    entry_size: usize,
}

impl<'a> SignatureList<'a> {
    pub fn signatures(self) -> impl Iterator<Item = Signature<'a>> {
        self.data
            .chunks_exact(self.entry_size)
            .map(|bytes| Signature { bytes })
    }

    fn decode(input: &'a [u8]) -> Result<(Self, &'a [u8]), ListError> {
        let header: &[u8; LIST_HEADER_BYTES] =
            input.first_chunk().ok_or(ListError::InvalidLayout)?;
        let kind =
            SignatureType::from_guid(&header[..GUID_BYTES]).ok_or(ListError::UnsupportedType)?;
        let list_size = u32::from_le_bytes(header[16..20].try_into().unwrap()) as usize;
        let header_size = u32::from_le_bytes(header[20..24].try_into().unwrap());
        let entry_size = u32::from_le_bytes(header[24..28].try_into().unwrap()) as usize;
        if header_size != 0 || !kind.valid_entry_size(entry_size) {
            return Err(ListError::InvalidLayout);
        }
        let (list, remaining) = input
            .split_at_checked(list_size)
            .ok_or(ListError::InvalidLayout)?;
        let data = list
            .get(LIST_HEADER_BYTES..)
            .ok_or(ListError::InvalidLayout)?;
        if !data.len().is_multiple_of(entry_size) {
            return Err(ListError::InvalidLayout);
        }
        Ok((
            Self {
                kind,
                data,
                entry_size,
            },
            remaining,
        ))
    }
}

pub fn signature_lists(
    mut input: &[u8],
) -> impl Iterator<Item = Result<SignatureList<'_>, ListError>> {
    core::iter::from_fn(move || {
        if input.is_empty() {
            return None;
        }
        match SignatureList::decode(input) {
            Ok((list, remaining)) => {
                input = remaining;
                Some(Ok(list))
            }
            Err(error) => {
                input = &[];
                Some(Err(error))
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(kind: SignatureType, entry_data: &[u8]) -> Vec<u8> {
        let entry_size = GUID_BYTES + entry_data.len();
        let mut bytes = kind.guid_bytes().to_vec();
        bytes.extend(((LIST_HEADER_BYTES + entry_size) as u32).to_le_bytes());
        bytes.extend([0; 4]);
        bytes.extend((entry_size as u32).to_le_bytes());
        bytes.extend([7; GUID_BYTES]);
        bytes.extend(entry_data);
        bytes
    }

    #[test]
    fn decodes_microsoft_dbx_without_altering_signature_data() {
        let bytes = include_bytes!("../../../../tests/fixtures/firmware/certs/dbx-x64-v1.7.2.bin");
        let mut lists = signature_lists(bytes);
        let decoded = lists.next().unwrap().unwrap();
        assert!(lists.next().is_none());
        assert_eq!(decoded.kind, SignatureType::Sha256);
        assert_eq!(decoded.signatures().count(), 445);
        let first = decoded.signatures().next().unwrap();
        assert_eq!(first.bytes(), &bytes[28..76]);
        assert_eq!(first.data(), &bytes[44..76]);
    }

    #[test]
    fn decodes_concatenated_x509_and_revocation_lists() {
        let cert =
            include_bytes!("../../../../tests/fixtures/firmware/certs/windows-uefi-ca-2023.der");
        let revocation = [5; 48];
        let mut bytes = list(SignatureType::X509, cert);
        bytes.extend(list(SignatureType::X509Sha256, &revocation));
        let mut decoded = signature_lists(&bytes).map(Result::unwrap);
        let x509 = decoded.next().unwrap();
        let revoked = decoded.next().unwrap();
        assert_eq!(x509.kind, SignatureType::X509);
        assert_eq!(x509.signatures().next().unwrap().data(), cert);
        assert_eq!(revoked.kind, SignatureType::X509Sha256);
        assert_eq!(revoked.signatures().next().unwrap().data(), revocation);
        assert!(decoded.next().is_none());
    }

    #[test]
    fn accepts_an_empty_list_and_continues_to_the_next_list() {
        let populated = list(SignatureType::Sha256, &[5; 32]);
        let mut bytes = populated[..LIST_HEADER_BYTES].to_vec();
        bytes[16..20].copy_from_slice(&(LIST_HEADER_BYTES as u32).to_le_bytes());
        bytes.extend(populated);
        let mut lists = signature_lists(&bytes).map(Result::unwrap);
        assert_eq!(lists.next().unwrap().signatures().count(), 0);
        assert_eq!(lists.next().unwrap().signatures().count(), 1);
        assert!(lists.next().is_none());
    }

    #[test]
    fn rejects_unknown_truncated_and_misaligned_lists() {
        let valid = list(SignatureType::Sha256, &[1; 32]);
        for length in 1..valid.len() {
            assert_eq!(
                signature_lists(&valid[..length])
                    .next()
                    .unwrap()
                    .unwrap_err(),
                ListError::InvalidLayout
            );
        }
        let mut invalid = valid.clone();
        invalid[0] ^= 1;
        assert_eq!(
            signature_lists(&invalid).next().unwrap().unwrap_err(),
            ListError::UnsupportedType
        );
        for (offset, value) in [
            (16, 27),
            (16, 75),
            (16, u32::MAX),
            (20, 1),
            (24, 0),
            (24, 47),
            (24, u32::MAX),
        ] {
            let mut invalid = valid.clone();
            invalid[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert_eq!(
                signature_lists(&invalid).next().unwrap().unwrap_err(),
                ListError::InvalidLayout
            );
        }
        let mut bytes = valid.clone();
        bytes.push(0);
        let mut decoded = signature_lists(&bytes);
        assert!(decoded.next().unwrap().is_ok());
        assert_eq!(
            decoded.next().unwrap().unwrap_err(),
            ListError::InvalidLayout
        );
        assert!(decoded.next().is_none());
    }
}
