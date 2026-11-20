//! TPM credential protection (Library v185 Part 1 §§21.4–5; Part 3 §12.6).

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::crypto::{
    DIGEST, RsaPadding, ecc_ecdh, ecc_generate_extra_random_bits, ecc_public, hmac_sha256,
    kdfa_sha256, rsa_encrypt_public,
};
use crate::dispatch::Invocation;
use crate::marshal::{MAX_DATA, PublicParams, Reader, Scheme, SymDef, Unique, Writer, limits};
use crate::object::{aes128_cfb, find_object};
use crate::random::Drbg;
use crate::response::{Output, Rc};
use crate::state::Tpm;

const IDENTITY_LABEL: &[u8] = b"IDENTITY\0";

impl Tpm {
    pub(crate) fn cmd_make_credential(
        &mut self,
        inv: &Invocation,
        reader: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        reader.set_param(1);
        let credential = reader.tpm2b(limits::MAX_DIGEST_SIZE)?;
        reader.set_param(2);
        let object_name = reader.tpm2b(MAX_DATA)?;
        reader.finish()?;

        let key = find_object(&self.objects, &self.persistent.objects, inv.handles[0])
            .ok_or(Rc::reference_handle(0))?;
        if !key.is_storage() {
            return Err(Rc::TYPE.in_handle(1));
        }
        let (symmetric, scheme) = match key.public.params {
            PublicParams::Rsa {
                symmetric, scheme, ..
            }
            | PublicParams::Ecc {
                symmetric, scheme, ..
            } => (symmetric, scheme),
            _ => return Err(Rc::TYPE.in_handle(1)),
        };
        // LoadExternal admits public keys for several uses. A Storage key
        // requires a NULL asymmetric scheme and a supported symmetric cipher.
        if scheme != Scheme::NULL {
            return Err(Rc::VALUE.in_handle(1));
        }
        if symmetric != SymDef::Aes128Cfb {
            return Err(Rc::TYPE.in_handle(1));
        }
        let (seed, secret) = make_seed(&key.public.unique, &mut self.drbg).ok_or(Rc::FAILURE)?;
        let blob = protect(&seed[..], object_name, credential);
        let mut output = Writer::new();
        output.tpm2b(&blob);
        output.tpm2b(&secret);
        Ok(Output {
            handles: Vec::new(),
            params: output.finish(),
        })
    }
}

/// Seed and encrypted secret, using the storage key's supported Name hash (SHA-256).
fn make_seed(public: &Unique, rng: &mut Drbg) -> Option<(Zeroizing<[u8; DIGEST]>, Vec<u8>)> {
    match public {
        Unique::Rsa(modulus) => {
            let mut seed = Zeroizing::new([0u8; DIGEST]);
            rng.generate(&mut seed[..])?;
            let cipher =
                rsa_encrypt_public(modulus, RsaPadding::Oaep, IDENTITY_LABEL, &seed[..], rng)?;
            Some((seed, cipher))
        }
        Unique::Ecc { x, y } => {
            let ephemeral = ecc_generate_extra_random_bits(rng)?;
            let (ephemeral_x, ephemeral_y) = ecc_public(&ephemeral)?;
            let (shared_x, _) = ecc_ecdh(&ephemeral, x, y)?;
            let seed = identity_kdfe(&shared_x, &ephemeral_x, x);
            let mut point = Writer::new();
            point.tpm2b(&ephemeral_x);
            point.tpm2b(&ephemeral_y);
            Some((seed, point.finish()))
        }
        _ => None,
    }
}

/// Single SHA-256 block of KDFe, with the ephemeral point as party U.
fn identity_kdfe(
    shared_x: &[u8],
    ephemeral_x: &[u8],
    receiver_x: &[u8],
) -> Zeroizing<[u8; DIGEST]> {
    let mut hash = Sha256::new();
    hash.update(1u32.to_be_bytes());
    hash.update(shared_x);
    hash.update(IDENTITY_LABEL);
    hash.update(ephemeral_x);
    hash.update(receiver_x);
    Zeroizing::new(hash.finalize().into())
}

fn protect(seed: &[u8], name: &[u8], credential: &[u8]) -> Vec<u8> {
    let sym_key = kdfa_sha256(seed, b"STORAGE", name, &[], 128);
    let hmac_key = kdfa_sha256(seed, b"INTEGRITY", &[], &[], (DIGEST * 8) as u32);
    let mut clear = Zeroizing::new(Vec::with_capacity(2 + credential.len()));
    clear.extend_from_slice(&(credential.len() as u16).to_be_bytes());
    clear.extend_from_slice(credential);
    let encrypted = aes128_cfb(&sym_key, &[0; 16], &clear, true);
    let integrity = hmac_sha256(&hmac_key, &[&encrypted, name]);
    let mut blob = Writer::new();
    blob.tpm2b(&integrity);
    // The inner length of the credential is encrypted with its contents.
    blob.bytes(&encrypted);
    blob.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_kdfe_and_wrapping_match_external_vectors() {
        // Python hashlib/HMAC, and OpenSSL AES-128-CFB with an all-zero IV.
        let mut name = [2u8; 34];
        name[..2].copy_from_slice(&[0, 0x0b]);
        assert_eq!(
            protect(&[1; 32], &name, &[3; 32]),
            [
                0x00, 0x20, 0x89, 0x46, 0xb6, 0x7b, 0x4c, 0xa4, 0x60, 0xb3, 0x6c, 0xa8, 0xd6, 0x9b,
                0x3e, 0xec, 0x7a, 0x89, 0xb6, 0xaf, 0x4d, 0x25, 0x33, 0xe6, 0xb2, 0x6e, 0x6e, 0x2b,
                0x98, 0x6c, 0x52, 0x33, 0x16, 0xaf, 0xe0, 0x19, 0x6e, 0x1c, 0x3b, 0x03, 0x9d, 0x55,
                0x07, 0x2e, 0x43, 0x2c, 0x21, 0x68, 0x6d, 0xa1, 0xf5, 0x62, 0x54, 0xb1, 0x54, 0x06,
                0x81, 0xc3, 0xc1, 0xf3, 0xec, 0x67, 0xcb, 0xec, 0x35, 0x37, 0x53, 0x0b,
            ]
        );
        assert_eq!(
            *identity_kdfe(&[1; 32], &[2; 32], &[3; 32]),
            [
                0x20, 0x73, 0xa3, 0x30, 0x6e, 0xed, 0xcf, 0xb1, 0x78, 0x18, 0x3d, 0x8a, 0x72, 0x1f,
                0x94, 0x9b, 0xba, 0x9c, 0x3d, 0x45, 0x01, 0xb6, 0xc2, 0x83, 0x94, 0x87, 0xbb, 0xdd,
                0x3b, 0x97, 0x30, 0x70,
            ]
        );
    }
}
