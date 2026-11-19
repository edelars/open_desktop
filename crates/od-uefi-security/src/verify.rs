//! Verification of detached Auth2 signatures with the enrolled platform and exchange keys.

use alloc::vec::Vec;

use der::{Decode, Encode, asn1::ObjectIdentifier, oid::AssociatedOid};
use rsa::{Pkcs1v15Sign, PublicKey, pkcs8::DecodePublicKey, traits::PublicKeyParts};
use sha2::{Digest, Sha256};
use x509_cert::{
    Certificate,
    ext::pkix::{BasicConstraints, ExtendedKeyUsage, KeyUsage},
};

use crate::pkcs7::{
    DetachedSignedData, MAX_CERTIFICATES, RSA_2048_SIGNATURE_BYTES, raw_sequence, sequence,
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
    issuer_max_path: Option<u8>,
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
            .map(|(_, constraints)| constraints.path_len_constraint.unwrap_or(u8::MAX));

        let key = public_key(&certificate)?;
        let tbs_der = sequence(der)
            .and_then(|mut fields| raw_sequence(&mut fields))
            .map_err(|_| Error::InvalidCertificate)?;
        Ok(Self {
            der,
            tbs_hash: Sha256::digest(tbs_der).into(),
            certificate,
            key,
            digital_signature: code_signing && usage.is_none_or(|usage| usage.digital_signature()),
            issuer_max_path,
        })
    }

    fn is_signed_by(&self, issuer: &Self) -> bool {
        let algorithm = self.certificate.signature_algorithm();
        let tbs = self.certificate.tbs_certificate();
        algorithm.oid == SHA256_WITH_RSA
            && algorithm
                .parameters
                .as_ref()
                .is_none_or(|param| param.is_null())
            && tbs.signature() == algorithm
            && tbs.issuer() == issuer.certificate.tbs_certificate().subject()
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

    fn is_self_issued(&self) -> bool {
        let tbs = self.certificate.tbs_certificate();
        tbs.issuer() == tbs.subject()
    }
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
    let candidates = signed
        .certificates()
        .map(Candidate::parse)
        .collect::<Result<Vec<_>, _>>()?;
    let mut signer = None;
    for (index, candidate) in candidates.iter().enumerate() {
        let tbs = candidate.certificate.tbs_certificate();
        if tbs.serial_number().as_bytes() == signed.signer.serial
            && tbs
                .issuer()
                .to_der()
                .map_err(|_| Error::InvalidCertificate)?
                == signed.signer.issuer_der
        {
            if signer.replace(index).is_some() {
                return Err(Error::AmbiguousSigner);
            }
        }
    }
    let signer = signer.ok_or(Error::UntrustedSigner)?;
    let leaf = &candidates[signer];
    if !leaf.digital_signature
        || leaf
            .key
            .verify(
                Pkcs1v15Sign::new::<Sha256>(),
                digest,
                signed.signer.signature,
            )
            .is_err()
    {
        return Err(Error::InvalidSignature);
    }

    for anchor in anchors {
        if let TrustAnchor::PlatformKey(bytes) = anchor {
            let certificate =
                Certificate::from_der(bytes).map_err(|_| Error::InvalidCertificate)?;
            let key = public_key(&certificate)?;
            if leaf.key.n() == key.n() && leaf.key.e() == key.e() {
                return Ok(());
            }
        }
    }

    // Visit each reachable certificate once, preferring the path with the fewest
    // non-self-issued intermediate CAs. This bounds work even if names collide.
    let mut path_len = [u8::MAX; MAX_CERTIFICATES];
    let mut visited = [false; MAX_CERTIFICATES];
    path_len[signer] = 0;
    while let Some(index) = (0..candidates.len())
        .filter(|&index| !visited[index] && path_len[index] != u8::MAX)
        .min_by_key(|&index| path_len[index])
    {
        visited[index] = true;
        let child = &candidates[index];
        if anchors
            .iter()
            .any(|anchor| matches!(anchor, TrustAnchor::ExchangeKey(bytes) if *bytes == child.der))
        {
            return Ok(());
        }
        let intermediates = path_len[index] + u8::from(index != signer && !child.is_self_issued());
        for (parent_index, parent) in candidates.iter().enumerate() {
            if !visited[parent_index]
                && intermediates < path_len[parent_index]
                && parent
                    .issuer_max_path
                    .is_some_and(|limit| intermediates <= limit)
                && child.is_signed_by(parent)
            {
                path_len[parent_index] = intermediates;
            }
        }
    }
    Err(Error::UntrustedSigner)
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
    // SHA-256 of the byte string verified independently by OpenSSL CMS.
    const DIGEST: [u8; 32] = [
        0x90, 0x8f, 0x44, 0x81, 0x06, 0x2a, 0xb2, 0xee, 0x42, 0x0f, 0x2d, 0xa7, 0x90, 0xb7, 0x3e,
        0xaa, 0x75, 0xfc, 0xac, 0x58, 0x96, 0x54, 0x02, 0x91, 0xc7, 0xdd, 0xc0, 0x5d, 0x54, 0x2d,
        0x66, 0x03,
    ];

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
