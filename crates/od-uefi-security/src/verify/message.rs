//! Trust policy for the UEFI PKCS7 Verify Protocol. All authority is caller supplied.

use alloc::vec::Vec;
use core::time::Duration;
use der::{DateTime, Encode};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha384, Sha512};

use super::{
    Candidate, CertificatePurpose, Error, IssuerPath, SignerChain, certificate_tbs,
    identify_signer_for, same_signer_serial,
};
use crate::{
    authvar::Timestamp,
    pkcs7::{
        MAX_DATABASE_BYTES, MessageSignedData, SPC_INDIRECT_DATA,
        timestamp::{self, CertificateHash, TimestampToken},
    },
    siglist::{ListError, SignatureType, signature_lists_with},
};

const MAX_CERTIFICATES: usize = 32;
const OWNER_BYTES: usize = 16;
const TIME_BYTES: usize = 16;
const SHA384_GUID: [u8; 16] = [
    0x07, 0x53, 0x3e, 0xff, 0xd0, 0x9f, 0xc9, 0x48, 0x85, 0xf1, 0x8a, 0xd5, 0x6c, 0x70, 0x1e, 0x01,
];
const SHA512_GUID: [u8; 16] = [
    0xae, 0x0f, 0x3e, 0x09, 0xc4, 0xa6, 0x50, 0x4f, 0x9f, 0x1b, 0xd4, 0x1e, 0x2b, 0x89, 0xc1, 0x9a,
];
const X509_SHA384_GUID: [u8; 16] = [
    0x6e, 0x87, 0x76, 0x70, 0xc2, 0x80, 0xe6, 0x4e, 0xaa, 0xd2, 0x28, 0xb3, 0x49, 0xa6, 0x86, 0x5b,
];
const X509_SHA512_GUID: [u8; 16] = [
    0x63, 0xbf, 0x6d, 0x44, 0x02, 0x25, 0xda, 0x4c, 0xbc, 0xfa, 0x24, 0x65, 0xd2, 0xb0, 0xfe, 0x9d,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageVerificationError {
    UnsupportedSignature,
    ConflictingContent,
    MissingContent,
    CompromisedData,
    InvalidDatabase,
    Unauthorized,
}

/// The input selected by the protocol method. The detached SHA-256 is computed over the
/// caller's message bytes. VerifySignature trusts the caller to supply that digest.
#[derive(Clone, Copy, Debug)]
pub enum MessageSource {
    Embedded,
    Detached([u8; 32]),
    Digest([u8; 32]),
}

#[derive(Clone, Copy, Debug)]
enum Entry {
    Certificate,
    ContentHash(usize),
    CertificateHash(usize),
}

fn entry_type(guid: &[u8], entry_size: usize) -> Result<Entry, ListError> {
    let kind = if guid == SignatureType::X509.guid_bytes() {
        Entry::Certificate
    } else if guid == SignatureType::Sha256.guid_bytes() {
        Entry::ContentHash(32)
    } else if guid == SHA384_GUID {
        Entry::ContentHash(48)
    } else if guid == SHA512_GUID {
        Entry::ContentHash(64)
    } else if guid == SignatureType::X509Sha256.guid_bytes() {
        Entry::CertificateHash(32)
    } else if guid == X509_SHA384_GUID {
        Entry::CertificateHash(48)
    } else if guid == X509_SHA512_GUID {
        Entry::CertificateHash(64)
    } else {
        return Err(ListError::UnsupportedType);
    };
    let valid = match kind {
        Entry::Certificate => entry_size > OWNER_BYTES,
        Entry::ContentHash(size) => entry_size == OWNER_BYTES + size,
        Entry::CertificateHash(size) => entry_size == OWNER_BYTES + size + TIME_BYTES,
    };
    valid.then_some(kind).ok_or(ListError::InvalidLayout)
}

#[derive(Default)]
struct Revocations<'a> {
    certificates: Vec<Candidate<'a>>,
    tbs_hashes: Vec<CertificateRevocation<'a>>,
    message: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Revocation {
    Always,
    After(Duration),
    Clear,
}

#[derive(Clone, Copy)]
struct CertificateRevocation<'a> {
    digest: &'a [u8],
    revocation: Revocation,
}

impl<'a> Revocations<'a> {
    fn read(
        database: &'a [u8],
        digest: &[u8; 32],
        purpose: CertificatePurpose,
    ) -> Result<Self, MessageVerificationError> {
        check_size(database)?;
        let mut revoked = Self::default();
        for list in signature_lists_with(database, entry_type) {
            let list = list.map_err(|_| MessageVerificationError::InvalidDatabase)?;
            for entry in list.signatures() {
                let data = entry.data();
                match list.kind {
                    Entry::Certificate => {
                        push_certificate(&mut revoked.certificates, data, purpose)?
                    }
                    Entry::ContentHash(32) => revoked.message |= data == digest,
                    Entry::ContentHash(_) => return Err(MessageVerificationError::InvalidDatabase),
                    Entry::CertificateHash(size) => {
                        revoked.tbs_hashes.push(certificate_hash_entry(data, size)?);
                    }
                }
            }
        }
        Ok(revoked)
    }

    fn find(&self, certificate: &Candidate<'_>) -> Revocation {
        if self.certificates.iter().any(|other| {
            certificate.tbs_hash == other.tbs_hash
                && certificate.certificate.tbs_certificate().issuer()
                    == other.certificate.tbs_certificate().issuer()
                && certificate.certificate.tbs_certificate().serial_number()
                    == other.certificate.tbs_certificate().serial_number()
        }) {
            return Revocation::Always;
        }
        let mut sha384 = None;
        let mut sha512 = None;
        self.tbs_hashes
            .iter()
            .filter(|entry| match entry.digest.len() {
                32 => entry.digest == certificate.tbs_hash,
                48 => sha384
                    .get_or_insert_with(|| certificate_tbs(certificate.der).map(Sha384::digest))
                    .as_ref()
                    .is_ok_and(|hash| entry.digest == hash.as_slice()),
                64 => sha512
                    .get_or_insert_with(|| certificate_tbs(certificate.der).map(Sha512::digest))
                    .as_ref()
                    .is_ok_and(|hash| entry.digest == hash.as_slice()),
                _ => false,
            })
            .map(|entry| entry.revocation)
            .min()
            .unwrap_or(Revocation::Clear)
    }

    fn find_chain<'cert>(
        &'cert self,
        chain: &'cert SignerChain<'cert>,
        anchors: &'cert [Candidate<'cert>],
    ) -> Revocation {
        let mut revocation = Revocation::Clear;
        chain.find_with_issuers(
            anchors.iter().chain(&self.certificates),
            IssuerPath::Revocation,
            |candidate, _| {
                revocation = revocation.min(self.find(candidate));
                (revocation == Revocation::Always).then_some(())
            },
        );
        revocation
    }
}

fn certificate_hash_entry(
    data: &[u8],
    digest_size: usize,
) -> Result<CertificateRevocation<'_>, MessageVerificationError> {
    let (digest, time) = data
        .split_at_checked(digest_size)
        .ok_or(MessageVerificationError::InvalidDatabase)?;
    let time = time
        .try_into()
        .map_err(|_| MessageVerificationError::InvalidDatabase)?;
    let revocation = revocation_time(time).ok_or(MessageVerificationError::InvalidDatabase)?;
    Ok(CertificateRevocation { digest, revocation })
}

fn revocation_time(time: &[u8; TIME_BYTES]) -> Option<Revocation> {
    if *time == [0; TIME_BYTES] {
        return Some(Revocation::Always);
    }
    let mut calendar = [0; TIME_BYTES];
    calendar[..7].copy_from_slice(&time[..7]);
    let nanos = u32::from_le_bytes(time[8..12].try_into().unwrap());
    let zone = i16::from_le_bytes([time[12], time[13]]);
    Timestamp::from_bytes(&calendar)?;
    if time[7] != 0
        || time[15] != 0
        || nanos >= 1_000_000_000
        || (!(-1440..=1440).contains(&zone) && zone != 2047)
        || time[14] & !3 != 0
    {
        return None;
    }
    // An unspecified time zone cannot establish a UTC deadline. Advisory daylight rules
    // are outside the rescue profile and leave the entry effective unconditionally.
    if zone == 2047 || time[14] != 0 {
        return Some(Revocation::Always);
    }
    let year = u16::from_le_bytes([time[0], time[1]]);
    let Ok(date) = DateTime::new(year, time[2], time[3], time[4], time[5], time[6]) else {
        return Some(Revocation::Always);
    };
    let local = Duration::new(date.unix_duration().as_secs(), nanos);
    let offset = Duration::from_secs(u64::from(zone.unsigned_abs()) * 60);
    let utc = if zone < 0 {
        local.checked_sub(offset)
    } else {
        local.checked_add(offset)
    };
    Some(utc.map_or(Revocation::Always, Revocation::After))
}

fn check_size(database: &[u8]) -> Result<(), MessageVerificationError> {
    if database.len() > MAX_DATABASE_BYTES {
        return Err(MessageVerificationError::InvalidDatabase);
    }
    Ok(())
}

fn push_certificate<'a>(
    certificates: &mut Vec<Candidate<'a>>,
    der: &'a [u8],
    purpose: CertificatePurpose,
) -> Result<(), MessageVerificationError> {
    if certificates.iter().any(|candidate| candidate.der == der) {
        return Ok(());
    }
    if certificates.len() == MAX_CERTIFICATES {
        return Err(MessageVerificationError::InvalidDatabase);
    }
    let certificate = Candidate::parse_for(der, purpose)
        .map_err(|_| MessageVerificationError::InvalidDatabase)?;
    certificates.push(certificate);
    Ok(())
}

fn read_anchors(
    database: &[u8],
    purpose: CertificatePurpose,
) -> Result<Vec<Candidate<'_>>, MessageVerificationError> {
    check_size(database)?;
    let mut anchors = Vec::new();
    for list in signature_lists_with(database, entry_type) {
        let list = list.map_err(|_| MessageVerificationError::InvalidDatabase)?;
        if matches!(purpose, CertificatePurpose::Timestamp)
            && !matches!(list.kind, Entry::Certificate)
        {
            return Err(MessageVerificationError::InvalidDatabase);
        }
        for entry in list.signatures() {
            match list.kind {
                Entry::Certificate => push_certificate(&mut anchors, entry.data(), purpose)?,
                Entry::CertificateHash(size) => {
                    certificate_hash_entry(entry.data(), size)?;
                }
                Entry::ContentHash(_) => {}
            }
        }
    }
    Ok(anchors)
}

/// Verifies one RSA-2048/SHA-256 SignedData envelope. Databases are exact concatenations of
/// EFI_SIGNATURE_LIST entries supplied by the caller. The returned bytes are the embedded
/// content, when selected and authenticated. Revocations apply before authority is accepted.
pub fn verify_message<'a>(
    encoded: &'a [u8],
    source: MessageSource,
    allowed_db: &[u8],
    revoked_db: &[u8],
) -> Result<Option<&'a [u8]>, MessageVerificationError> {
    verify_message_with_timestamps(encoded, source, allowed_db, revoked_db, &[])
}

/// Verifies the message with caller-supplied RFC 3161 timestamp authorities.
pub fn verify_message_with_timestamps<'a>(
    encoded: &'a [u8],
    source: MessageSource,
    allowed_db: &[u8],
    revoked_db: &[u8],
    timestamp_db: &[u8],
) -> Result<Option<&'a [u8]>, MessageVerificationError> {
    use MessageVerificationError as Failure;
    let message = MessageSignedData::decode(encoded).map_err(|_| Failure::UnsupportedSignature)?;
    let (digest, embedded) = match source {
        MessageSource::Embedded => {
            let embedded = message.embedded().ok_or(Failure::MissingContent)?;
            (Sha256::digest(embedded).into(), Some(embedded))
        }
        MessageSource::Detached(digest) => {
            if message.embedded().is_some() {
                return Err(Failure::ConflictingContent);
            }
            (digest, None)
        }
        MessageSource::Digest(digest) => (digest, None),
    };
    let signature_digest = message
        .signature_digest(digest)
        .ok_or(Failure::CompromisedData)?;
    let purpose = if message.content_type() == SPC_INDIRECT_DATA {
        CertificatePurpose::AuthenticodeMessage
    } else {
        CertificatePurpose::SignedMessage
    };
    let chain = identify_signer_for(message.signer(), message.certificates(), purpose).map_err(
        |error| match error {
            Error::UntrustedSigner => Failure::Unauthorized,
            _ => Failure::UnsupportedSignature,
        },
    )?;
    if !chain.certificates[chain.signer].digital_signature {
        return Err(Failure::Unauthorized);
    }
    chain
        .check_signature(message.signer().signature, &signature_digest)
        .map_err(|_| Failure::CompromisedData)?;

    let allowed = read_anchors(allowed_db, purpose)?;
    let revoked = Revocations::read(revoked_db, &digest, purpose)?;
    let timestamps = read_anchors(timestamp_db, CertificatePurpose::Timestamp)?;
    if revoked.message {
        return Err(Failure::Unauthorized);
    }
    chain
        .find_with_issuers(allowed.iter(), IssuerPath::Authorized, |candidate, _| {
            allowed
                .iter()
                .any(|anchor| anchor.der == candidate.der)
                .then_some(())
        })
        .ok_or(Failure::Unauthorized)?;
    match revoked.find_chain(&chain, &allowed) {
        Revocation::Clear => Ok(embedded),
        Revocation::After(cutoff)
            if !timestamps.is_empty()
                && timestamp::tokens(&message).is_ok_and(|tokens| {
                    tokens.into_iter().any(|token| {
                        trusted_timestamp(
                            token,
                            message.signer().signature,
                            cutoff,
                            &timestamps,
                            &revoked,
                        )
                    })
                }) =>
        {
            Ok(embedded)
        }
        _ => Err(Failure::Unauthorized),
    }
}

fn trusted_timestamp(
    encoded: &[u8],
    signature: &[u8],
    cutoff: Duration,
    anchors: &[Candidate<'_>],
    revoked: &Revocations<'_>,
) -> bool {
    let Ok(token) = TimestampToken::decode(encoded, signature) else {
        return false;
    };
    if token.latest_time >= cutoff {
        return false;
    }
    let Ok(chain) = identify_signer_for(
        token.signer,
        token.certificates(),
        CertificatePurpose::Timestamp,
    ) else {
        return false;
    };
    let signer = &chain.certificates[chain.signer];
    let tbs = signer.certificate.tbs_certificate();
    let matches_hash = match token.certificate_hash {
        CertificateHash::Sha1(hash) => hash == Sha1::digest(signer.der).as_slice(),
        CertificateHash::Sha256(hash) => hash == Sha256::digest(signer.der).as_slice(),
    };
    matches_hash
        && token.issuer_serial.is_none_or(|identifier| {
            same_signer_serial(tbs.serial_number().as_bytes(), identifier.serial)
                && tbs
                    .issuer()
                    .to_der()
                    .is_ok_and(|bytes| bytes == identifier.issuer)
        })
        && token
            .tsa_name
            .is_none_or(|name| tbs.subject().to_der().is_ok_and(|bytes| bytes == name))
        && signer.valid_at(token.time)
        && chain
            .check_signature(token.signer.signature, &token.signature_digest)
            .is_ok()
        && revoked.find_chain(&chain, anchors) == Revocation::Clear
        && chain
            .find_with_issuers(
                anchors.iter(),
                IssuerPath::AuthorizedAt(token.time),
                |candidate, _| {
                    anchors
                        .iter()
                        .any(|anchor| anchor.der == candidate.der)
                        .then_some(())
                },
            )
            .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkcs7::raw_sequence;
    use crate::verify::tests::{TEST_AUTHENTICODE, TEST_IMAGE_CA};
    use alloc::vec;
    use der::SliceReader;

    #[test]
    fn revocation_times_normalize_efi_offset_and_preserve_nanoseconds() {
        let mut time = [0; 16];
        time[..2].copy_from_slice(&1970u16.to_le_bytes());
        time[2..7].copy_from_slice(&[1, 1, 1, 2, 3]);
        time[8..12].copy_from_slice(&123u32.to_le_bytes());
        time[12..14].copy_from_slice(&(-60i16).to_le_bytes());
        assert_eq!(
            revocation_time(&time),
            Some(Revocation::After(Duration::new(123, 123)))
        );
        time[12..14].copy_from_slice(&60i16.to_le_bytes());
        assert_eq!(
            revocation_time(&time),
            Some(Revocation::After(Duration::new(7323, 123)))
        );
        time[12..14].copy_from_slice(&2047i16.to_le_bytes());
        assert_eq!(revocation_time(&time), Some(Revocation::Always));
        time[15] = 1;
        assert_eq!(revocation_time(&time), None);
        assert_eq!(revocation_time(&[0; 16]), Some(Revocation::Always));
    }

    fn list(guid: [u8; 16], data: &[u8]) -> Vec<u8> {
        let entry_size = OWNER_BYTES + data.len();
        let mut encoded = guid.to_vec();
        encoded.extend_from_slice(&(28 + entry_size as u32).to_le_bytes());
        encoded.extend_from_slice(&0u32.to_le_bytes());
        encoded.extend_from_slice(&(entry_size as u32).to_le_bytes());
        encoded.extend_from_slice(&[0; OWNER_BYTES]);
        encoded.extend_from_slice(data);
        encoded
    }

    fn signature() -> &'static [u8] {
        raw_sequence(&mut SliceReader::new(TEST_AUTHENTICODE).unwrap()).unwrap()
    }

    #[test]
    fn message_trust_accepts_a_direct_signer_or_an_external_ca() {
        let encoded = signature();
        let signed = MessageSignedData::decode(encoded).unwrap();
        let content = signed.embedded().unwrap();
        let digest = Sha256::digest(content).into();
        let leaf = signed.certificates().next().unwrap();
        for certificate in [leaf, TEST_IMAGE_CA] {
            let allowed = list(SignatureType::X509.guid_bytes(), certificate);
            assert_eq!(
                verify_message(encoded, MessageSource::Embedded, &allowed, &[]),
                Ok(Some(content))
            );
            assert_eq!(
                verify_message(encoded, MessageSource::Digest(digest), &allowed, &[]),
                Ok(None)
            );
            assert_eq!(
                verify_message(encoded, MessageSource::Detached(digest), &allowed, &[]),
                Err(MessageVerificationError::ConflictingContent)
            );
            assert_eq!(
                verify_message(encoded, MessageSource::Digest([0; 32]), &allowed, &[]),
                Err(MessageVerificationError::CompromisedData)
            );
        }
        let allowed = list(SignatureType::Sha256.guid_bytes(), &digest);
        assert_eq!(
            verify_message(encoded, MessageSource::Embedded, &allowed, &[]),
            Err(MessageVerificationError::Unauthorized)
        );
    }

    #[test]
    fn revocation_overrides_leaf_trust_for_certificates_and_all_supported_tbs_hashes() {
        let encoded = signature();
        let signed = MessageSignedData::decode(encoded).unwrap();
        let leaf = signed.certificates().next().unwrap();
        let allowed = list(SignatureType::X509.guid_bytes(), leaf);
        let digest = Sha256::digest(signed.embedded().unwrap());
        let mut revocations = vec![
            list(SignatureType::X509.guid_bytes(), TEST_IMAGE_CA),
            list(SignatureType::Sha256.guid_bytes(), &digest),
        ];
        for certificate in [leaf, TEST_IMAGE_CA] {
            let tbs = certificate_tbs(certificate).unwrap();
            for (guid, mut hash) in [
                (
                    SignatureType::X509Sha256.guid_bytes(),
                    Sha256::digest(tbs).to_vec(),
                ),
                (X509_SHA384_GUID, Sha384::digest(tbs).to_vec()),
                (X509_SHA512_GUID, Sha512::digest(tbs).to_vec()),
            ] {
                hash.extend_from_slice(&[0; TIME_BYTES]);
                if certificate == TEST_IMAGE_CA {
                    // The issuer must be present in one of the caller's or message's lists.
                    let trust_chain = [
                        allowed.as_slice(),
                        list(SignatureType::X509.guid_bytes(), TEST_IMAGE_CA).as_slice(),
                    ]
                    .concat();
                    assert_eq!(
                        verify_message(
                            encoded,
                            MessageSource::Embedded,
                            &trust_chain,
                            &list(guid, &hash)
                        ),
                        Err(MessageVerificationError::Unauthorized)
                    );
                } else {
                    revocations.push(list(guid, &hash));
                }
            }
        }
        for revoked in revocations {
            assert_eq!(
                verify_message(encoded, MessageSource::Embedded, &allowed, &revoked),
                Err(MessageVerificationError::Unauthorized)
            );
        }
    }

    #[test]
    fn message_verification_separates_bad_signatures_and_malformed_databases() {
        let encoded = signature();
        let allowed = list(SignatureType::X509.guid_bytes(), TEST_IMAGE_CA);
        let signed = MessageSignedData::decode(encoded).unwrap();
        let mut tampered = encoded.to_vec();
        let signature_at = encoded
            .windows(256)
            .position(|part| part == signed.signer().signature)
            .unwrap();
        tampered[signature_at + 255] ^= 1;
        assert_eq!(
            verify_message(&tampered, MessageSource::Embedded, &allowed, &[]),
            Err(MessageVerificationError::CompromisedData)
        );
        assert_eq!(
            verify_message(
                encoded,
                MessageSource::Embedded,
                &allowed[..allowed.len() - 1],
                &[]
            ),
            Err(MessageVerificationError::InvalidDatabase)
        );
        assert_eq!(
            verify_message(
                encoded,
                MessageSource::Embedded,
                &allowed,
                &list([0; 16], &[0; 32])
            ),
            Err(MessageVerificationError::InvalidDatabase)
        );
        let mut invalid_cutoff = [0; 32 + TIME_BYTES];
        invalid_cutoff[32] = 1;
        let invalid = list(SignatureType::X509Sha256.guid_bytes(), &invalid_cutoff);
        assert_eq!(
            verify_message(encoded, MessageSource::Embedded, &allowed, &invalid,),
            Err(MessageVerificationError::InvalidDatabase)
        );
        assert_eq!(
            verify_message(
                encoded,
                MessageSource::Embedded,
                &[allowed, invalid].concat(),
                &[],
            ),
            Err(MessageVerificationError::InvalidDatabase)
        );
    }
}
