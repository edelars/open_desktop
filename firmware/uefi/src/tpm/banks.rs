//! The TCG2 bitmap and digest widths for the board's PCR banks.

use sha2::{Digest, Sha256, Sha384};

use crate::pe::{DigestAlgorithm, DigestError, PeImage, authenticode_digest};
use od_uefi_security::pe::authenticode_digest_sha384;

pub(super) const SHA256: u8 = 0x0b;
pub(super) const SHA384: u8 = 0x0c;
pub(super) const SHA256_BANK: u32 = 1 << 1;
pub(super) const SHA384_BANK: u32 = 1 << 2;
pub(super) const MAX_TAGGED_BYTES: usize = 2 + 32 + 2 + 48;

pub(super) const fn bank_bit(algorithm: u8) -> Option<u32> {
    match algorithm {
        SHA256 => Some(SHA256_BANK),
        SHA384 => Some(SHA384_BANK),
        _ => None,
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
#[repr(u32)]
pub(super) enum PcrBanks {
    Sha256 = SHA256_BANK,
    Sha384 = SHA384_BANK,
    Both = SHA256_BANK | SHA384_BANK,
}

impl PcrBanks {
    pub(super) fn from_bitmap(bitmap: u32) -> Option<Self> {
        const BOTH: u32 = SHA256_BANK | SHA384_BANK;
        match bitmap {
            SHA256_BANK => Some(Self::Sha256),
            SHA384_BANK => Some(Self::Sha384),
            BOTH => Some(Self::Both),
            _ => None,
        }
    }

    pub(super) fn algorithms(self) -> &'static [(u8, u16)] {
        match self {
            Self::Sha256 => &[(SHA256, 32)],
            Self::Sha384 => &[(SHA384, 48)],
            Self::Both => &[(SHA256, 32), (SHA384, 48)],
        }
    }
}

pub(super) struct EventDigests {
    banks: PcrBanks,
    sha256: [u8; 32],
    sha384: [u8; 48],
}

impl EventDigests {
    pub(super) fn zero(banks: PcrBanks) -> Self {
        Self {
            banks,
            sha256: [0; 32],
            sha384: [0; 48],
        }
    }

    pub(super) fn hash_parts(banks: PcrBanks, parts: &[&[u8]], sha256: Option<[u8; 32]>) -> Self {
        let mut digests = Self::zero(banks);
        if banks != PcrBanks::Sha384 {
            digests.sha256 = sha256.unwrap_or_else(|| digest::<Sha256, 32>(parts));
        }
        if banks != PcrBanks::Sha256 {
            digests.sha384 = digest::<Sha384, 48>(parts);
        }
        digests
    }

    pub(super) fn hash_image(banks: PcrBanks, image: &PeImage<'_>) -> Result<Self, DigestError> {
        let mut digests = Self::zero(banks);
        if banks != PcrBanks::Sha384 {
            digests.sha256 = authenticode_digest(image, DigestAlgorithm::Sha256)?;
        }
        if banks != PcrBanks::Sha256 {
            digests.sha384 = authenticode_digest_sha384(image)?;
        }
        Ok(digests)
    }

    pub(super) fn count(&self) -> u32 {
        self.banks.algorithms().len() as u32
    }

    /// Writes the digests after the count, in bank order. TPM commands are
    /// big-endian and Event2 records are little-endian.
    pub(super) fn write_tagged<const BIG_ENDIAN: bool>(&self, out: &mut [u8]) -> usize {
        const SHA256_BYTES: usize = 2 + 32;
        const SHA384_BYTES: usize = 2 + 48;
        let sha256_tag = if BIG_ENDIAN { [0, SHA256] } else { [SHA256, 0] };
        let sha384_tag = if BIG_ENDIAN { [0, SHA384] } else { [SHA384, 0] };
        if self.banks == PcrBanks::Sha384 {
            out[..2].copy_from_slice(&sha384_tag);
            out[2..SHA384_BYTES].copy_from_slice(&self.sha384);
            return SHA384_BYTES;
        }
        out[..2].copy_from_slice(&sha256_tag);
        out[2..SHA256_BYTES].copy_from_slice(&self.sha256);
        if self.banks == PcrBanks::Sha256 {
            return SHA256_BYTES;
        }
        out[SHA256_BYTES..SHA256_BYTES + 2].copy_from_slice(&sha384_tag);
        out[SHA256_BYTES + 2..MAX_TAGGED_BYTES].copy_from_slice(&self.sha384);
        MAX_TAGGED_BYTES
    }
}

fn digest<H: Digest, const N: usize>(parts: &[&[u8]]) -> [u8; N]
where
    [u8; N]: From<sha2::digest::Output<H>>,
{
    let mut hash = H::new();
    for part in parts {
        hash.update(part);
    }
    hash.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagged_digests_match_command_and_log_layouts() {
        for (banks, count, tagged) in [
            (PcrBanks::Sha256, 1, &[(11u8, 32usize)][..]),
            (PcrBanks::Sha384, 1, &[(12, 48)][..]),
            (PcrBanks::Both, 2, &[(11, 32), (12, 48)][..]),
        ] {
            let mut digests = EventDigests::zero(banks);
            digests.sha256.fill(0x25);
            digests.sha384.fill(0x38);
            assert_eq!(digests.count(), count);
            let mut command = [0xff; 88];
            let mut log = command;
            let command_len = digests.write_tagged::<true>(&mut command);
            let log_len = digests.write_tagged::<false>(&mut log);
            let mut at = 0;
            for &(algorithm, width) in tagged {
                let byte = if width == 32 { 0x25 } else { 0x38 };
                assert_eq!(command[at..at + 2], [0, algorithm]);
                assert_eq!(log[at..at + 2], [algorithm, 0]);
                at += 2;
                assert_eq!(command[at..at + width], [byte; 48][..width]);
                assert_eq!(log[at..at + width], [byte; 48][..width]);
                at += width;
            }
            assert_eq!((command_len, log_len), (at, at));
            assert_eq!(command[at..], [0xff; 88][..88 - at]);
            assert_eq!(log[at..], [0xff; 88][..88 - at]);
        }
    }
}
