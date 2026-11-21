//! ML-DSA-65 primitive (FIPS 204) for the object type `TPM_ALG_MLDSA` (TPM Library v185, Part 2
//! clauses 11.2.7, 11.3.4 and 12.2.3.6).
//!
//! The private value of an object is the 32-octet seed xi of `ML-DSA.KeyGen_internal` (Part 2,
//! Table 211); the expanded key is derived from it. Signatures are the deterministic variant of
//! FIPS 204 (`rnd` of 32 zero octets, Algorithm 2), like the RFC 6979 nonces of ECDSA, so the same
//! key and message always give the same signature. Messages are signed with a context of up to
//! 255 octets (the context of `TPM2_SignSequenceStart`); `TPM2_SignDigest` signs an external mu
//! (64 octets, Algorithm 7, line 6).

use ml_dsa::common::array::Array;
use ml_dsa::common::typenum::U64;
use ml_dsa::{EncodedVerifyingKey, Keypair, MlDsa65, Seed, Signature, SigningKey, VerifyingKey};
use zeroize::Zeroizing;

use crate::random::Drbg;
use crate::response::Rc;

/// The public key of ML-DSA-65 (`pkEncode`, FIPS 204 Table 2).
pub(crate) const PUBLIC_BYTES: usize = 1952;
/// The size of the message representative mu.
pub(crate) const MU_BYTES: usize = 64;
/// The longest context string of FIPS 204 (a single octet carries its length).
pub(crate) const MAX_CONTEXT_BYTES: usize = 255;

pub(crate) type MlDsaPrivate = SigningKey<MlDsa65>;

pub(crate) fn generated_key(drbg: &mut Drbg) -> Result<MlDsaPrivate, Rc> {
    let mut seed = Zeroizing::new(Seed::default());
    drbg.generate(seed.as_mut_slice()).ok_or(Rc::FAILURE)?;
    Ok(MlDsaPrivate::from_seed(&seed))
}

/// `TPM_RC_SIZE` for a seed that is not 32 octets.
pub(crate) fn private_from_seed(bytes: &[u8]) -> Result<MlDsaPrivate, Rc> {
    let seed = Zeroizing::new(Seed::try_from(bytes).map_err(|_| Rc::SIZE)?);
    Ok(MlDsaPrivate::from_seed(&seed))
}

pub(crate) fn private_seed(key: &MlDsaPrivate) -> Zeroizing<Vec<u8>> {
    let seed = Zeroizing::new(key.to_seed());
    Zeroizing::new(seed.as_slice().to_vec())
}

pub(crate) fn public_bytes(key: &MlDsaPrivate) -> Vec<u8> {
    key.verifying_key().encode().as_slice().to_vec()
}

/// Every string of 1952 octets is a public key: FIPS 204 `pkDecode` checks nothing else.
pub(crate) fn public_encoding_is_valid(bytes: &[u8]) -> bool {
    bytes.len() == PUBLIC_BYTES
}

fn mu_of(mu: &[u8]) -> Option<Array<u8, U64>> {
    Array::<u8, U64>::try_from(mu).ok()
}

/// The signature of `message` under `context` (`None` for a context longer than 255 octets).
pub(crate) fn sign_message(key: &MlDsaPrivate, message: &[u8], context: &[u8]) -> Option<Vec<u8>> {
    let signature = key
        .expanded_key()
        .sign_deterministic(message, context)
        .ok()?;
    Some(signature.encode().as_slice().to_vec())
}

/// The signature of an external mu (`None` unless `mu` has 64 octets). The crate panics only if its
/// rejection-sampling loop finds no signature, which FIPS 204 bounds by a negligible probability.
pub(crate) fn sign_mu(key: &MlDsaPrivate, mu: &[u8]) -> Option<Vec<u8>> {
    let signature = key.expanded_key().sign_mu_deterministic(&mu_of(mu)?);
    Some(signature.encode().as_slice().to_vec())
}

fn verifying_key(public: &[u8]) -> Option<VerifyingKey<MlDsa65>> {
    let encoded = EncodedVerifyingKey::<MlDsa65>::try_from(public).ok()?;
    Some(VerifyingKey::decode(&encoded))
}

/// `ML-DSA.Verify` of `signature` over `message` and `context`; false for a signature of another
/// length, one that does not decode (`sigDecode` rejects a malformed hint) and a wrong one.
pub(crate) fn verify_message(
    public: &[u8],
    message: &[u8],
    context: &[u8],
    signature: &[u8],
) -> bool {
    let (Some(key), Ok(signature)) = (
        verifying_key(public),
        Signature::<MlDsa65>::try_from(signature),
    ) else {
        return false;
    };
    key.verify_with_context(message, context, &signature)
}

/// `ML-DSA.Verify` of `signature` over an external mu of 64 octets.
pub(crate) fn verify_mu(public: &[u8], mu: &[u8], signature: &[u8]) -> bool {
    let (Some(key), Some(mu), Ok(signature)) = (
        verifying_key(public),
        mu_of(mu),
        Signature::<MlDsa65>::try_from(signature),
    ) else {
        return false;
    };
    key.verify_mu(&mu, &signature)
}
