//! Random number generation: HMAC_DRBG (NIST SP 800-90A, section 10.1.2) over SHA-256,
//! seeded by entropy the owner passes in, plus `TPM2_GetRandom` and `TPM2_StirRandom`.
//!
//! Part 1 (v184, clause 9.4.11.2) recommends a DRBG that follows SP 800-90A (revision of
//! March 2012) for a non-FIPS TPM; HMAC_DRBG is such a mechanism (ADR-0009 open question 1).
//! NOT VERIFIED: the text of SP 800-90A itself was not read; the update/generate/reseed steps
//! below follow the algorithm as published and are checked against an independent Python
//! implementation (see the `drbg_matches_independent_python_vector` test), not against NIST CAVP
//! vectors. The generator is not FIPS-validated and no such claim is made.
//!
//! The HMAC helper below exists only because the `hmac` crate (ADR-0009: `=0.13.0`) is not in
//! the offline cache and belongs to S7-23; S7-23 replaces it with the crate (DRY).
//! Obligation of S7-23: replace this helper with `hmac =0.13.0` (see `docs/specs/VTPM.md`).
//!
//! State (`K`, `V`) is wiped on drop; `Debug` never prints it.

use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::marshal::{Reader, Writer, limits};
use crate::response::{Output, Rc};
use crate::state::Tpm;

const BLOCK: usize = 64;
const OUT: usize = 32;
/// Minimum seed material: 32 octets of entropy input plus a 16 octet nonce (SP 800-90A, 10.1.2.1
/// for security strength 256).
pub const MIN_ENTROPY_LEN: usize = 48;
/// `reseed_interval`: 2^48 generate calls (SP 800-90A, table 2).
const RESEED_INTERVAL: u64 = 1 << 48;
const PERSONALIZATION: &[u8] = b"open-desktop od-tpm hmac-drbg-sha256 v1";

/// Deliberate limitation: the key is always exactly 32 octets (`K` of HMAC_DRBG with SHA-256), so
/// hashing of keys longer than the block size is not needed and not implemented.
fn hmac_sha256(key: &[u8; OUT], parts: &[&[u8]]) -> [u8; OUT] {
    let mut k = [0u8; BLOCK];
    k[..OUT].copy_from_slice(key);
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5Cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    for p in parts {
        inner.update(p);
    }
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    let mut out = [0u8; OUT];
    out.copy_from_slice(&outer.finalize());
    k.zeroize();
    ipad.zeroize();
    opad.zeroize();
    out
}

/// HMAC_DRBG with SHA-256.
pub struct Drbg {
    k: [u8; OUT],
    v: [u8; OUT],
    reseed_counter: u64,
}

impl core::fmt::Debug for Drbg {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Drbg(..)")
    }
}

impl Drop for Drbg {
    fn drop(&mut self) {
        self.k.zeroize();
        self.v.zeroize();
    }
}

impl Drbg {
    /// Instantiates from at least [`MIN_ENTROPY_LEN`] octets of owner-supplied entropy.
    pub fn new(entropy: &[u8]) -> Option<Drbg> {
        if entropy.len() < MIN_ENTROPY_LEN {
            return None;
        }
        let mut d = Drbg {
            k: [0x00; OUT],
            v: [0x01; OUT],
            reseed_counter: 1,
        };
        d.update(&[entropy, PERSONALIZATION]);
        Some(d)
    }

    /// HMAC_DRBG_Update (SP 800-90A, 10.1.2.2).
    fn update(&mut self, provided: &[&[u8]]) {
        let empty = provided.iter().all(|p| p.is_empty());
        for round in 0u8..2 {
            if round == 1 && empty {
                break;
            }
            let mut parts: Vec<&[u8]> = vec![&self.v[..], core::slice::from_ref(&round)];
            parts.extend_from_slice(provided);
            self.k = hmac_sha256(&self.k, &parts);
            self.v = hmac_sha256(&self.k, &[&self.v]);
        }
    }

    /// Mixes caller data into the state without crediting it as entropy (`TPM2_StirRandom`).
    pub fn stir(&mut self, data: &[u8]) {
        self.update(&[data]);
    }

    /// Reseeds with fresh owner-supplied entropy (restore of a snapshot).
    pub fn reseed(&mut self, entropy: &[u8]) {
        self.update(&[entropy]);
        self.reseed_counter = 1;
    }

    /// HMAC_DRBG_Generate (SP 800-90A, 10.1.2.5) without additional input. Returns `None` when
    /// a reseed is required (never reached in practice: 2^48 calls).
    pub fn generate(&mut self, out: &mut [u8]) -> Option<()> {
        if self.reseed_counter > RESEED_INTERVAL {
            return None;
        }
        let mut filled = 0;
        while filled < out.len() {
            self.v = hmac_sha256(&self.k, &[&self.v]);
            let n = (out.len() - filled).min(OUT);
            out[filled..filled + n].copy_from_slice(&self.v[..n]);
            filled += n;
        }
        self.update(&[]);
        self.reseed_counter += 1;
        Some(())
    }

    /// 32 random octets (a hierarchy seed).
    pub fn generate_array(&mut self) -> Option<[u8; OUT]> {
        let mut a = [0u8; OUT];
        self.generate(&mut a)?;
        Some(a)
    }
}

/// Known-answer test of the HMAC helper that the DRBG relies on (RFC 4231, test case 1).
pub(crate) fn self_test() -> bool {
    let mut key = [0u8; OUT];
    key[..20].fill(0x0b);
    let expect: [u8; OUT] = [
        0xb0, 0x34, 0x4c, 0x61, 0xd8, 0xdb, 0x38, 0x53, 0x5c, 0xa8, 0xaf, 0xce, 0xaf, 0x0b, 0xf1,
        0x2b, 0x88, 0x1d, 0xc2, 0x00, 0xc9, 0x83, 0x3d, 0xa7, 0x26, 0xe9, 0x37, 0x6c, 0x2e, 0x32,
        0xcf, 0xf7,
    ];
    hmac_sha256(&key, &[b"Hi There"]) == expect
}

/// Test helper: HMAC-SHA-256 with a 32-octet key (RFC 4231 cases use shorter keys, so tests pad).
#[cfg(test)]
pub(crate) fn hmac_for_tests(key: &[u8; OUT], data: &[u8]) -> [u8; OUT] {
    hmac_sha256(key, &[data])
}

impl Tpm {
    /// `TPM2_GetRandom` (Part 3, clause 16.1): `bytesRequested` octets, at most one digest.
    pub(crate) fn cmd_get_random(&mut self, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let requested = usize::from(p.u16()?);
        p.finish()?;
        // The result is a TPM2B_DIGEST: no error when more is requested, only what fits.
        let n = requested.min(limits::MAX_DIGEST_SIZE);
        let mut buf = vec![0u8; n];
        self.drbg.generate(&mut buf).ok_or(Rc::FAILURE)?;
        let mut w = Writer::new();
        w.tpm2b(&buf);
        buf.zeroize();
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }

    /// `TPM2_StirRandom` (Part 3, clause 16.2): at most 128 octets of additional data.
    pub(crate) fn cmd_stir_random(&mut self, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let data = p.tpm2b(limits::MAX_STIR_SIZE)?;
        p.finish()?;
        self.drbg.stir(data);
        Ok(Output::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad32(key: &[u8]) -> [u8; 32] {
        let mut k = [0u8; 32];
        k[..key.len()].copy_from_slice(key);
        k
    }

    #[test]
    fn hmac_matches_rfc_4231() {
        // RFC 4231, test case 2 (key "Jefe" zero-padded to the 32 octets this helper takes:
        // HMAC pads keys shorter than the block with zeros, so the result is identical).
        let mac = hmac_for_tests(&pad32(b"Jefe"), b"what do ya want for nothing?");
        assert_eq!(
            mac,
            hex32("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")
        );
        // RFC 4231, test case 1: key 0x0b x 20, data "Hi There".
        let mac = hmac_for_tests(&pad32(&[0x0b; 20]), b"Hi There");
        assert_eq!(
            mac,
            hex32("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7")
        );
    }

    fn hex32(s: &str) -> [u8; 32] {
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
        }
        out
    }

    #[test]
    fn drbg_matches_independent_python_vector() {
        // Computed independently with Python `hmac`/`hashlib` (SP 800-90A 10.1.2
        // update/instantiate/generate re-implemented from the algorithm in a separate script),
        // entropy = bytes 0..48, personalization as in this module, two generate calls of 32
        // and 40 octets.
        let entropy: Vec<u8> = (0u8..48).collect();
        let mut d = Drbg::new(&entropy).unwrap();
        let mut a = [0u8; 32];
        d.generate(&mut a).unwrap();
        let mut b = [0u8; 40];
        d.generate(&mut b).unwrap();
        assert_eq!(a, hex32(DRBG_FIRST));
        assert_eq!(b.to_vec(), hex_vec(DRBG_SECOND));
    }

    // Computed independently (Python hmac/hashlib), 2026-11-07; see the test above.
    const DRBG_FIRST: &str = "5990bc4a3ace179f764ce400fb04ea068702231318252f994164efcb56ed50fe";
    const DRBG_SECOND: &str =
        "600751a52d539ac1458309cd147cfed5585468bffd48b981458c9fa61c01dea1ab9a16b913c8cb12";

    fn hex_vec(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn drbg_rejects_short_entropy_and_is_deterministic() {
        assert!(Drbg::new(&[0u8; 47]).is_none());
        let e = [9u8; 48];
        let mut a = Drbg::new(&e).unwrap();
        let mut b = Drbg::new(&e).unwrap();
        let (mut x, mut y) = ([0u8; 16], [0u8; 16]);
        a.generate(&mut x).unwrap();
        b.generate(&mut y).unwrap();
        assert_eq!(x, y);
        b.stir(b"more");
        a.generate(&mut x).unwrap();
        b.generate(&mut y).unwrap();
        assert_ne!(x, y);
    }

    #[test]
    fn debug_does_not_print_state() {
        let d = Drbg::new(&[1u8; 48]).unwrap();
        assert_eq!(format!("{d:?}"), "Drbg(..)");
    }
}
