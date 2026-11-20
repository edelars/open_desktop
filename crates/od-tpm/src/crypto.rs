//! Primitives of the vTPM over the RustCrypto crates `hmac`, `sha2`, `rsa` and `p256` (ADR-0009,
//! decision 2): SHA-256/384/512 digests, HMAC and KDFa over them ([`HashAlg`]), a constant-time
//! HMAC check, and (slice B of phase B) RSA-2048 PKCS#1 v1.5 (any of the three hashes) and ECC
//! P-256 ECDSA key generation, signing and verification.
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
use sha2::digest::block_api::EagerHash;
use sha2::{Digest, Sha256, Sha384, Sha512};
use zeroize::Zeroizing;

use crate::codes::alg;
use crate::random::Drbg;

/// Digest size of SHA-256, the hash of contexts, tickets and the DRBG (sessions, objects and
/// snapshots are SHA-256-only too until their hash agility is implemented).
pub(crate) const DIGEST: usize = 32;

/// Block size of SHA-256 in octets (the HMAC key block of `hmac_sha256_state`).
const BLOCK: usize = 64;

/// A hash algorithm of the TPM profile (Part 2, Table 9 identifiers).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HashAlg {
    Sha256,
    Sha384,
    Sha512,
}

impl HashAlg {
    /// The algorithm for a `TPM_ALG_ID`; `None` for anything but SHA-256, SHA-384 and SHA-512.
    pub(crate) fn from_alg(a: u16) -> Option<HashAlg> {
        match a {
            alg::SHA256 => Some(HashAlg::Sha256),
            alg::SHA384 => Some(HashAlg::Sha384),
            alg::SHA512 => Some(HashAlg::Sha512),
            _ => None,
        }
    }

    /// Digest size in octets.
    pub(crate) fn size(self) -> usize {
        match self {
            HashAlg::Sha256 => 32,
            HashAlg::Sha384 => 48,
            HashAlg::Sha512 => 64,
        }
    }

    /// Index 0 (SHA-256), 1 (SHA-384) or 2 (SHA-512): the slot of the per-hash caches of cpHash and
    /// rpHash, and the `authHash` nibble of a saved session body.
    pub(crate) fn index(self) -> usize {
        match self {
            HashAlg::Sha256 => 0,
            HashAlg::Sha384 => 1,
            HashAlg::Sha512 => 2,
        }
    }

    /// The `TPM_ALG_ID`.
    pub(crate) fn alg(self) -> u16 {
        match self {
            HashAlg::Sha256 => alg::SHA256,
            HashAlg::Sha384 => alg::SHA384,
            HashAlg::Sha512 => alg::SHA512,
        }
    }
}

/// A digest of any implemented hash, held inline (at most 64 octets).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct TpmDigest {
    pub len: usize,
    pub bytes: [u8; 64],
}

impl TpmDigest {
    /// The digest held in `out` (at most 64 octets).
    pub(crate) fn from_output(out: &[u8]) -> TpmDigest {
        let mut bytes = [0u8; 64];
        bytes[..out.len()].copy_from_slice(out);
        TpmDigest {
            len: out.len(),
            bytes,
        }
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    #[cfg(test)]
    pub(crate) fn to_vec(self) -> Vec<u8> {
        self.as_slice().to_vec()
    }
}

/// `H(parts[0] || parts[1] || ...)` with the hash `alg`.
pub(crate) fn hash(alg: HashAlg, parts: &[&[u8]]) -> TpmDigest {
    fn run<D: Digest>(parts: &[&[u8]]) -> TpmDigest {
        let mut h = D::new();
        for p in parts {
            h.update(p);
        }
        TpmDigest::from_output(&h.finalize())
    }
    match alg {
        HashAlg::Sha256 => run::<Sha256>(parts),
        HashAlg::Sha384 => run::<Sha384>(parts),
        HashAlg::Sha512 => run::<Sha512>(parts),
    }
}

/// HMAC state of hash `D` for a key of any length. The key block is that of `D` (64 octets for
/// SHA-256, 128 for SHA-384 and SHA-512): the library applies RFC 2104 for the hash's own block.
fn hmac_state<D: EagerHash>(key: &[u8]) -> Hmac<D>
where
    Hmac<D>: KeyInit,
{
    <Hmac<D> as KeyInit>::new_from_slice(key).expect("HMAC accepts a key of any length")
}

/// `HMAC-alg(key, parts[0] || parts[1] || ...)`; `key` may have any length.
pub(crate) fn hmac(alg: HashAlg, key: &[u8], parts: &[&[u8]]) -> TpmDigest {
    fn run<D: EagerHash>(key: &[u8], parts: &[&[u8]]) -> TpmDigest
    where
        Hmac<D>: KeyInit + Mac,
    {
        let mut m = hmac_state::<D>(key);
        for p in parts {
            m.update(p);
        }
        TpmDigest::from_output(&m.finalize().into_bytes())
    }
    match alg {
        HashAlg::Sha256 => run::<Sha256>(key, parts),
        HashAlg::Sha384 => run::<Sha384>(key, parts),
        HashAlg::Sha512 => run::<Sha512>(key, parts),
    }
}

/// Checks `tag` against `HMAC-alg(key, parts)` in constant time (`Mac::verify_slice`); a tag of
/// the wrong length is a mismatch.
pub(crate) fn hmac_verify(alg: HashAlg, key: &[u8], parts: &[&[u8]], tag: &[u8]) -> bool {
    fn run<D: EagerHash>(key: &[u8], parts: &[&[u8]], tag: &[u8]) -> bool
    where
        Hmac<D>: KeyInit + Mac,
    {
        let mut m = hmac_state::<D>(key);
        for p in parts {
            m.update(p);
        }
        m.verify_slice(tag).is_ok()
    }
    match alg {
        HashAlg::Sha256 => run::<Sha256>(key, parts, tag),
        HashAlg::Sha384 => run::<Sha384>(key, parts, tag),
        HashAlg::Sha512 => run::<Sha512>(key, parts, tag),
    }
}

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

/// `HMAC-SHA-256(key, parts[0] || parts[1] || ...)`; `key` may have any length.
pub(crate) fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; DIGEST] {
    let mut out = [0u8; DIGEST];
    out.copy_from_slice(hmac(HashAlg::Sha256, key, parts).as_slice());
    out
}

/// Checks `tag` against `HMAC-SHA-256(key, parts)` in constant time (`Mac::verify_slice`); a tag
/// of the wrong length is a mismatch.
pub(crate) fn hmac_sha256_verify(key: &[u8], parts: &[&[u8]], tag: &[u8]) -> bool {
    hmac_verify(HashAlg::Sha256, key, parts, tag)
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
    kdfa(HashAlg::Sha256, key, label, context_u, context_v, bits)
}

/// KDFa with the hash `alg` (see [`kdfa_sha256`]).
pub(crate) fn kdfa(
    alg: HashAlg,
    key: &[u8],
    label: &[u8],
    context_u: &[u8],
    context_v: &[u8],
    bits: u32,
) -> Zeroizing<Vec<u8>> {
    let bytes = (bits as usize).div_ceil(8);
    let mut out = Zeroizing::new(Vec::with_capacity(bytes + alg.size()));
    let mut counter: u32 = 1;
    while out.len() < bytes {
        let block = hmac(
            alg,
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
        out.extend_from_slice(block.as_slice());
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

/// TPM duplication carries a single RSA prime. Derive the other from the public
/// modulus and validate the factors before handing them to the private-key codec.
pub(crate) fn rsa_from_one_prime(prime: &[u8], modulus: &[u8]) -> Option<RsaPrivate> {
    if prime.len() != PRIME_BYTES || modulus.len() != RSA_BYTES || modulus[0] & 0x80 == 0 {
        return None;
    }
    let modulus_value = BoxedUint::from_be_slice(modulus, 2048).ok()?;
    let prime_value = Zeroizing::new(BoxedUint::from_be_slice(prime, 2048).ok()?);
    let other = Zeroizing::new(modulus_value.checked_div(&*prime_value).into_option()?);
    // Both factors must have squares with the same bit length as the modulus
    // (TPM Library v185 Part 1 §43.1). Both 1024-bit inputs fit the 2048-bit square.
    // rsa_from_primes also checks the exact product.
    if other.bit_width() != 1024
        || Zeroizing::new(prime_value.wrapping_square()).bit_width() != 2048
        || Zeroizing::new(other.wrapping_square()).bit_width() != 2048
    {
        return None;
    }
    let tail = Zeroizing::new(other.to_be_bytes());
    let mut pair = Zeroizing::new(Vec::with_capacity(RSA_BYTES));
    pair.extend_from_slice(prime);
    pair.extend_from_slice(&tail[tail.len() - PRIME_BYTES..]);
    rsa_from_primes(&pair, modulus)
}

/// RSASSA-PKCS1-v1_5 with `hash` over `digest` (already a digest of that hash; `None` for another
/// length). Private operation: see the ISSUE-064 note at the top of this module.
pub(crate) fn rsa_sign(
    key: &RsaPrivate,
    hash: HashAlg,
    digest: &[u8],
    rng: &mut Drbg,
) -> Option<Vec<u8>> {
    let rng = &mut DrbgRng(rng);
    match hash {
        HashAlg::Sha256 => key.pkcs1v15_sign_prehash_with_rng::<Sha256, _>(rng, digest),
        HashAlg::Sha384 => key.pkcs1v15_sign_prehash_with_rng::<Sha384, _>(rng, digest),
        HashAlg::Sha512 => key.pkcs1v15_sign_prehash_with_rng::<Sha512, _>(rng, digest),
    }
    .ok()
}

/// The RSA public key `(n, 65537)`; `None` for a modulus the library rejects.
fn rsa_public_key(n: &[u8]) -> Option<rsa::PublicKey> {
    let n = BoxedUint::from_be_slice(n, (n.len() * 8) as u32).ok()?;
    rsa::PublicKey::new(n, BoxedUint::from(u64::from(RSA_EXPONENT))).ok()
}

/// Verifies an RSASSA-PKCS1-v1_5 SHA-256 signature with the public key `(n, 65537)`.
pub(crate) fn rsa_verify(n: &[u8], digest: &[u8], sig: &[u8]) -> bool {
    rsa_verify_hash(n, HashAlg::Sha256, digest, sig)
}

/// Verifies an RSASSA-PKCS1-v1_5 signature over a `hash` digest with the public key `(n, 65537)`.
pub(crate) fn rsa_verify_hash(n: &[u8], hash: HashAlg, digest: &[u8], sig: &[u8]) -> bool {
    let Some(key) = rsa_public_key(n) else {
        return false;
    };
    let scheme = match hash {
        HashAlg::Sha256 => Pkcs1v15Sign::new::<Sha256>(),
        HashAlg::Sha384 => Pkcs1v15Sign::new::<Sha384>(),
        HashAlg::Sha512 => Pkcs1v15Sign::new::<Sha512>(),
    };
    key.verify(scheme, digest, sig).is_ok()
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

/// P-256 key generation with extra random bits (Part 1 v185 §44.6).
/// Reduce 320 uniform bits modulo the group order minus one, then add one.
pub(crate) fn ecc_generate_extra_random_bits(rng: &mut Drbg) -> Option<EccPrivate> {
    use p256::elliptic_curve::bigint::{NonZero, U256, U320};

    const ORDER_MINUS_ONE: U256 =
        U256::from_be_hex("FFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632550");
    let modulus = NonZero::new(ORDER_MINUS_ONE).expect("P-256 order minus one is nonzero");
    let mut bytes = Zeroizing::new([0u8; 40]);
    rng.generate(&mut bytes[..])?;
    let random = Zeroizing::new(U320::from_be_slice(&bytes[..]));
    let scalar = Zeroizing::new(random.rem(&modulus).wrapping_add(&U256::ONE));
    let encoded = Zeroizing::new(scalar.to_be_bytes());
    ecc_from_scalar(encoded.as_ref())
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

    #[test]
    fn hash_and_hmac_sha256_wrappers_match_the_generic_functions() {
        let d = hash(HashAlg::Sha256, &[b"ab", b"c"]);
        assert_eq!(d.len, 32);
        assert_eq!(
            d.as_slice(),
            hex("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        let key = [7u8; 100];
        assert_eq!(
            hmac_sha256(&key, &[b"x", b"y"]).as_slice(),
            hmac(HashAlg::Sha256, &key, &[b"xy"]).as_slice()
        );
        assert_eq!(
            &kdfa_sha256(&key, b"L", b"u", b"v", 300)[..],
            &kdfa(HashAlg::Sha256, &key, b"L", b"u", b"v", 300)[..]
        );
        for (alg, size, id) in [
            (HashAlg::Sha256, 32, 0x000B),
            (HashAlg::Sha384, 48, 0x000C),
            (HashAlg::Sha512, 64, 0x000D),
        ] {
            assert_eq!((alg.size(), alg.alg()), (size, id));
            assert_eq!(HashAlg::from_alg(id), Some(alg));
            assert_eq!(hash(alg, &[]).len, size);
            let tag = hmac(alg, b"k", &[b"data"]);
            assert!(hmac_verify(alg, b"k", &[b"data"], tag.as_slice()));
            assert!(!hmac_verify(alg, b"k", &[b"datb"], tag.as_slice()));
            assert!(!hmac_verify(alg, b"k", &[b"data"], &tag.as_slice()[1..]));
        }
        assert_eq!(HashAlg::from_alg(0x0004), None);
        assert_eq!(HashAlg::from_alg(0x0027), None);
        assert_eq!(HashAlg::from_alg(0x0010), None);
    }

    #[test]
    fn sha512_matches_the_published_abc_vector() {
        assert_eq!(
            hash(HashAlg::Sha512, &[b"a", b"bc"]).as_slice(),
            hex(
                "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
            )
        );
    }

    // Computed outside the implementation with Python hmac/hashlib (SP 800-108 counter mode as in
    // Part 1, clause 9.4.10.2): key = 32 x 0x01, label "ATH", contextU = 16 x 0x02,
    // contextV = 16 x 0x03, with 384, 512 and 600 bits (several blocks of 48 / 64 octets).
    #[test]
    fn kdfa_sha384_sha512_match_independent_vectors() {
        let (key, u, v) = ([1u8; 32], [2u8; 16], [3u8; 16]);
        for (alg, bits, expected) in [
            (
                HashAlg::Sha384,
                384,
                "616d97c04d97197abccb1fc58922bdecd4538a4f2786a1bff5df45c3d7adf688a093163b68e965fd597434a767027698",
            ),
            (
                HashAlg::Sha384,
                512,
                "6fd1b43479db67b5ccacdb123a4844354de192db242fe5f88f2cc078a5a7ed098e5bf8de4b3543a2437ab90857b0539a413a60d9a2d1b809e5d1c606b9cc4aaf",
            ),
            (
                HashAlg::Sha384,
                600,
                "f02fc2fb91bd9da254b6703aca4d714443769d927e9b05d7d3d6cad38839a45775aff5c7827de8d6b6e64a9b535d29a0f5054b58b5fc1fd305f7abb7a27122cedc40f29c3c7e1947d56042",
            ),
            (
                HashAlg::Sha512,
                384,
                "6b9ec12eb88457198f9fdbba1006b600c5051999ed8a822977167de1ab8410ea762269fb1ca4bfc2743909c92470500a",
            ),
            (
                HashAlg::Sha512,
                512,
                "12bb228d1b396014d0e513569a8b96338da5448f34bcdc9e0d8355b19676c7710df2317cb74c678e30426339b66080a237237b71bcf05b902730a151d35c58e3",
            ),
            (
                HashAlg::Sha512,
                600,
                "e67bb9db2b69a2714b3aab6bf7fe077b66aac683099af2c74c8f6db8c95ee2184bb7bdaeb52d472a7ea96f5a813bb12a60667fe13d9d5699ca3193212e3b4e7105d0399b2b0dc2ff3e6127",
            ),
        ] {
            let out = kdfa(alg, &key, b"ATH", &u, &v, bits);
            assert_eq!(out.len(), (bits as usize).div_ceil(8));
            assert_eq!(&out[..], &hex(expected)[..], "{alg:?} {bits}");
        }
    }

    // RFC 4231 test cases 1, 2 and 6 (the key of case 6 is 131 octets, longer than the 128-octet
    // block of SHA-384/SHA-512, so it must be hashed first), and a 100-octet key that is longer
    // than the SHA-256 block but shorter than the SHA-384/SHA-512 one; the last expected value
    // computed with Python hmac.
    #[test]
    fn hmac_sha384_sha512_match_rfc_4231() {
        let case6 = [
            &b"Test Using Larger Than Block-Size Key - "[..],
            &b"Hash Key First"[..],
        ];
        let jefe = [&b"what do ya want "[..], &b"for nothing?"[..]];
        for (alg, c1, c2, c6, c100) in [
            (
                HashAlg::Sha384,
                "afd03944d84895626b0825f4ab46907f15f9dadbe4101ec682aa034c7cebc59cfaea9ea9076ede7f4af152e8b2fa9cb6",
                "af45d2e376484031617f78d2b58a6b1b9c7ef464f5a01b47e42ec3736322445e8e2240ca5e69e2c78b3239ecfab21649",
                "4ece084485813e9088d2c63a041bc5b44f9ef1012a2b588f3cd11f05033ac4c60c2ef6ab4030fe8296248df163f44952",
                "91c2453fce427e190b74520d00eb72245d1d92277e3d7cc74142b0ff5e2293b88d5867837cdcc2fbe33fbf0ecb369406",
            ),
            (
                HashAlg::Sha512,
                "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cdedaa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854",
                "164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea2505549758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737",
                "80b24263c7c1a3ebb71493c1dd7be8b49b46d1f41b4aeec1121b013783f8f3526b56d037e05f2598bd0fd2215d6a1e5295e64f73f63f0aec8b915a985d786598",
                "fae1f6ccea063431488787cda53ed9b3b8f14a9c0715ce105f78d5a271ef47acac8f7eb2ec566e50f22531035071b224da5df6e6f3dbf959790c2dbaad258565",
            ),
        ] {
            let tag = |key: &[u8], parts: &[&[u8]]| hmac(alg, key, parts);
            assert_eq!(tag(&[0x0b; 20], &[b"Hi There"]).as_slice(), &hex(c1)[..]);
            assert_eq!(tag(b"Jefe", &jefe).as_slice(), &hex(c2)[..]);
            assert_eq!(tag(&[0xaa; 131], &case6).as_slice(), &hex(c6)[..]);
            assert_eq!(tag(&[0xaa; 100], &[b"Hi There"]).as_slice(), &hex(c100)[..]);
            assert!(hmac_verify(alg, &[0xaa; 131], &case6, &hex(c6)));
            assert!(!hmac_verify(
                alg,
                &[0xaa; 131],
                &case6,
                &hex(c6)[..alg.size() - 1]
            ));
        }
    }

    const RSA_SHA2_MESSAGE: &[u8] = b"Open Desktop SHA-2 agility";

    // PKCS#1 v1.5 encoded messages (DigestInfo prefixes of RFC 8017, note 1) and pow(EM, d, n)
    // computed in Python with the private exponent derived from the self-test primes.
    #[test]
    fn rsassa_sha384_sha512_match_python() {
        use crate::self_test::{EXT_RSA_N, EXT_RSA_P, EXT_RSA_Q};
        let n = hex(EXT_RSA_N);
        let pq = [hex(EXT_RSA_P), hex(EXT_RSA_Q)].concat();
        let key = rsa_from_primes(&pq, &n).expect("self-test key");
        let mut rng = Drbg::new(&[9; 48]).unwrap();
        for (alg, digest, signature) in [
            (
                HashAlg::Sha384,
                "ea1e2f1e41699e15c434570e2a30cb428f9c42ca2c09a06f6b5932b0fc2c7af9310259aa1b579ed9c6c14b3cae501601",
                "3f87909d7fa6ad591c25e711ca7914318f82c45ddac87e06ca5966c0eb5603a38168cf8a37e764b778ef351dd43ed6a154be958ddabfc173fe2bdfb26e772baaeb173093d0c02266d3019e9bb3f95ac4037cb50e47bc8bf459d55d599db7a0c857e02301fbc28d2d9e375f5ffba6571770c81d524fac5f29bd4eebd582f36c11a69fc7900c2d9ef0aac5d468d88ba6d06ff9276da065c3c09498f1fbc59f1b0d03e6bf0f9595a24228842fe72ca8aadb6b92fdc5c6c598aecc2e54389240276e4c7cdfb802dd8ecd54df538f574ecf761231297c7158b9878ce7960377e64559e0efe893fc52c683c84b249e155826d246d4bf4b6d747b6087153467f9bf5620",
            ),
            (
                HashAlg::Sha512,
                "ec4d110bd09591930a6a317710851481b49d7c4607b33744db0b1ddb0c5fd636c4a185dc3ac5738289226d00ccd91d2c8ea2ad96be705b4e6cd8581dc3caf780",
                "79762a7568771c314eb7223d8a0c38c77af2b52dd7d08abf9596499fc5176c97f9cf07c785bbca09f5ee903aa4c75ab68a9f2be6aa36c1486a71ede89005aa19743bab518bdc14192a56be5fad71dd1d5e89c2788627d9fe5101ce0552c8adb264d5ec9fa691af12eb7de97ad1c557514c573141f69bffd4e87a44948f9c8658579669306ad6c2d38f3145ec568ece8380e7e10a6d6496c65e1b416c2eae1a75651de73afcd37e920305939258feaaf3c0bf4d5deae3601642a3160f14c97baa3bf91bf8346b0ce9546d3cc732a133691195c80067a9816bbee028065223579c03554b7df820a7529604b74d9c0e4925a5d241048e233e9f794d66cc66ee9c53",
            ),
        ] {
            let digest = hex(digest);
            assert_eq!(hash(alg, &[RSA_SHA2_MESSAGE]).as_slice(), &digest[..]);
            let sig = rsa_sign(&key, alg, &digest, &mut rng).unwrap();
            assert_eq!(sig, hex(signature), "{alg:?}");
            assert!(rsa_verify_hash(&n, alg, &digest, &sig));
            // Another hash or a changed digest does not verify.
            let other = if alg == HashAlg::Sha384 {
                HashAlg::Sha512
            } else {
                HashAlg::Sha384
            };
            assert!(!rsa_verify_hash(&n, other, &digest, &sig));
            assert!(!rsa_verify_hash(&n, HashAlg::Sha256, &digest, &sig));
            let mut bad = digest.clone();
            bad[0] ^= 1;
            assert!(!rsa_verify_hash(&n, alg, &bad, &sig));
            // A digest of the wrong length for the hash is refused by the signer.
            assert!(rsa_sign(&key, alg, &digest[..32], &mut rng).is_none());
        }
        // The SHA-256 wrappers are unchanged.
        let digest = hash(HashAlg::Sha256, &[RSA_SHA2_MESSAGE]);
        let sig = rsa_sign(&key, HashAlg::Sha256, digest.as_slice(), &mut rng).unwrap();
        assert!(rsa_verify(&n, digest.as_slice(), &sig));
        assert!(!rsa_verify_hash(
            &n,
            HashAlg::Sha384,
            digest.as_slice(),
            &sig
        ));
    }

    // RFC 6979 Appendix A.2.5 (ECDSA, 256 bits (P-256)), message "sample": the published
    // (r, s) for SHA-384 and SHA-512 and the public key U; a Python verifier agreed with all
    // three. The digests are longer than the 32-octet field: the leftmost 256 bits are used.
    #[test]
    fn ecdsa_verifies_rfc6979_sha384_sha512() {
        let x = hex("60fed4ba255a9d31c961eb74c6356d68c049b8923b61fa6ce669622e60f29fb6");
        let y = hex("7903fe1008b8bc99a41ae9e95628bc64f2f1b20c2d7e9f5177a3c294d4462299");
        for (alg, digest, r, s) in [
            (
                HashAlg::Sha384,
                "9a9083505bc92276aec4be312696ef7bf3bf603f4bbd381196a029f340585312313bca4a9b5b890efee42c77b1ee25fe",
                "0eafea039b20e9b42309fb1d89e213057cbf973dc0cfc8f129edddc800ef7719",
                "4861f0491e6998b9455193e34e7b0d284ddd7149a74b95b9261f13abde940954",
            ),
            (
                HashAlg::Sha512,
                "39a5e04aaff7455d9850c605364f514c11324ce64016960d23d5dc57d3ffd8f49a739468ab8049bf18eef820cdb1ad6c9015f838556bc7fad4138b23fdf986c7",
                "8496a60b5e9b47c825488827e0495b0e3fa109ec4568fd3f8d1097678eb97f00",
                "2362ab1adbe2b8adf9cb9edab740ea6049c028114f2460f96554f61fae3302fe",
            ),
        ] {
            let digest = hex(digest);
            assert_eq!(hash(alg, &[b"sample"]).as_slice(), &digest[..]);
            let (r, s) = (hex(r), hex(s));
            assert!(ecc_verify(&x, &y, &digest, &r, &s), "{alg:?}");
            let mut bad = digest.clone();
            bad[0] ^= 1;
            assert!(!ecc_verify(&x, &y, &bad, &r, &s));
            // The SHA-384 signature does not verify the SHA-512 digest and vice versa.
            let other = hash(
                if alg == HashAlg::Sha384 {
                    HashAlg::Sha512
                } else {
                    HashAlg::Sha384
                },
                &[b"sample"],
            );
            assert!(!ecc_verify(&x, &y, other.as_slice(), &r, &s));
        }
    }

    // Computed independently (Python hmac/hashlib), 2026-11-07; see the test above.
    const KDFA_256: &str = "32316f0b89c58eaa1e22f5fda644e5b5aca6fbb7605b7bc37d8ffe830730fc07";
    const KDFA_600: &str = "b3f9defef114ba5c090845dfc65e6edc79c2b1a81e7b651bb8c5f262eb226ebdb6b4f1050688e39291e6378b24fd89f665b3c6395507603c2fefce38ee86013c6d26ae1205465f9217cce6";
}
