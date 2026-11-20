//! Image authorization against the enrolled db and dbx.

use alloc::vec::Vec;
use der::Decode;
use x509_cert::Certificate;

use crate::{
    image_execution::RejectedImageAction,
    pe::{DigestAlgorithm, PeImage, authenticode_digest},
    pkcs7::AuthenticodeSignedData,
    siglist::{Signature, SignatureType, signature_lists},
};

use super::{Candidate, IssuerPath, certificate_tbs_hash, identify_signer};

const MAX_SIGNATURES: usize = 16;
const MAX_NESTED_DEPTH: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImagePolicyError {
    InvalidImage,
    InvalidDatabase,
    IncompleteSignatureInventory,
    RevokedImage { signed: bool },
    RevokedCertificate,
    InvalidSignature,
    Unsigned,
    Untrusted,
}

impl ImagePolicyError {
    /// Preserve the result of the authorization attempt for the UEFI execution table.
    pub fn execution_action(self) -> RejectedImageAction {
        match self {
            Self::Unsigned | Self::RevokedImage { signed: false } => RejectedImageAction::Untested,
            Self::InvalidSignature | Self::RevokedCertificate => {
                RejectedImageAction::SignatureFailed
            }
            Self::Untrusted => RejectedImageAction::SignatureNotFound,
            Self::RevokedImage { signed: true } => RejectedImageAction::SignatureFound,
            Self::InvalidImage | Self::InvalidDatabase | Self::IncompleteSignatureInventory => {
                RejectedImageAction::PolicyFailed
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ImageAuthority<'a> {
    pub kind: SignatureType,
    /// Complete enrolled EFI_SIGNATURE_DATA, including its owner GUID.
    pub signature: Signature<'a>,
}

/// Authorizes an external PE32+ image using UEFI 2.11 §32.5.3.3 and ADR-0011.
///
/// All signatures are inventoried before an authority is returned. A signature envelope outside
/// the supported RSA/SHA-256 profile prevents a complete inventory and denies the image.
/// The database slices come from the protected variable store, which bounds each to 64 KiB.
pub fn authorize_image<'db>(
    file: &[u8],
    db: &'db [u8],
    dbx: &[u8],
) -> Result<ImageAuthority<'db>, ImagePolicyError> {
    let image = PeImage::parse(file).map_err(|_| ImagePolicyError::InvalidImage)?;
    let digest = authenticode_digest(&image, DigestAlgorithm::Sha256)
        .map_err(|_| ImagePolicyError::InvalidImage)?;
    let revocations = Revocations::read(dbx, &digest)?;
    let mut policy = ImagePolicy::read(db, &digest, &revocations)?;
    let signed = image.certificate_file_range().is_some();
    if revocations.image {
        // An execution record reports a found signature only after validating
        // the WIN_CERTIFICATE framing. The image hash still vetoes execution.
        for (index, entry) in image.win_certificates().enumerate() {
            if index == MAX_SIGNATURES || entry.is_err() {
                return Err(ImagePolicyError::IncompleteSignatureInventory);
            }
        }
        return Err(ImagePolicyError::RevokedImage { signed });
    }
    for entry in image.win_certificates() {
        let entry = entry.map_err(|_| ImagePolicyError::IncompleteSignatureInventory)?;
        if !entry.is_authenticode() {
            return Err(ImagePolicyError::IncompleteSignatureInventory);
        }
        policy.scan_signature(entry.data, 0)?;
    }
    policy.authority.ok_or_else(|| policy.rejection(signed))
}

struct RevokedCertificate<'a> {
    certificate: Certificate,
    tbs_hash: [u8; 32],
    issuer: Option<Candidate<'a>>,
}

impl RevokedCertificate<'_> {
    fn matches(&self, presented: &Candidate<'_>) -> bool {
        let enrolled = self.certificate.tbs_certificate();
        let presented_cert = presented.certificate.tbs_certificate();
        self.tbs_hash == presented.tbs_hash
            && enrolled.issuer() == presented_cert.issuer()
            && enrolled.serial_number() == presented_cert.serial_number()
    }
}

fn may_be_issuer(certificate: &Certificate, child: &Candidate<'_>) -> bool {
    child.certificate.tbs_certificate().issuer() == certificate.tbs_certificate().subject()
}

struct Revocations<'a> {
    image: bool,
    certificates: Vec<RevokedCertificate<'a>>,
    tbs_hashes: Vec<&'a [u8]>,
}

impl<'a> Revocations<'a> {
    fn read(dbx: &'a [u8], image_digest: &[u8; 32]) -> Result<Self, ImagePolicyError> {
        let mut revocations = Self {
            image: false,
            certificates: Vec::new(),
            tbs_hashes: Vec::new(),
        };
        for list in signature_lists(dbx) {
            let list = list.map_err(|_| ImagePolicyError::InvalidDatabase)?;
            for signature in list.signatures() {
                let data = signature.data();
                match list.kind {
                    SignatureType::Sha256 => revocations.image |= data == image_digest,
                    SignatureType::X509Sha256 => {
                        // ADR-0011 applies the revocation unconditionally.
                        revocations.tbs_hashes.push(&data[..32]);
                    }
                    SignatureType::X509 => {
                        let certificate = Certificate::from_der(data)
                            .map_err(|_| ImagePolicyError::InvalidDatabase)?;
                        let tbs_hash = certificate_tbs_hash(data)
                            .map_err(|_| ImagePolicyError::InvalidDatabase)?;
                        revocations.certificates.push(RevokedCertificate {
                            certificate,
                            tbs_hash,
                            issuer: Candidate::parse(data).ok(),
                        });
                    }
                }
            }
        }
        Ok(revocations)
    }

    fn contains(&self, candidate: &Candidate<'_>) -> bool {
        self.tbs_hashes.contains(&candidate.tbs_hash.as_slice())
            || self.certificates.iter().any(|cert| cert.matches(candidate))
    }
}

struct TrustedCertificate<'a> {
    signature: Signature<'a>,
    certificate: Candidate<'a>,
}

struct ImagePolicy<'db, 'check> {
    digest: &'check [u8; 32],
    revocations: &'check Revocations<'check>,
    certificates: Vec<TrustedCertificate<'db>>,
    unsupported_issuers: Vec<Certificate>,
    authority: Option<ImageAuthority<'db>>,
    failed_signature: bool,
    remaining_signatures: usize,
}

impl<'db, 'check> ImagePolicy<'db, 'check> {
    fn rejection(&self, signed: bool) -> ImagePolicyError {
        if !signed {
            ImagePolicyError::Unsigned
        } else if self.failed_signature {
            ImagePolicyError::InvalidSignature
        } else {
            ImagePolicyError::Untrusted
        }
    }

    fn read(
        db: &'db [u8],
        digest: &'check [u8; 32],
        revocations: &'check Revocations<'check>,
    ) -> Result<Self, ImagePolicyError> {
        let mut policy = Self {
            digest,
            revocations,
            certificates: Vec::new(),
            unsupported_issuers: Vec::new(),
            authority: None,
            failed_signature: false,
            remaining_signatures: MAX_SIGNATURES,
        };
        for list in signature_lists(db) {
            let list = list.map_err(|_| ImagePolicyError::InvalidDatabase)?;
            for signature in list.signatures() {
                let data = signature.data();
                match list.kind {
                    SignatureType::Sha256 if data == digest && policy.authority.is_none() => {
                        policy.authority = Some(ImageAuthority {
                            kind: list.kind,
                            signature,
                        });
                    }
                    SignatureType::X509 => {
                        let parsed = Certificate::from_der(data)
                            .map_err(|_| ImagePolicyError::InvalidDatabase)?;
                        if let Ok(certificate) = Candidate::parse(data) {
                            policy.certificates.push(TrustedCertificate {
                                signature,
                                certificate,
                            });
                        } else {
                            policy.unsupported_issuers.push(parsed);
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(policy)
    }

    fn scan_signature(&mut self, data: &[u8], depth: usize) -> Result<(), ImagePolicyError> {
        let incomplete = ImagePolicyError::IncompleteSignatureInventory;
        if depth > MAX_NESTED_DEPTH {
            return Err(incomplete);
        }
        self.remaining_signatures = self.remaining_signatures.checked_sub(1).ok_or(incomplete)?;
        let signed = AuthenticodeSignedData::decode(data).map_err(|_| incomplete)?;
        let signer = signed.signer();
        let chain = identify_signer(signer, signed.certificates()).map_err(|_| incomplete)?;
        let enrolled = self.certificates.iter().map(|trusted| &trusted.certificate);
        let external = enrolled.clone().chain(
            self.revocations
                .certificates
                .iter()
                .filter_map(|cert| cert.issuer.as_ref()),
        );
        let mut incomplete_issuer = false;
        let revoked =
            chain.find_with_issuers(external.clone(), IssuerPath::Revocation, |candidate, _| {
                if self.revocations.contains(candidate) {
                    return Some(());
                }
                incomplete_issuer |= self
                    .unsupported_issuers
                    .iter()
                    .any(|issuer| may_be_issuer(issuer, candidate))
                    || self.revocations.certificates.iter().any(|revoked| {
                        revoked.issuer.is_none() && may_be_issuer(&revoked.certificate, candidate)
                    });
                if !candidate.certificate_signature_supported() {
                    incomplete_issuer |=
                        chain
                            .certificates
                            .iter()
                            .chain(external.clone())
                            .any(|issuer| {
                                candidate.der != issuer.der
                                    && may_be_issuer(&issuer.certificate, candidate)
                            });
                }
                None
            });
        if revoked.is_some() {
            return Err(ImagePolicyError::RevokedCertificate);
        }
        if incomplete_issuer {
            return Err(incomplete);
        }
        if self.authority.is_none() {
            if signed.image_digest() != self.digest
                || chain
                    .check_signature(signer.signature, signed.signature_digest())
                    .is_err()
            {
                self.failed_signature = true;
            } else {
                self.authority =
                    chain.find_with_issuers(enrolled, IssuerPath::CodeSigning, |candidate, _| {
                        self.certificates
                            .iter()
                            .find(|trusted| trusted.certificate.der == candidate.der)
                            .map(|trusted| ImageAuthority {
                                kind: SignatureType::X509,
                                signature: trusted.signature,
                            })
                    });
            }
        }
        for nested in signed.nested_signatures().map_err(|_| incomplete)? {
            self.scan_signature(nested.map_err(|_| incomplete)?, depth + 1)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{TEST_AUTHENTICODE, TEST_IMAGE_CA, TEST_IMAGE_DIGEST};
    use super::*;

    const NESTED: &[u8] = include_bytes!(
        "../../../../tests/fixtures/firmware/certs/authenticode-test-nested-sha256.p7b"
    );

    fn esl(kind: SignatureType, data: &[u8], owner: u8) -> Vec<u8> {
        let mut list = kind.guid_bytes().to_vec();
        let entry_size = 16 + data.len() as u32;
        list.extend((28 + entry_size).to_le_bytes());
        list.extend(0u32.to_le_bytes());
        list.extend(entry_size.to_le_bytes());
        list.extend([owner; 16]);
        list.extend(data);
        list
    }

    fn leaf(data: &[u8]) -> &[u8] {
        AuthenticodeSignedData::decode(data)
            .unwrap()
            .certificates()
            .next()
            .unwrap()
    }

    #[test]
    fn honors_certificate_identity_and_tbs_revocations_even_with_a_db_hash() {
        let allowed = esl(SignatureType::Sha256, &TEST_IMAGE_DIGEST, 7);
        let leaf = leaf(TEST_AUTHENTICODE);
        let mut changed_outer_signature = leaf.to_vec();
        *changed_outer_signature.last_mut().unwrap() ^= 1;
        let mut revoked_tbs = certificate_tbs_hash(leaf).unwrap().to_vec();
        revoked_tbs.extend([0x01; 16]);
        for denied in [
            esl(SignatureType::X509, &changed_outer_signature, 8),
            esl(SignatureType::X509Sha256, &revoked_tbs, 8),
            esl(SignatureType::X509, TEST_IMAGE_CA, 8),
        ] {
            let revoked = Revocations::read(&denied, &TEST_IMAGE_DIGEST).unwrap();
            let mut policy = ImagePolicy::read(&allowed, &TEST_IMAGE_DIGEST, &revoked).unwrap();
            assert!(policy.authority.is_some());
            assert_eq!(
                policy.scan_signature(TEST_AUTHENTICODE, 0),
                Err(ImagePolicyError::RevokedCertificate),
            );
        }
    }

    #[test]
    fn checks_nested_signatures_and_does_not_stop_at_a_trusted_signature() {
        let allowed = esl(SignatureType::X509, leaf(NESTED), 41);
        let denied = esl(SignatureType::X509, TEST_IMAGE_CA, 2);
        let revoked = Revocations::read(&denied, &TEST_IMAGE_DIGEST).unwrap();
        let mut policy = ImagePolicy::read(&allowed, &TEST_IMAGE_DIGEST, &revoked).unwrap();
        let primary = AuthenticodeSignedData::decode(NESTED).unwrap();
        let mut nested = primary.nested_signatures().unwrap();
        assert_eq!(nested.next(), Some(Ok(TEST_AUTHENTICODE)));
        assert!(nested.next().is_none());
        assert_eq!(
            policy.scan_signature(NESTED, 0),
            Err(ImagePolicyError::RevokedCertificate)
        );
        let primary_authority = policy.authority.unwrap();
        assert_eq!(primary_authority.kind, SignatureType::X509);
        assert_eq!(primary_authority.signature.bytes(), &allowed[28..]);

        let allowed = esl(SignatureType::X509, TEST_IMAGE_CA, 42);
        let denied = esl(SignatureType::X509, leaf(NESTED), 2);
        let revoked = Revocations::read(&denied, &TEST_IMAGE_DIGEST).unwrap();
        let mut policy = ImagePolicy::read(&allowed, &TEST_IMAGE_DIGEST, &revoked).unwrap();
        assert_eq!(policy.scan_signature(TEST_AUTHENTICODE, 0), Ok(()));
        assert_eq!(policy.authority.unwrap().signature.bytes(), &allowed[28..]);
        assert_eq!(
            policy.scan_signature(NESTED, 0),
            Err(ImagePolicyError::RevokedCertificate)
        );
    }

    #[test]
    fn revocation_scans_invalid_rsa_and_incomplete_inventory_fails_closed() {
        let signed = AuthenticodeSignedData::decode(TEST_AUTHENTICODE).unwrap();
        let mut corrupt = TEST_AUTHENTICODE.to_vec();
        let offset = corrupt
            .windows(signed.signer().signature.len())
            .position(|bytes| bytes == signed.signer().signature)
            .unwrap();
        corrupt[offset] ^= 1;
        let allowed = esl(SignatureType::Sha256, &TEST_IMAGE_DIGEST, 5);
        let denied = esl(SignatureType::X509, leaf(TEST_AUTHENTICODE), 8);
        let revoked = Revocations::read(&denied, &TEST_IMAGE_DIGEST).unwrap();
        let mut policy = ImagePolicy::read(&allowed, &TEST_IMAGE_DIGEST, &revoked).unwrap();
        assert_eq!(
            policy.scan_signature(&corrupt, 0),
            Err(ImagePolicyError::RevokedCertificate)
        );
        assert_eq!(
            policy.scan_signature(&TEST_AUTHENTICODE[..20], 0),
            Err(ImagePolicyError::IncompleteSignatureInventory),
        );

        let revoked = Revocations::read(&[], &TEST_IMAGE_DIGEST).unwrap();
        let mut untrusted = ImagePolicy::read(&[], &TEST_IMAGE_DIGEST, &revoked).unwrap();
        assert_eq!(untrusted.rejection(false), ImagePolicyError::Unsigned);
        assert_eq!(untrusted.scan_signature(TEST_AUTHENTICODE, 0), Ok(()));
        assert!(untrusted.authority.is_none());
        assert_eq!(untrusted.rejection(true), ImagePolicyError::Untrusted);
        assert_eq!(untrusted.scan_signature(&corrupt, 0), Ok(()));
        assert!(untrusted.authority.is_none());
        assert_eq!(
            untrusted.rejection(true),
            ImagePolicyError::InvalidSignature
        );

        let mut policy = ImagePolicy::read(&allowed, &TEST_IMAGE_DIGEST, &revoked).unwrap();
        assert_eq!(policy.scan_signature(&corrupt, 0), Ok(()));
        assert_eq!(
            policy.scan_signature(NESTED, MAX_NESTED_DEPTH),
            Err(ImagePolicyError::IncompleteSignatureInventory),
        );
        policy.remaining_signatures = 1;
        assert_eq!(
            policy.scan_signature(NESTED, 0),
            Err(ImagePolicyError::IncompleteSignatureInventory),
        );
    }

    #[test]
    fn unsupported_external_issuer_suppresses_hash_authority_in_either_database() {
        let mut unsupported_ca = TEST_IMAGE_CA.to_vec();
        let critical_key_usage = [0x06, 3, 0x55, 0x1d, 0x0f, 1, 1, 0xff];
        let offset = unsupported_ca
            .windows(critical_key_usage.len())
            .position(|bytes| bytes == critical_key_usage)
            .unwrap();
        unsupported_ca[offset + 4] = 0x7f;
        assert!(Certificate::from_der(&unsupported_ca).is_ok());
        assert!(Candidate::parse(&unsupported_ca).is_err());
        let unsupported = esl(SignatureType::X509, &unsupported_ca, 9);
        let hash = esl(SignatureType::Sha256, &TEST_IMAGE_DIGEST, 8);
        let db_with_ca = [hash.clone(), unsupported.clone()].concat();
        for (db, dbx) in [(&db_with_ca[..], &[][..]), (&hash[..], &unsupported[..])] {
            let revocations = Revocations::read(dbx, &TEST_IMAGE_DIGEST).unwrap();
            let mut policy = ImagePolicy::read(db, &TEST_IMAGE_DIGEST, &revocations).unwrap();
            assert!(policy.authority.is_some());
            assert_eq!(
                policy.scan_signature(TEST_AUTHENTICODE, 0),
                Err(ImagePolicyError::IncompleteSignatureInventory),
            );
        }
    }

    #[test]
    fn unsupported_certificate_signature_cannot_hide_an_enrolled_issuer() {
        let mut signed = TEST_AUTHENTICODE.to_vec();
        let original = leaf(TEST_AUTHENTICODE);
        let start = signed
            .windows(original.len())
            .position(|bytes| bytes == original)
            .unwrap();
        let sha256_rsa = [0x06, 9, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 1, 1, 0x0b];
        let offsets: Vec<_> = original
            .windows(sha256_rsa.len())
            .enumerate()
            .filter_map(|(offset, bytes)| (bytes == sha256_rsa).then_some(offset))
            .collect();
        assert_eq!(offsets.len(), 2);
        for offset in offsets {
            // A SHA-384 certificate signature falls outside the current chain profile.
            signed[start + offset + sha256_rsa.len() - 1] = 0x0c;
        }
        let allowed = [
            esl(SignatureType::Sha256, &TEST_IMAGE_DIGEST, 7),
            esl(SignatureType::X509, TEST_IMAGE_CA, 8),
        ]
        .concat();
        let revoked = Revocations::read(&[], &TEST_IMAGE_DIGEST).unwrap();
        let mut policy = ImagePolicy::read(&allowed, &TEST_IMAGE_DIGEST, &revoked).unwrap();
        assert_eq!(
            policy.scan_signature(&signed, 0),
            Err(ImagePolicyError::IncompleteSignatureInventory),
        );
    }

    #[test]
    fn revokes_a_cryptographic_issuer_that_cannot_authorize_code_signing() {
        let mut issuer = TEST_IMAGE_CA.to_vec();
        let usage = [0x04, 4, 0x03, 2, 2, 4];
        let offset = issuer
            .windows(usage.len())
            .position(|bytes| bytes == usage)
            .unwrap();
        issuer[offset + 4..offset + 6].copy_from_slice(&[7, 0x80]); // digitalSignature only
        let candidate = Candidate::parse(&issuer).unwrap();
        assert!(candidate.issuer_max_path.is_none());

        let allowed = esl(SignatureType::Sha256, &TEST_IMAGE_DIGEST, 7);
        let denied = esl(SignatureType::X509, &issuer, 8);
        let revocations = Revocations::read(&denied, &TEST_IMAGE_DIGEST).unwrap();
        let mut policy = ImagePolicy::read(&allowed, &TEST_IMAGE_DIGEST, &revocations).unwrap();
        assert_eq!(
            policy.scan_signature(TEST_AUTHENTICODE, 0),
            Err(ImagePolicyError::RevokedCertificate)
        );

        let revocations = Revocations::read(&[], &TEST_IMAGE_DIGEST).unwrap();
        let mut policy = ImagePolicy::read(&denied, &TEST_IMAGE_DIGEST, &revocations).unwrap();
        assert_eq!(policy.scan_signature(TEST_AUTHENTICODE, 0), Ok(()));
        assert!(policy.authority.is_none());
    }
}
