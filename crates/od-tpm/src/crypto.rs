//! Symmetric primitives of the vTPM over the RustCrypto crates `hmac` and `sha2` (ADR-0009,
//! decision 2): HMAC-SHA-256, a constant-time HMAC check and KDFa.
//!
//! The protocol-level constructions (KDFa, session HMACs, the DRBG that uses them) stay in this
//! crate; only the primitives come from the libraries. No function here logs or prints a key.

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// Digest size of SHA-256, the only session and DRBG hash.
pub(crate) const DIGEST: usize = 32;

/// Block size of SHA-256 in octets (the HMAC key block).
const BLOCK: usize = 64;

fn mac(key: &[u8], parts: &[&[u8]]) -> Hmac<Sha256> {
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
    let mut m = <Hmac<Sha256> as KeyInit>::new(&(*block).into());
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
