//! TPM credential protection and seed sharing (Library v185 Part 1 §§21.3–5, 43.10 and 44.7).

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::crypto::{
    DIGEST, RsaPadding, ecc_ecdh, ecc_generate_extra_random_bits, ecc_public, hmac_sha256,
    hmac_sha256_verify, kdfa_sha256, rsa_decrypt_private, rsa_encrypt_public,
};
use crate::dispatch::Invocation;
use crate::marshal::{
    MAX_DATA, MAX_ECC_BYTES, MAX_RSA_BYTES, Reader, Scheme, SymDef, Unique, Writer, limits,
};
use crate::object::{Object, Secret, find_object, storage_cfb};
use crate::random::Drbg;
use crate::response::{Output, Rc};
use crate::state::Tpm;

const IDENTITY_LABEL: &[u8] = b"IDENTITY\0";
pub(crate) const DUPLICATE_LABEL: &[u8] = b"DUPLICATE\0";

pub(crate) enum SeedError {
    NoPrivate,
    KeyType,
    Ciphertext,
    Point,
}

impl SeedError {
    pub(crate) fn response(self, handle: u8, param: u8) -> Rc {
        match self {
            Self::NoPrivate => Rc::AUTH_UNAVAILABLE,
            Self::KeyType => Rc::TYPE.in_handle(handle),
            Self::Ciphertext => Rc::VALUE.in_param(param),
            Self::Point => Rc::ECC_POINT.in_param(param),
        }
    }
}

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
        let symmetric = check_storage_key(key, 1)?;
        let (seed, secret) =
            make_seed(&key.public.unique, IDENTITY_LABEL, &mut self.drbg).ok_or(Rc::FAILURE)?;
        let blob = protect(&seed[..], symmetric, object_name, credential);
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
        let symmetric = check_storage_key(key, 2)?;
        let mut id = Reader::new(blob);
        id.set_param(1);
        let tag = id.tpm2b(DIGEST)?;
        let encrypted = id.bytes(id.remaining())?;
        let seed = recover_seed(key, secret, IDENTITY_LABEL, &mut self.drbg)
            .map_err(|error| error.response(2, 2))?;
        let clear = open_seed_blob(&seed, symmetric, &activated.name, tag, encrypted)
            .ok_or(Rc::INTEGRITY.in_param(1))?;
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

pub(crate) fn check_storage_key(key: &Object, position: u8) -> Result<SymDef, Rc> {
    let (symmetric, scheme) = key
        .public
        .storage_parameters()
        .ok_or(Rc::TYPE.in_handle(position))?;
    // LoadExternal admits public keys for several uses. A Storage key
    // requires a NULL asymmetric scheme and a supported symmetric cipher.
    if scheme != Scheme::NULL {
        return Err(Rc::VALUE.in_handle(position));
    }
    if symmetric == SymDef::Null {
        return Err(Rc::TYPE.in_handle(position));
    }
    Ok(symmetric)
}

pub(crate) fn recover_seed(
    key: &Object,
    encrypted: &[u8],
    label: &[u8],
    rng: &mut Drbg,
) -> Result<Zeroizing<Vec<u8>>, SeedError> {
    let sensitive = key.sensitive.as_ref().ok_or(SeedError::NoPrivate)?;
    match (&sensitive.secret, &key.public.unique) {
        (Secret::Rsa(rsa), Unique::Rsa(modulus)) => {
            if encrypted.len() != modulus.len() {
                return Err(SeedError::Ciphertext);
            }
            rsa_decrypt_private(rsa, RsaPadding::Oaep, label, encrypted, rng)
                .filter(|seed| seed.len() <= DIGEST)
                .ok_or(SeedError::Ciphertext)
        }
        (Secret::Ecc(ecc), Unique::Ecc { x: receiver_x, .. }) => {
            let mut point = Reader::new(encrypted);
            let x = point.tpm2b(MAX_ECC_BYTES).map_err(|_| SeedError::Point)?;
            let y = point.tpm2b(MAX_ECC_BYTES).map_err(|_| SeedError::Point)?;
            point.finish().map_err(|_| SeedError::Point)?;
            if x.len() != MAX_ECC_BYTES || y.len() != MAX_ECC_BYTES {
                return Err(SeedError::Point);
            }
            let (shared_x, _) = ecc_ecdh(ecc, x, y).ok_or(SeedError::Point)?;
            Ok(Zeroizing::new(
                kdfe(&shared_x, label, x, receiver_x).to_vec(),
            ))
        }
        _ => Err(SeedError::KeyType),
    }
}

/// Seed and encrypted secret, using the storage key's supported Name hash (SHA-256).
pub(crate) fn make_seed(
    public: &Unique,
    label: &[u8],
    rng: &mut Drbg,
) -> Option<(Zeroizing<[u8; DIGEST]>, Vec<u8>)> {
    match public {
        Unique::Rsa(modulus) => {
            let mut seed = Zeroizing::new([0u8; DIGEST]);
            rng.generate(&mut seed[..])?;
            let cipher = rsa_encrypt_public(modulus, RsaPadding::Oaep, label, &seed[..], rng)?;
            Some((seed, cipher))
        }
        Unique::Ecc { x, y } => {
            let ephemeral = ecc_generate_extra_random_bits(rng)?;
            let (ephemeral_x, ephemeral_y) = ecc_public(&ephemeral)?;
            let (shared_x, _) = ecc_ecdh(&ephemeral, x, y)?;
            let seed = kdfe(&shared_x, label, &ephemeral_x, x);
            let mut point = Writer::new();
            point.tpm2b(&ephemeral_x);
            point.tpm2b(&ephemeral_y);
            Some((seed, point.finish()))
        }
        _ => None,
    }
}

/// Single SHA-256 block of KDFe, with the ephemeral point as party U.
fn kdfe(
    shared_x: &[u8],
    label: &[u8],
    ephemeral_x: &[u8],
    receiver_x: &[u8],
) -> Zeroizing<[u8; DIGEST]> {
    let mut hash = Sha256::new();
    hash.update(1u32.to_be_bytes());
    hash.update(shared_x);
    hash.update(label);
    hash.update(ephemeral_x);
    hash.update(receiver_x);
    Zeroizing::new(hash.finalize().into())
}

fn protect(seed: &[u8], symmetric: SymDef, name: &[u8], credential: &[u8]) -> Vec<u8> {
    let mut clear = Zeroizing::new(Vec::with_capacity(2 + credential.len()));
    clear.extend_from_slice(&(credential.len() as u16).to_be_bytes());
    clear.extend_from_slice(credential);
    protect_seed_blob(seed, symmetric, name, &clear)
}

pub(crate) fn protect_seed_blob(
    seed: &[u8],
    symmetric: SymDef,
    name: &[u8],
    clear: &[u8],
) -> Vec<u8> {
    let encrypted = storage_cfb(seed, symmetric, name, &[0; 16], clear, true);
    let integrity = hmac_sha256(&integrity_key(seed), &[&encrypted, name]);
    let mut blob = Writer::new();
    blob.tpm2b(&integrity);
    blob.bytes(&encrypted);
    blob.finish()
}

pub(crate) fn self_test() -> bool {
    // Independent Python SHA-256 vector for the credential KDFe transcript.
    *kdfe(&[1; 32], IDENTITY_LABEL, &[2; 32], &[3; 32])
        == [
            0x20, 0x73, 0xa3, 0x30, 0x6e, 0xed, 0xcf, 0xb1, 0x78, 0x18, 0x3d, 0x8a, 0x72, 0x1f,
            0x94, 0x9b, 0xba, 0x9c, 0x3d, 0x45, 0x01, 0xb6, 0xc2, 0x83, 0x94, 0x87, 0xbb, 0xdd,
            0x3b, 0x97, 0x30, 0x70,
        ]
}

fn integrity_key(seed: &[u8]) -> Zeroizing<Vec<u8>> {
    kdfa_sha256(seed, b"INTEGRITY", &[], &[], (DIGEST * 8) as u32)
}

pub(crate) fn open_seed_blob(
    seed: &[u8],
    symmetric: SymDef,
    name: &[u8],
    tag: &[u8],
    encrypted: &[u8],
) -> Option<Zeroizing<Vec<u8>>> {
    hmac_sha256_verify(&integrity_key(seed), &[encrypted, name], tag)
        .then(|| storage_cfb(seed, symmetric, name, &[0; 16], encrypted, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_kdfe_and_wrapping_match_external_vectors() {
        // Python hashlib/HMAC and OpenSSL AES-CFB with an all-zero IV.
        let mut name = [2u8; 34];
        name[..2].copy_from_slice(&[0, 0x0b]);
        assert_eq!(
            protect(&[1; 32], SymDef::Aes128Cfb, &name, &[3; 32]),
            [
                0x00, 0x20, 0x89, 0x46, 0xb6, 0x7b, 0x4c, 0xa4, 0x60, 0xb3, 0x6c, 0xa8, 0xd6, 0x9b,
                0x3e, 0xec, 0x7a, 0x89, 0xb6, 0xaf, 0x4d, 0x25, 0x33, 0xe6, 0xb2, 0x6e, 0x6e, 0x2b,
                0x98, 0x6c, 0x52, 0x33, 0x16, 0xaf, 0xe0, 0x19, 0x6e, 0x1c, 0x3b, 0x03, 0x9d, 0x55,
                0x07, 0x2e, 0x43, 0x2c, 0x21, 0x68, 0x6d, 0xa1, 0xf5, 0x62, 0x54, 0xb1, 0x54, 0x06,
                0x81, 0xc3, 0xc1, 0xf3, 0xec, 0x67, 0xcb, 0xec, 0x35, 0x37, 0x53, 0x0b,
            ]
        );
        assert_eq!(
            protect(&[1; 32], SymDef::Aes256Cfb, &name, &[3; 32]),
            [
                0x00, 0x20, 0x97, 0x42, 0xcf, 0x51, 0xbf, 0xce, 0x05, 0x50, 0xe4, 0x09, 0xd4, 0xda,
                0xb4, 0x75, 0x64, 0xc3, 0x13, 0x7b, 0xdd, 0x4b, 0x5f, 0x54, 0xd9, 0x3f, 0x8a, 0xf0,
                0x75, 0xfa, 0xdf, 0xec, 0xda, 0xa6, 0xc4, 0xc3, 0x8f, 0x9f, 0x07, 0x46, 0x54, 0x7a,
                0x8a, 0x10, 0x11, 0x89, 0x18, 0xa8, 0x0f, 0x5d, 0xde, 0x80, 0x99, 0x23, 0x11, 0x05,
                0xb1, 0xeb, 0x91, 0x63, 0x92, 0xe1, 0xb4, 0x00, 0x2f, 0x8f, 0x78, 0xd3,
            ]
        );
        assert!(self_test());
    }
}
