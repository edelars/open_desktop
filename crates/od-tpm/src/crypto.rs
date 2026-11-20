//! Primitives of the vTPM over the RustCrypto crates `hmac`, `sha2`, `rsa` and `p256` (ADR-0009,
//! decision 2): HMAC-SHA-256, a constant-time HMAC check, KDFa, and (slice B of phase B) RSA-2048
//! PKCS#1 v1.5 and ECC P-256 ECDSA key generation, signing and verification.
//!
//! The protocol-level constructions (KDFa, session HMACs, the DRBG that uses them) stay in this
//! crate; only the primitives come from the libraries. No function here logs or prints a key.
//!
//! RSA and the timing side channel (ISSUE-064, RUSTSEC-2023-0071): private RSA operations use
//! `rsa =0.10.0`, selected after RustSec marked `>=0.10.0` patched and the upstream release
//! documented constant-time modular exponentiation and further private-operation hardening. The
//! TPM DRBG is supplied for blinding. The migration's deterministic keys, signatures, ciphertext
//! compatibility, and tests pass; this project does not certify the dependency's complete
//! constant-time behavior; independent review approved the migration on 2026-11-18, resolving
//! ISSUE-064 as the selected-version migration requirement. The exposure is a timing attack on the host process that runs the vTPM, not on a
//! firmware stack. ECDSA runs on `p256` (`crypto-bigint` arithmetic); its constant-time property is
//! the vendor's claim, not checked here (ADR-0009).

use hmac::{Hmac, KeyInit, Mac};
use p256::ecdsa::signature::hazmat::{PrehashSigner, PrehashVerifier};
use rsa::rand_core::{TryCryptoRng, TryRng};
use rsa::traits::{PrivateKeyParts, PublicKeyParts};
use rsa::{BoxedUint, Oaep, Pkcs1v15Encrypt, Pkcs1v15Sign};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::random::Drbg;

/// Digest size of SHA-256, the only session and DRBG hash.
pub(crate) const DIGEST: usize = 32;

/// Block size of SHA-256 in octets (the HMAC key block).
const BLOCK: usize = 64;

pub(crate) fn hmac_sha256_state(key: &[u8]) -> Hmac<Sha256> {
    // RFC 2104 key preparation done here so that the infallible fixed-size `KeyInit::new` is used
    // instead of the fallible `new_from_slice`: a key longer than the 64-octet block is replaced
    // by its SHA-256 digest, then the key is zero-padded to the block size. The result is the same
    // as for `new_from_slice`, which applies exactly this rule for every key length.
    let mut block = Zeroizing::new([0u8; BLOCK]);
    if key.len() > BLOCK {
        block[..DIGEST].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    <Hmac<Sha256> as KeyInit>::new(&(*block).into())
}

fn mac(key: &[u8], parts: &[&[u8]]) -> Hmac<Sha256> {
    let mut m = hmac_sha256_state(key);
    for p in parts {
        m.update(p);
    }
    m
}

/// `HMAC-SHA-256(key, parts[0] || parts[1] || ...)`; `key` may have any length.
pub(crate) fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; DIGEST] {
    let mut out = [0u8; DIGEST];
    out.copy_from_slice(&mac(key, parts).finalize().into_bytes());
    out
}

/// Checks `tag` against `HMAC-SHA-256(key, parts)` in constant time (`Mac::verify_slice`); a tag
/// of the wrong length is a mismatch.
pub(crate) fn hmac_sha256_verify(key: &[u8], parts: &[&[u8]], tag: &[u8]) -> bool {
    mac(key, parts).verify_slice(tag).is_ok()
}

/// KDFa (Part 1 v184, clause 9.4.10.2): SP 800-108 counter mode with HMAC-SHA-256,
/// `K(i) = HMAC(key, [i]_4 || label || 00 || contextU || contextV || [bits]_4)`, `i` from 1; the
/// result is `(bits + 7) / 8` octets, the unused high bits of the first octet cleared when `bits`
/// is not a multiple of 8. The label is given without its terminating zero octet.
pub(crate) fn kdfa_sha256(
    key: &[u8],
    label: &[u8],
    context_u: &[u8],
    context_v: &[u8],
    bits: u32,
) -> Zeroizing<Vec<u8>> {
    let bytes = (bits as usize).div_ceil(8);
    let mut out = Zeroizing::new(Vec::with_capacity(bytes + DIGEST));
    let mut counter: u32 = 1;
    while out.len() < bytes {
        let block = hmac_sha256(
            key,
            &[
                &counter.to_be_bytes(),
                label,
                &[0u8],
                context_u,
                context_v,
                &bits.to_be_bytes(),
            ],
        );
        out.extend_from_slice(&block);
        counter += 1;
    }
    out.truncate(bytes);
    if !bits.is_multiple_of(8)
        && let Some(first) = out.first_mut()
    {
        *first &= 0xFFu8 >> (8 - bits % 8);
    }
    out
}

// ---------------------------------------------------------------------------- asymmetric keys

/// The TPM DRBG seen as a `rand_core` generator, for key generation and RSA blinding. A reseed
/// request of the DRBG (never reached in practice) is an error of the generator.
pub(crate) struct DrbgRng<'a>(pub &'a mut Drbg);

/// The DRBG needs a reseed.
#[derive(Debug)]
pub(crate) struct DrbgExhausted;

impl core::fmt::Display for DrbgExhausted {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("DRBG reseed required")
    }
}

impl core::error::Error for DrbgExhausted {}

impl TryRng for DrbgRng<'_> {
    type Error = DrbgExhausted;

    fn try_next_u32(&mut self) -> Result<u32, DrbgExhausted> {
        let mut b = [0u8; 4];
        self.try_fill_bytes(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }

    fn try_next_u64(&mut self) -> Result<u64, DrbgExhausted> {
        let mut b = [0u8; 8];
        self.try_fill_bytes(&mut b)?;
        Ok(u64::from_le_bytes(b))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), DrbgExhausted> {
        self.0.generate(dst).ok_or(DrbgExhausted)
    }
}

impl TryCryptoRng for DrbgRng<'_> {}

/// An RSA-2048 private key (two primes, exponent 65537).
pub(crate) type RsaPrivate = rsa::PrivateKey;
/// An ECC P-256 private key.
pub(crate) type EccPrivate = p256::ecdsa::SigningKey;

/// Bytes of an RSA-2048 modulus.
const RSA_BYTES: usize = 256;
/// Bytes of one RSA prime.
const PRIME_BYTES: usize = RSA_BYTES / 2;
/// Public exponent of RSA keys supported by the virtual TPM.
pub(crate) const RSA_EXPONENT: u32 = 65_537;

pub(crate) fn supported_rsa_exponent(encoded: u32) -> bool {
    matches!(encoded, 0 | RSA_EXPONENT)
}

/// Generates an RSA-2048 key (public exponent 65537) from `rng`. The result is a function of the
/// bytes the generator yields, so a generator seeded identically gives the same key (primary
/// objects); the prime search is that of `rsa`/`crypto-primes` at the pinned versions.
pub(crate) fn rsa_generate(rng: &mut Drbg) -> Option<RsaPrivate> {
    RsaPrivate::new(&mut DrbgRng(rng), 2048).ok()
}

/// `n` as 256 big-endian octets.
pub(crate) fn rsa_modulus(key: &RsaPrivate) -> Vec<u8> {
    key.n_bytes().to_vec()
}

/// The two primes of the key, `p || q`, 128 octets each; wiped on drop.
pub(crate) fn rsa_primes(key: &RsaPrivate) -> Option<Zeroizing<Vec<u8>>> {
    let primes = key.primes();
    if primes.len() != 2 {
        return None;
    }
    let mut out = Zeroizing::new(Vec::with_capacity(RSA_BYTES));
    for p in primes {
        let b = p.to_be_bytes();
        // Left-pad to the fixed width (both primes have 1024 bits, so no padding occurs).
        if b.len() > PRIME_BYTES {
            return None;
        }
        let pad = out.len() + PRIME_BYTES - b.len();
        out.resize(pad, 0);
        out.extend_from_slice(&b);
    }
    Some(out)
}

/// Rebuilds the key for the profile's fixed exponent from `p || q` and checks the public
/// modulus `n`. The caller must validate the encoded public exponent separately.
pub(crate) fn rsa_from_primes(pq: &[u8], n: &[u8]) -> Option<RsaPrivate> {
    if pq.len() != RSA_BYTES || n.len() != RSA_BYTES {
        return None;
    }
    let p = BoxedUint::from_be_slice(&pq[..PRIME_BYTES], 1024).ok()?;
    let q = BoxedUint::from_be_slice(&pq[PRIME_BYTES..], 1024).ok()?;
    let key = RsaPrivate::from_p_q(p, q, BoxedUint::from(u64::from(RSA_EXPONENT))).ok()?;
    (key.n_bytes().as_ref() == n).then_some(key)
}

/// RSASSA-PKCS1-v1_5 with SHA-256 over `digest` (already a SHA-256 digest). Private operation:
/// see the ISSUE-064 note at the top of this module.
pub(crate) fn rsa_sign(key: &RsaPrivate, digest: &[u8], rng: &mut Drbg) -> Option<Vec<u8>> {
    key.pkcs1v15_sign_prehash_with_rng::<Sha256, _>(&mut DrbgRng(rng), digest)
        .ok()
}

/// The RSA public key `(n, 65537)`; `None` for a modulus the library rejects.
fn rsa_public_key(n: &[u8]) -> Option<rsa::PublicKey> {
    let n = BoxedUint::from_be_slice(n, (n.len() * 8) as u32).ok()?;
    rsa::PublicKey::new(n, BoxedUint::from(u64::from(RSA_EXPONENT))).ok()
}

/// Verifies an RSASSA-PKCS1-v1_5 SHA-256 signature with the public key `(n, 65537)`.
pub(crate) fn rsa_verify(n: &[u8], digest: &[u8], sig: &[u8]) -> bool {
    let Some(key) = rsa_public_key(n) else {
        return false;
    };
    key.verify(Pkcs1v15Sign::new::<Sha256>(), digest, sig)
        .is_ok()
}

/// The padding of `TPM2_RSA_Encrypt` and `TPM2_RSA_Decrypt` (Part 3, Table 42): none (the
/// message is an integer, raw RSA), RSAES-PKCS1-v1_5 and RSAES-OAEP (SHA-256 for the label and for
/// MGF1, the only mask generation the text allows).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RsaPadding {
    None,
    Rsaes,
    Oaep,
}

/// `TPM2_RSA_Encrypt` primitive with the public key `(n, 65537)`: `None` when the message does not
/// fit the padding or the integer is not below `n` (the caller answers `TPM_RC_VALUE`). `label` is
/// the OAEP label including its terminating zero octet (empty: no label). The RNG is the TPM DRBG.
pub(crate) fn rsa_encrypt_public(
    n: &[u8],
    padding: RsaPadding,
    label: &[u8],
    message: &[u8],
    rng: &mut Drbg,
) -> Option<Vec<u8>> {
    let key = rsa_public_key(n)?;
    match padding {
        RsaPadding::None => {
            let m = BoxedUint::from_be_slice(message, (n.len() * 8) as u32).ok()?;
            if m.cmp(key.n().as_ref()).is_ge() {
                return None;
            }
            let c = rsa::hazmat::rsa_encrypt(&key, &m).ok()?;
            Some(c.to_be_bytes().to_vec())
        }
        RsaPadding::Rsaes => key
            .encrypt(&mut DrbgRng(rng), Pkcs1v15Encrypt, message)
            .ok(),
        RsaPadding::Oaep => {
            let scheme = if label.is_empty() {
                Oaep::<Sha256>::new()
            } else {
                Oaep::<Sha256>::new_with_label(label)
            };
            key.encrypt(&mut DrbgRng(rng), scheme, message).ok()
        }
    }
}

/// `TPM2_RSA_Decrypt` primitive (private operation; see the ISSUE-064 note at the top of this
/// module). `None` when the padding does not check or the integer is not below `n` (the caller
/// answers `TPM_RC_VALUE`). RSAES uses the implicit rejection of the `rsa` crate: an invalid
/// padding yields a deterministic synthetic message, not an error (recorded deviation, see
/// `docs/specs/VTPM.md`). The result is wiped on drop.
pub(crate) fn rsa_decrypt_private(
    key: &RsaPrivate,
    padding: RsaPadding,
    label: &[u8],
    ciphertext: &[u8],
    rng: &mut Drbg,
) -> Option<Zeroizing<Vec<u8>>> {
    let out = match padding {
        RsaPadding::None => {
            let c = BoxedUint::from_be_slice(ciphertext, RSA_BYTES as u32 * 8).ok()?;
            let m = rsa::hazmat::rsa_decrypt_and_check(key, Some(&mut DrbgRng(rng)), &c).ok()?;
            Zeroizing::new(Zeroizing::new(m).to_be_bytes().to_vec())
        }
        RsaPadding::Rsaes => Zeroizing::new(
            key.decrypt_blinded(&mut DrbgRng(rng), Pkcs1v15Encrypt, ciphertext)
                .ok()?,
        ),
        RsaPadding::Oaep => {
            let scheme = if label.is_empty() {
                Oaep::<Sha256>::new()
            } else {
                Oaep::<Sha256>::new_with_label(label)
            };
            Zeroizing::new(
                key.decrypt_blinded(&mut DrbgRng(rng), scheme, ciphertext)
                    .ok()?,
            )
        }
    };
    Some(out)
}

/// A P-256 private key from `rng`: 32 octets per attempt until the value is in `[1, n - 1]`.
pub(crate) fn ecc_generate(rng: &mut Drbg) -> Option<EccPrivate> {
    for _ in 0..64 {
        let mut b = Zeroizing::new([0u8; 32]);
        rng.generate(&mut b[..])?;
        if let Ok(k) = EccPrivate::from_slice(&b[..]) {
            return Some(k);
        }
    }
    None
}

/// The private scalar, 32 octets; wiped on drop.
pub(crate) fn ecc_scalar(key: &EccPrivate) -> Zeroizing<Vec<u8>> {
    Zeroizing::new(key.to_bytes().to_vec())
}

/// The private key from its scalar (`None` if zero or not below the group order).
pub(crate) fn ecc_from_scalar(d: &[u8]) -> Option<EccPrivate> {
    if d.len() != 32 {
        return None;
    }
    EccPrivate::from_slice(d).ok()
}

/// Affine coordinates `(x, y)` of the public point, 32 octets each.
pub(crate) fn ecc_public(key: &EccPrivate) -> Option<(Vec<u8>, Vec<u8>)> {
    let point = key.verifying_key().to_sec1_point(false);
    let b = point.as_bytes();
    (b.len() == 65 && b[0] == 4).then(|| (b[1..33].to_vec(), b[33..].to_vec()))
}

/// `true` if `(x, y)` is a point of the curve (both coordinates 32 octets).
pub(crate) fn ecc_point_valid(x: &[u8], y: &[u8]) -> bool {
    ecc_verifying_key(x, y).is_some()
}

fn ecc_verifying_key(x: &[u8], y: &[u8]) -> Option<p256::ecdsa::VerifyingKey> {
    if x.len() != 32 || y.len() != 32 {
        return None;
    }
    let mut sec1 = Vec::with_capacity(65);
    sec1.push(4);
    sec1.extend_from_slice(x);
    sec1.extend_from_slice(y);
    p256::ecdsa::VerifyingKey::from_sec1_bytes(&sec1).ok()
}

/// The affine coordinates `(x, y)` of a secret point, wiped on drop.
pub(crate) type SecretPoint = (Zeroizing<Vec<u8>>, Zeroizing<Vec<u8>>);

/// ECDH over P-256 (`TPM2_ECDH_ZGen`, Part 3, clause 14.5): `Z = [h * d] Q` with the cofactor
/// `h = 1`, as the 32-octet affine coordinates `(x, y)`; the input coordinates may be shorter than
/// 32 octets (padded on the left); `None` if `Q = (x, y)` is not a point of
/// the curve or the product is the point at infinity (not possible for a point of the prime-order
/// group and a scalar in `[1, n - 1]`). Both coordinates are wiped on drop.
pub(crate) fn ecc_ecdh(key: &EccPrivate, x: &[u8], y: &[u8]) -> Option<SecretPoint> {
    use p256::elliptic_curve::sec1::ToSec1Point;
    // A TPM2B coordinate may come without its leading zero octets: pad on the left to 32 octets.
    let (x, y) = (pad32(x)?, pad32(y)?);
    let mut sec1 = [0u8; 65];
    sec1[0] = 4;
    sec1[1..33].copy_from_slice(&x);
    sec1[33..].copy_from_slice(&y);
    let q = p256::PublicKey::from_sec1_bytes(&sec1).ok()?;
    let scalar = Zeroizing::new(key.to_bytes());
    let d = p256::SecretKey::from_bytes(&scalar).ok()?;
    let z = (q.to_projective() * *d.to_nonzero_scalar()).to_affine();
    // The encoded point Z is secret: it is wiped after its coordinates are copied out.
    let point = Zeroizing::new(z.to_sec1_point(false).as_bytes().to_vec());
    (point.len() == 65 && point[0] == 4).then(|| {
        (
            Zeroizing::new(point[1..33].to_vec()),
            Zeroizing::new(point[33..].to_vec()),
        )
    })
}

/// ECDSA over P-256 of a SHA-256 `digest`: `(r, s)`, 32 octets each (RFC 6979 nonce).
pub(crate) fn ecc_sign(key: &EccPrivate, digest: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let sig: p256::ecdsa::Signature = key.sign_prehash(digest).ok()?;
    let (r, s) = sig.split_bytes();
    Some((r.to_vec(), s.to_vec()))
}

/// Verifies an ECDSA P-256 signature `(r, s)` of `digest` with the public point `(x, y)`.
/// `r` and `s` are given as TPM2B values (leading zeros may be stripped).
pub(crate) fn ecc_verify(x: &[u8], y: &[u8], digest: &[u8], r: &[u8], s: &[u8]) -> bool {
    let Some(key) = ecc_verifying_key(x, y) else {
        return false;
    };
    let (Some(r), Some(s)) = (pad32(r), pad32(s)) else {
        return false;
    };
    let mut rs = [0u8; 64];
    rs[..32].copy_from_slice(&r);
    rs[32..].copy_from_slice(&s);
    let Ok(sig) = p256::ecdsa::Signature::from_slice(&rs) else {
        return false;
    };
    key.verify_prehash(digest, &sig).is_ok()
}

/// Left-pads a big-endian value of at most 32 octets to 32.
fn pad32(v: &[u8]) -> Option<[u8; 32]> {
    if v.len() > 32 {
        return None;
    }
    let mut out = [0u8; 32];
    out[32 - v.len()..].copy_from_slice(v);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn hmac_matches_rfc_4231_cases_with_keys_of_any_length() {
        // Case 1: key 0x0b x 20.
        assert_eq!(
            hmac_sha256(&[0x0b; 20], &[b"Hi There"]).to_vec(),
            hex("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7")
        );
        // Case 2: key "Jefe".
        assert_eq!(
            hmac_sha256(b"Jefe", &[b"what do ya want ", b"for nothing?"]).to_vec(),
            hex("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")
        );
        // Case 6: a 131-octet key (longer than the block, hashed first), the data in two parts.
        assert_eq!(
            hmac_sha256(
                &[0xaa; 131],
                &[
                    b"Test Using Larger Than Block-Size Key - ",
                    b"Hash Key First"
                ]
            )
            .to_vec(),
            hex("60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54")
        );
        // The empty key is valid and differs from the all-zero 32-octet key only through padding:
        // HMAC pads with zeros, so both give the same tag.
        assert_eq!(hmac_sha256(&[], &[b"x"]), hmac_sha256(&[0u8; 32], &[b"x"]));
    }

    #[test]
    fn hmac_verify_accepts_only_the_exact_tag() {
        let tag = hmac_sha256(b"k", &[b"data"]);
        assert!(hmac_sha256_verify(b"k", &[b"data"], &tag));
        assert!(!hmac_sha256_verify(b"k", &[b"datb"], &tag));
        assert!(!hmac_sha256_verify(b"j", &[b"data"], &tag));
        assert!(!hmac_sha256_verify(b"k", &[b"data"], &tag[..31]));
        assert!(!hmac_sha256_verify(b"k", &[b"data"], &[]));
        let mut bad = tag;
        bad[31] ^= 1;
        assert!(!hmac_sha256_verify(b"k", &[b"data"], &bad));
    }

    #[test]
    fn kdfa_matches_an_independent_vector() {
        // Computed independently with Python `hmac`/`hashlib` (SP 800-108 counter mode as in
        // Part 1, clause 9.4.10.2): key = 32 x 0x01, label "ATH", contextU = 16 x 0x02,
        // contextV = 16 x 0x03, with 256 bits, 600 bits (three blocks) and 12 bits (masked first octet).
        let key = [1u8; 32];
        let u = [2u8; 16];
        let v = [3u8; 16];
        assert_eq!(
            &kdfa_sha256(&key, b"ATH", &u, &v, 256)[..],
            &hex(KDFA_256)[..]
        );
        let long = kdfa_sha256(&key, b"ATH", &u, &v, 600);
        assert_eq!(&long[..], &hex(KDFA_600)[..]);
        assert_eq!(long.len(), 75);
        // 12 bits: two octets, the upper nibble of the first one cleared (clause 9.4.10.2).
        let short = kdfa_sha256(&key, b"ATH", &u, &v, 12);
        assert_eq!(&short[..], &hex("04c0")[..]);
    }

    // Computed independently (Python hmac/hashlib), 2026-11-07; see the test above.
    const KDFA_256: &str = "32316f0b89c58eaa1e22f5fda644e5b5aca6fbb7605b7bc37d8ffe830730fc07";
    const KDFA_600: &str = "b3f9defef114ba5c090845dfc65e6edc79c2b1a81e7b651bb8c5f262eb226ebdb6b4f1050688e39291e6378b24fd89f665b3c6395507603c2fefce38ee86013c6d26ae1205465f9217cce6";
}
