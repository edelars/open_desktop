//! Verification of detached Auth2 signatures with the enrolled platform and exchange keys.

use alloc::{vec, vec::Vec};

use der::{Decode, Encode, asn1::ObjectIdentifier, oid::AssociatedOid};
use rsa::{Pkcs1v15Sign, PublicKey, pkcs8::DecodePublicKey, traits::PublicKeyParts};
use sha2::{Digest, Sha256};
use x509_cert::{
    Certificate,
    ext::pkix::{BasicConstraints, ExtendedKeyUsage, KeyUsage},
};

use crate::pkcs7::{
    AuthenticodeSignedData, DetachedSignedData, RSA_2048_SIGNATURE_BYTES, Signer, raw_sequence,
    sequence,
};

mod image_policy;
pub use image_policy::{
    ImageAuthority, ImagePolicyError, ImagePolicyFailure, authorize_image,
    authorize_image_with_evidence,
};

const SHA256_WITH_RSA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.11");
const CODE_SIGNING: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.3");
const ANY_EXTENDED_KEY_USAGE: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.37.0");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidCertificate,
    InvalidSignature,
    UntrustedSigner,
    AmbiguousSigner,
}

#[derive(Clone, Copy, Debug)]
pub enum TrustAnchor<'a> {
    /// The update must be signed by this key. Its issuer confers no authority.
    PlatformKey(&'a [u8]),
    /// The presented chain must terminate at this exact enrolled certificate.
    ExchangeKey(&'a [u8]),
}

pub fn validate_certificate(der: &[u8]) -> Result<(), Error> {
    Certificate::from_der(der)
        .map(|_| ())
        .map_err(|_| Error::InvalidCertificate)
}

pub fn same_rsa_public_key(left: &[u8], right: &[u8]) -> Result<bool, Error> {
    let left = Certificate::from_der(left).map_err(|_| Error::InvalidCertificate)?;
    let right = Certificate::from_der(right).map_err(|_| Error::InvalidCertificate)?;
    let left = public_key(&left)?;
    let right = public_key(&right)?;
    Ok(same_key(&left, &right))
}

fn same_key(left: &PublicKey, right: &PublicKey) -> bool {
    left.n() == right.n() && left.e() == right.e()
}

pub fn validate_anchor(anchor: TrustAnchor<'_>) -> Result<(), Error> {
    let allowed = match anchor {
        TrustAnchor::PlatformKey(bytes) => Candidate::parse(bytes)?.digital_signature,
        TrustAnchor::ExchangeKey(bytes) => {
            let certificate = Candidate::parse(bytes)?;
            certificate.digital_signature || certificate.issuer_max_path.is_some()
        }
    };
    if allowed {
        Ok(())
    } else {
        Err(Error::InvalidCertificate)
    }
}

struct Candidate<'a> {
    der: &'a [u8],
    tbs_hash: [u8; 32],
    certificate: Certificate,
    key: PublicKey,
    digital_signature: bool,
    issuer_max_path: Option<usize>,
}

impl<'a> Candidate<'a> {
    fn parse(der: &'a [u8]) -> Result<Self, Error> {
        let malformed = |_| Error::InvalidCertificate;
        let certificate = Certificate::from_der(der).map_err(malformed)?;
        let tbs = certificate.tbs_certificate();

        for extension in tbs.extensions().into_iter().flatten() {
            if extension.critical
                && ![BasicConstraints::OID, KeyUsage::OID, ExtendedKeyUsage::OID]
                    .contains(&extension.extn_id)
            {
                return Err(Error::InvalidCertificate);
            }
        }
        let usage = tbs
            .get_extension::<KeyUsage>()
            .map_err(malformed)?
            .map(|(_, usage)| usage);
        let extended = tbs.get_extension::<ExtendedKeyUsage>().map_err(malformed)?;
        let code_signing = extended.is_none_or(|(_, usage)| {
            usage.0.contains(&CODE_SIGNING) || usage.0.contains(&ANY_EXTENDED_KEY_USAGE)
        });
        let constraints = tbs.get_extension::<BasicConstraints>().map_err(malformed)?;
        let issuer_max_path = constraints
            .filter(|(_, constraints)| {
                constraints.ca && code_signing && usage.is_none_or(|usage| usage.key_cert_sign())
            })
            .map(|(_, constraints)| {
                constraints
                    .path_len_constraint
                    .map_or(usize::MAX, usize::from)
            });

        let key = public_key(&certificate)?;
        Ok(Self {
            der,
            tbs_hash: certificate_tbs_hash(der)?,
            certificate,
            key,
            digital_signature: code_signing && usage.is_none_or(|usage| usage.digital_signature()),
            issuer_max_path,
        })
    }

    fn certificate_signature_supported(&self) -> bool {
        let algorithm = self.certificate.signature_algorithm();
        let tbs = self.certificate.tbs_certificate();
        algorithm.oid == SHA256_WITH_RSA
            && algorithm
                .parameters
                .as_ref()
                .is_none_or(|param| param.is_null())
            && tbs.signature() == algorithm
    }

    fn is_signed_by(&self, issuer: &Self) -> bool {
        self.certificate_signature_supported()
            && self.certificate.tbs_certificate().issuer()
                == issuer.certificate.tbs_certificate().subject()
            && self
                .certificate
                .signature()
                .as_bytes()
                .is_some_and(|signature| {
                    issuer
                        .key
                        .verify(Pkcs1v15Sign::new::<Sha256>(), &self.tbs_hash, signature)
                        .is_ok()
                })
    }

    fn can_issue(&self, child: &Self, intermediates: usize) -> bool {
        self.issuer_max_path
            .is_some_and(|limit| intermediates <= limit)
            && child.is_signed_by(self)
    }

    fn is_self_issued(&self) -> bool {
        let tbs = self.certificate.tbs_certificate();
        tbs.issuer() == tbs.subject()
    }
}

fn certificate_tbs_hash(der: &[u8]) -> Result<[u8; 32], Error> {
    let tbs = sequence(der)
        .and_then(|mut fields| raw_sequence(&mut fields))
        .map_err(|_| Error::InvalidCertificate)?;
    Ok(Sha256::digest(tbs).into())
}

fn public_key(certificate: &Certificate) -> Result<PublicKey, Error> {
    let spki = certificate
        .tbs_certificate()
        .subject_public_key_info()
        .to_der()
        .map_err(|_| Error::InvalidCertificate)?;
    let key = PublicKey::from_public_key_der(&spki).map_err(|_| Error::InvalidCertificate)?;
    if key.n().bit_width() != (8 * RSA_2048_SIGNATURE_BYTES) as u32 {
        return Err(Error::InvalidCertificate);
    }
    Ok(key)
}

/// Verifies the update against the supplied enrolled PK/KEK anchors.
///
/// The caller chooses the anchors permitted for the variable. Certificate dates are
/// excluded by ADR-0011. `digest` covers the exact Auth2 signed fields.
pub fn detached(
    signed: &DetachedSignedData<'_>,
    digest: &[u8; 32],
    anchors: &[TrustAnchor<'_>],
) -> Result<(), Error> {
    let verified = verify_signer(signed.signer, signed.certificates(), digest)?;
    let leaf = &verified.certificates[verified.signer];
    for anchor in anchors {
        if let TrustAnchor::PlatformKey(bytes) = anchor {
            let certificate =
                Certificate::from_der(bytes).map_err(|_| Error::InvalidCertificate)?;
            let key = public_key(&certificate)?;
            if same_key(&leaf.key, &key) {
                return Ok(());
            }
        }
    }
    verified
        .find_authority(|child, _| {
            anchors
                .iter()
                .any(|anchor| matches!(anchor, TrustAnchor::ExchangeKey(bytes) if *bytes == child.der))
                .then_some(())
        })
        .ok_or(Error::UntrustedSigner)
}

/// Verifies an Authenticode signature and returns the index of the matching enrolled X.509
/// certificate. The image digest must be computed from the same PE bytes used by the loader.
/// An enrolled CA can complete a chain even when the PE does not embed the CA itself.
pub fn authenticode_signature(
    signed: &AuthenticodeSignedData<'_>,
    image_digest: &[u8; 32],
    enrolled_certificates: &[&[u8]],
) -> Result<usize, Error> {
    if signed.image_digest() != image_digest {
        return Err(Error::InvalidSignature);
    }
    let verified = verify_signer(
        signed.signer(),
        signed.certificates(),
        signed.signature_digest(),
    )?;
    let anchors = enrolled_certificates
        .iter()
        .map(|bytes| Candidate::parse(bytes).ok())
        .collect::<Vec<_>>();
    verified
        .find_authority(|child, intermediates| {
            anchors.iter().position(|anchor| {
                anchor.as_ref().is_some_and(|anchor| {
                    child.der == anchor.der || anchor.can_issue(child, intermediates)
                })
            })
        })
        .ok_or(Error::UntrustedSigner)
}

struct SignerChain<'a> {
    certificates: Vec<Candidate<'a>>,
    signer: usize,
}

fn verify_signer<'a>(
    signed: Signer<'a>,
    certificates: impl Iterator<Item = &'a [u8]>,
    digest: &[u8; 32],
) -> Result<SignerChain<'a>, Error> {
    let chain = identify_signer(signed, certificates)?;
    chain.check_signature(signed.signature, digest)?;
    Ok(chain)
}

fn identify_signer<'a>(
    signed: Signer<'a>,
    certificates: impl Iterator<Item = &'a [u8]>,
) -> Result<SignerChain<'a>, Error> {
    let certificates = certificates
        .map(Candidate::parse)
        .collect::<Result<Vec<_>, _>>()?;
    let mut signer = None;
    for (index, candidate) in certificates.iter().enumerate() {
        let tbs = candidate.certificate.tbs_certificate();
        if same_signer_serial(tbs.serial_number().as_bytes(), signed.serial)
            && tbs
                .issuer()
                .to_der()
                .map_err(|_| Error::InvalidCertificate)?
                == signed.issuer_der
        {
            if signer.replace(index).is_some() {
                return Err(Error::AmbiguousSigner);
            }
        }
    }
    let signer = signer.ok_or(Error::UntrustedSigner)?;
    Ok(SignerChain {
        certificates,
        signer,
    })
}

fn same_signer_serial(certificate: &[u8], unsigned_signer: &[u8]) -> bool {
    // X.509 preserves the DER INTEGER sign octet. SignerInfo uses a UintRef
    // and exposes the unsigned magnitude.
    match certificate {
        [0, magnitude @ ..] if !magnitude.is_empty() => magnitude == unsigned_signer,
        [first, ..] if first & 0x80 == 0 => certificate == unsigned_signer,
        _ => false,
    }
}

#[derive(Clone, Copy)]
enum IssuerPath {
    CodeSigning,
    Revocation,
}

impl SignerChain<'_> {
    fn check_signature(&self, signature: &[u8], digest: &[u8; 32]) -> Result<(), Error> {
        let leaf = &self.certificates[self.signer];
        if leaf.digital_signature
            && leaf
                .key
                .verify(Pkcs1v15Sign::new::<Sha256>(), digest, signature)
                .is_ok()
        {
            Ok(())
        } else {
            Err(Error::InvalidSignature)
        }
    }

    fn find_authority<T>(
        &self,
        authorize: impl FnMut(&Candidate<'_>, usize) -> Option<T>,
    ) -> Option<T> {
        self.find_with_issuers(core::iter::empty(), IssuerPath::CodeSigning, authorize)
    }

    fn find_with_issuers<'a, T>(
        &'a self,
        external: impl Iterator<Item = &'a Candidate<'a>>,
        path: IssuerPath,
        mut authorize: impl FnMut(&Candidate<'_>, usize) -> Option<T>,
    ) -> Option<T> {
        // A certificate's shortest path minimizes the number of non-self-issued
        // intermediate CAs. Longer paths cannot satisfy a stricter pathLen.
        let certificates = self.certificates.iter().chain(external).collect::<Vec<_>>();
        let mut path_len = vec![usize::MAX; certificates.len()];
        let mut visited = vec![false; certificates.len()];
        path_len[self.signer] = 0;
        while let Some(index) = (0..certificates.len())
            .filter(|&index| !visited[index] && path_len[index] != usize::MAX)
            .min_by_key(|&index| path_len[index])
        {
            visited[index] = true;
            let child = certificates[index];
            let intermediates =
                path_len[index] + usize::from(index != self.signer && !child.is_self_issued());
            if let Some(authority) = authorize(child, intermediates) {
                return Some(authority);
            }
            for (parent_index, parent) in certificates.iter().enumerate() {
                if !visited[parent_index]
                    && intermediates < path_len[parent_index]
                    && match path {
                        IssuerPath::CodeSigning => parent.can_issue(child, intermediates),
                        IssuerPath::Revocation => child.is_signed_by(parent),
                    }
                {
                    path_len[parent_index] = intermediates;
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authvar::Descriptor;

    const UPDATE: &[u8] = include_bytes!(
        "../../../tests/fixtures/firmware/certs/dbx-update-x64-v1.7.2-signed.efiauth2"
    );
    const KEK_2023: &[u8] =
        include_bytes!("../../../tests/fixtures/firmware/certs/microsoft-kek-ca-2023.der");
    const KEK_2011: &[u8] =
        include_bytes!("../../../tests/fixtures/firmware/certs/microsoft-kek-ca-2011.der");
    pub(super) const TEST_AUTHENTICODE: &[u8] =
        include_bytes!("../../../tests/fixtures/firmware/certs/authenticode-test-hello-sha256.p7b");
    pub(super) const TEST_IMAGE_CA: &[u8] =
        include_bytes!("../../../tests/fixtures/firmware/certs/authenticode-test-ca.der");
    pub(super) const TEST_IMAGE_DIGEST: [u8; 32] = [
        0xca, 0x85, 0x1c, 0x2a, 0xcc, 0x75, 0x3a, 0xd7, 0x77, 0x2f, 0x5a, 0x36, 0x6f, 0xaa, 0xac,
        0xad, 0xf7, 0x39, 0x9a, 0x1e, 0xa7, 0x36, 0x4a, 0xc4, 0x31, 0x1a, 0x56, 0x25, 0xf4, 0x87,
        0xda, 0x39,
    ];
    // SHA-256 of the byte string verified independently by OpenSSL CMS.
    const DIGEST: [u8; 32] = [
        0x90, 0x8f, 0x44, 0x81, 0x06, 0x2a, 0xb2, 0xee, 0x42, 0x0f, 0x2d, 0xa7, 0x90, 0xb7, 0x3e,
        0xaa, 0x75, 0xfc, 0xac, 0x58, 0x96, 0x54, 0x02, 0x91, 0xc7, 0xdd, 0xc0, 0x5d, 0x54, 0x2d,
        0x66, 0x03,
    ];

    #[test]
    fn signer_serial_compares_positive_magnitudes_without_der_sign_padding() {
        assert!(same_signer_serial(&[0, 0x80, 0x32], &[0x80, 0x32]));
        assert!(same_signer_serial(&[0x7f, 0x32], &[0x7f, 0x32]));
        assert!(!same_signer_serial(&[0, 0x80, 0x32], &[0x80, 0x33]));
        assert!(!same_signer_serial(&[0x80, 0x32], &[0x80, 0x32]));
    }

    #[test]
    fn verifies_authenticode_with_enrolled_leaf_or_external_ca() {
        let signed = AuthenticodeSignedData::decode(TEST_AUTHENTICODE).unwrap();
        let leaf = signed.certificates().next().unwrap();
        assert_eq!(signed.certificates().count(), 1);
        assert_ne!(leaf, TEST_IMAGE_CA);
        assert_eq!(
            authenticode_signature(&signed, &TEST_IMAGE_DIGEST, &[leaf]),
            Ok(0)
        );
        assert_eq!(
            authenticode_signature(&signed, &TEST_IMAGE_DIGEST, &[KEK_2011, TEST_IMAGE_CA]),
            Ok(1)
        );
        assert_eq!(
            authenticode_signature(&signed, &TEST_IMAGE_DIGEST, &[KEK_2011]),
            Err(Error::UntrustedSigner)
        );
        let mut wrong_image = TEST_IMAGE_DIGEST;
        wrong_image[0] ^= 1;
        assert_eq!(
            authenticode_signature(&signed, &wrong_image, &[TEST_IMAGE_CA]),
            Err(Error::InvalidSignature)
        );
        let mut corrupt = TEST_AUTHENTICODE.to_vec();
        let signature = signed.signer().signature;
        let offset = corrupt
            .windows(signature.len())
            .position(|bytes| bytes == signature)
            .unwrap();
        corrupt[offset] ^= 1;
        let signed = AuthenticodeSignedData::decode(&corrupt).unwrap();
        assert_eq!(
            authenticode_signature(&signed, &TEST_IMAGE_DIGEST, &[TEST_IMAGE_CA]),
            Err(Error::InvalidSignature)
        );
    }

    #[test]
    fn verifies_microsoft_update_against_enrolled_2023_kek() {
        let descriptor = Descriptor::decode(UPDATE, true).unwrap();
        let parsed = DetachedSignedData::decode(descriptor.signed_data).unwrap();
        assert_eq!(
            detached(&parsed, &DIGEST, &[TrustAnchor::ExchangeKey(KEK_2023)]),
            Ok(())
        );
        assert_eq!(
            detached(&parsed, &DIGEST, &[TrustAnchor::ExchangeKey(KEK_2011)]),
            Err(Error::UntrustedSigner)
        );
        assert_eq!(detached(&parsed, &DIGEST, &[]), Err(Error::UntrustedSigner));
        // The same chain does not authorize changes that require a direct PK signature.
        assert_eq!(
            detached(&parsed, &DIGEST, &[TrustAnchor::PlatformKey(KEK_2023)]),
            Err(Error::UntrustedSigner)
        );
        let leaf = parsed
            .certificates()
            .find(|cert| *cert != KEK_2023)
            .unwrap();
        assert_eq!(
            detached(&parsed, &DIGEST, &[TrustAnchor::PlatformKey(leaf)]),
            Ok(())
        );
    }

    #[test]
    fn rejects_corrupted_signed_data_and_signature() {
        let descriptor = Descriptor::decode(UPDATE, true).unwrap();
        let parsed = DetachedSignedData::decode(descriptor.signed_data).unwrap();
        let anchor = [TrustAnchor::ExchangeKey(KEK_2023)];
        let mut wrong_digest = DIGEST;
        wrong_digest[0] ^= 1;
        assert_eq!(
            detached(&parsed, &wrong_digest, &anchor),
            Err(Error::InvalidSignature)
        );

        let mut bad_signature = descriptor.signed_data.to_vec();
        assert!(bad_signature.ends_with(parsed.signer.signature));
        *bad_signature.last_mut().unwrap() ^= 1;
        let parsed = DetachedSignedData::decode(&bad_signature).unwrap();
        assert_eq!(
            detached(&parsed, &DIGEST, &anchor),
            Err(Error::InvalidSignature)
        );
    }

    #[test]
    fn rejects_a_valid_detached_signature_under_a_corrupted_certificate_chain() {
        let descriptor = Descriptor::decode(UPDATE, true).unwrap();
        let parsed = DetachedSignedData::decode(descriptor.signed_data).unwrap();
        let leaf = parsed
            .certificates()
            .find(|cert| *cert != KEK_2023)
            .unwrap();
        let mut bad_chain = descriptor.signed_data.to_vec();
        let position = bad_chain
            .windows(leaf.len())
            .position(|window| window == leaf)
            .unwrap();
        bad_chain[position + leaf.len() - 1] ^= 1;
        let parsed = DetachedSignedData::decode(&bad_chain).unwrap();
        assert_eq!(
            detached(&parsed, &DIGEST, &[TrustAnchor::ExchangeKey(KEK_2023)]),
            Err(Error::UntrustedSigner)
        );
    }
}
