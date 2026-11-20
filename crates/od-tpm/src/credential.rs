//! TPM credential protection (Library v185 Part 1 §21.4; Part 3 §§12.5–6).

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::crypto::{
    DIGEST, RsaPadding, ecc_ecdh, ecc_generate_extra_random_bits, ecc_public, hmac_sha256,
    hmac_sha256_verify, kdfa_sha256, rsa_decrypt_private, rsa_encrypt_public,
};
use crate::dispatch::Invocation;
use crate::marshal::{
    MAX_DATA, MAX_ECC_BYTES, MAX_RSA_BYTES, PublicParams, Reader, Scheme, SymDef, Unique, Writer,
    limits,
};
use crate::object::{Object, Secret, aes128_cfb, find_object};
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
        check_storage_key(key, 1)?;
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

    pub(crate) fn cmd_activate_credential(
        &mut self,
        inv: &Invocation,
        reader: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        reader.set_param(1);
        let blob = reader.tpm2b(4 + 2 * DIGEST)?;
        reader.set_param(2);
        let secret = reader.tpm2b(MAX_RSA_BYTES)?;
        reader.finish()?;

        let activated = find_object(&self.objects, &self.persistent.objects, inv.handles[0])
            .ok_or(Rc::reference_handle(0))?;
        let key = find_object(&self.objects, &self.persistent.objects, inv.handles[1])
            .ok_or(Rc::reference_handle(1))?;
        check_storage_key(key, 2)?;
        let mut id = Reader::new(blob);
        id.set_param(1);
        let tag = id.tpm2b(DIGEST)?;
        let encrypted = id.bytes(id.remaining())?;
        let seed = recover_seed(key, secret, &mut self.drbg)?;
        if !hmac_sha256_verify(&integrity_key(&seed), &[encrypted, &activated.name], tag) {
            return Err(Rc::INTEGRITY.in_param(1));
        }

        let clear = crypt_identity(&seed, &activated.name, encrypted, false);
        let mut value = Reader::new(&clear);
        let credential = value
            .tpm2b(DIGEST)
            .and_then(|credential| value.finish().map(|()| credential))
            .map_err(|_| Rc::SIZE.in_param(1))?;
        let mut output = Writer::new();
        output.tpm2b(credential);
        Ok(Output {
            handles: Vec::new(),
            params: output.finish(),
        })
    }
}

fn check_storage_key(key: &Object, position: u8) -> Result<(), Rc> {
    if !key.is_storage() {
        return Err(Rc::TYPE.in_handle(position));
    }
    let (symmetric, scheme) = match key.public.params {
        PublicParams::Rsa {
            symmetric, scheme, ..
        }
        | PublicParams::Ecc {
            symmetric, scheme, ..
        } => (symmetric, scheme),
        _ => return Err(Rc::TYPE.in_handle(position)),
    };
    // LoadExternal admits public keys for several uses. A Storage key
    // requires a NULL asymmetric scheme and a supported symmetric cipher.
    if scheme != Scheme::NULL {
        return Err(Rc::VALUE.in_handle(position));
    }
    if symmetric != SymDef::Aes128Cfb {
        return Err(Rc::TYPE.in_handle(position));
    }
    Ok(())
}

fn recover_seed(key: &Object, encrypted: &[u8], rng: &mut Drbg) -> Result<Zeroizing<Vec<u8>>, Rc> {
    let sensitive = key.sensitive.as_ref().ok_or(Rc::AUTH_UNAVAILABLE)?;
    match (&sensitive.secret, &key.public.unique) {
        (Secret::Rsa(rsa), Unique::Rsa(modulus)) => {
            let error = Rc::VALUE.in_param(2);
            if encrypted.len() != modulus.len() {
                return Err(error);
            }
            rsa_decrypt_private(rsa, RsaPadding::Oaep, IDENTITY_LABEL, encrypted, rng)
                .filter(|seed| seed.len() <= DIGEST)
                .ok_or(error)
        }
        (Secret::Ecc(ecc), Unique::Ecc { x: receiver_x, .. }) => {
            let error = Rc::ECC_POINT.in_param(2);
            let mut point = Reader::new(encrypted);
            let x = point.tpm2b(MAX_ECC_BYTES).map_err(|_| error)?;
            let y = point.tpm2b(MAX_ECC_BYTES).map_err(|_| error)?;
            point.finish().map_err(|_| error)?;
            if x.len() != MAX_ECC_BYTES || y.len() != MAX_ECC_BYTES {
                return Err(error);
            }
            let (shared_x, _) = ecc_ecdh(ecc, x, y).ok_or(error)?;
            Ok(Zeroizing::new(
                identity_kdfe(&shared_x, x, receiver_x).to_vec(),
            ))
        }
        _ => Err(Rc::TYPE.in_handle(2)),
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
    let mut clear = Zeroizing::new(Vec::with_capacity(2 + credential.len()));
    clear.extend_from_slice(&(credential.len() as u16).to_be_bytes());
    clear.extend_from_slice(credential);
    let encrypted = crypt_identity(seed, name, &clear, true);
    let integrity = hmac_sha256(&integrity_key(seed), &[&encrypted, name]);
    let mut blob = Writer::new();
    blob.tpm2b(&integrity);
    // The inner length of the credential is encrypted with its contents.
    blob.bytes(&encrypted);
    blob.finish()
}

pub(crate) fn self_test() -> bool {
    // Independent Python SHA-256 vector for the credential KDFe transcript.
    *identity_kdfe(&[1; 32], &[2; 32], &[3; 32])
        == [
            0x20, 0x73, 0xa3, 0x30, 0x6e, 0xed, 0xcf, 0xb1, 0x78, 0x18, 0x3d, 0x8a, 0x72, 0x1f,
            0x94, 0x9b, 0xba, 0x9c, 0x3d, 0x45, 0x01, 0xb6, 0xc2, 0x83, 0x94, 0x87, 0xbb, 0xdd,
            0x3b, 0x97, 0x30, 0x70,
        ]
}

fn integrity_key(seed: &[u8]) -> Zeroizing<Vec<u8>> {
    kdfa_sha256(seed, b"INTEGRITY", &[], &[], (DIGEST * 8) as u32)
}

fn crypt_identity(seed: &[u8], name: &[u8], data: &[u8], encrypt: bool) -> Zeroizing<Vec<u8>> {
    let key = kdfa_sha256(seed, b"STORAGE", name, &[], 128);
    aes128_cfb(&key, &[0; 16], data, encrypt)
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
        assert!(self_test());
    }
}
