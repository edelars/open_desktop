//! TPM credential protection and seed sharing (Library v185 Part 1 §§21.3–5, 43.10, 44.7 and 47.4).
//!
//! The labeled KEM of an ML-KEM-768 Storage key (clause 47.4, Equation 67) is `seed = KDFa(nameAlg,
//! K, label, ciphertext, publicKey, digest bits)`, K being the 32-octet ML-KEM shared secret and the
//! ciphertext the whole `TPM2B_ENCRYPTED_SECRET`. Labels (Part 1, Table 5, checked against the v185
//! text): "IDENTITY" for credentials, "SECRET" for the salt of a session, "DUPLICATE" for duplication.
//! NOT VERIFIED: that clause 47.4 calls K "the 64-byte shared secret"; FIPS 203 and this code use 32.

use zeroize::Zeroizing;

use crate::crypto::{
    DIGEST, HashAlg, RsaPadding, ecc_coordinate_bytes, ecc_ecdh, ecc_generate_extra_random_bits,
    ecc_public, hash, hmac, hmac_verify, kdfa, rsa_decrypt_private, rsa_encrypt_public,
};
use crate::dispatch::Invocation;
use crate::kem;
use crate::marshal::{
    MAX_NAME, MAX_RSA_BYTES, PublicParams, Reader, Scheme, SymDef, TpmtPublic, Unique, Writer,
    limits,
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
        let key = find_object(&self.objects, &self.persistent.objects, inv.handles[0])
            .ok_or(Rc::reference_handle(0))?;
        // The credential is at most one digest of the `nameAlg` of the key (Part 3, clause 12.6:
        // `TPM_RC_SIZE`); a key whose `nameAlg` is no hash is refused below.
        let limit =
            HashAlg::from_alg(key.public.name_alg).map_or(limits::MAX_DIGEST_SIZE, HashAlg::size);
        reader.set_param(1);
        let credential = reader.tpm2b(limit)?;
        reader.set_param(2);
        // `TPM2B_NAME`: `sizeof(TPMU_NAME)`, a SHA-512 Name included.
        let object_name = reader.tpm2b(MAX_NAME)?;
        reader.finish()?;

        let (symmetric, name_hash) = check_sharing_key(key, 1)?;
        let (seed, secret) =
            make_seed(&key.public, IDENTITY_LABEL, &mut self.drbg, name_hash).ok_or(Rc::FAILURE)?;
        let blob = protect(&seed[..], symmetric, name_hash, object_name, credential);
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
        let activated = find_object(&self.objects, &self.persistent.objects, inv.handles[0])
            .ok_or(Rc::reference_handle(0))?;
        let key = find_object(&self.objects, &self.persistent.objects, inv.handles[1])
            .ok_or(Rc::reference_handle(1))?;
        // The wrapper is made with the `nameAlg` of the key (Part 1, clause 24.4); the blob is at
        // most an integrity value and a credential of one digest of it. A key whose `nameAlg` is no
        // hash is refused below.
        let digest = HashAlg::from_alg(key.public.name_alg).map_or(DIGEST, HashAlg::size);
        reader.set_param(1);
        let blob = reader.tpm2b(4 + 2 * digest)?;
        reader.set_param(2);
        let secret = reader.tpm2b(encrypted_secret_limit(key))?;
        reader.finish()?;

        // The Name of the activated object is hashed into the wrapper as it is; an object with no
        // `nameAlg` has no Name to prove.
        if HashAlg::from_alg(activated.public.name_alg).is_none() {
            return Err(Rc::HASH.in_handle(1));
        }
        let (symmetric, name_hash) = check_sharing_key(key, 2)?;
        let mut id = Reader::new(blob);
        id.set_param(1);
        let tag = id.tpm2b(name_hash.size())?;
        let encrypted = id.bytes(id.remaining())?;
        let seed = recover_seed(key, secret, IDENTITY_LABEL, &mut self.drbg, name_hash)
            .map_err(|error| error.response(2, 2))?;
        let clear = open_seed_blob(&seed, symmetric, name_hash, &activated.name, tag, encrypted)
            .ok_or(Rc::INTEGRITY.in_param(1))?;
        let mut value = Reader::new(&clear);
        let credential = value
            .tpm2b(name_hash.size())
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

/// [`check_storage_key`] for a key that shares a secret (`MakeCredential`, `ActivateCredential`,
/// the new parent of `Duplicate`): a symmetric Storage key shares none, the seed travels under an
/// asymmetric key (Part 3 v185, clause 13.1: `newParentHandle` "shall reference the public area of
/// an asymmetric key"). NOT VERIFIED: `TPM_RC_TYPE` for a SYMCIPHER key.
pub(crate) fn check_sharing_key(key: &Object, position: u8) -> Result<(SymDef, HashAlg), Rc> {
    let checked = check_storage_key(key, position)?;
    if matches!(key.public.params, PublicParams::SymCipher { .. }) {
        return Err(Rc::TYPE.in_handle(position));
    }
    Ok(checked)
}

/// The storage key of a seed wrapper: its symmetric cipher and the hash of its `nameAlg`, which is the
/// hash of the KDFa, the HMAC, the KDFe and the OAEP padding (Part 1, clauses 24.4 and 24.5).
pub(crate) fn check_storage_key(key: &Object, position: u8) -> Result<(SymDef, HashAlg), Rc> {
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
    let name_hash = HashAlg::from_alg(key.public.name_alg).ok_or(Rc::HASH.in_handle(position))?;
    Ok((symmetric, name_hash))
}

pub(crate) fn recover_seed(
    key: &Object,
    encrypted: &[u8],
    label: &[u8],
    rng: &mut Drbg,
    name_hash: HashAlg,
) -> Result<Zeroizing<Vec<u8>>, SeedError> {
    let sensitive = key.sensitive.as_ref().ok_or(SeedError::NoPrivate)?;
    match (&sensitive.secret, &key.public.unique) {
        (Secret::Rsa(rsa), Unique::Rsa(modulus)) => {
            if encrypted.len() != modulus.len() {
                return Err(SeedError::Ciphertext);
            }
            rsa_decrypt_private(rsa, RsaPadding::OaepWith(name_hash), label, encrypted, rng)
                .filter(|seed| seed.len() <= name_hash.size())
                .ok_or(SeedError::Ciphertext)
        }
        (Secret::Ecc(ecc), Unique::Ecc { x: receiver_x, .. }) => {
            let mut point = Reader::new(encrypted);
            // A coordinate has the size of the curve of the key (32 octets for P-256, 48 for P-384).
            let size = ecc_coordinate_bytes(ecc.curve()).ok_or(SeedError::Point)?;
            let x = point.tpm2b(size).map_err(|_| SeedError::Point)?;
            let y = point.tpm2b(size).map_err(|_| SeedError::Point)?;
            point.finish().map_err(|_| SeedError::Point)?;
            if x.len() != size || y.len() != size {
                return Err(SeedError::Point);
            }
            let (shared_x, _) = ecc_ecdh(ecc, x, y).ok_or(SeedError::Point)?;
            Ok(kdfe(name_hash, &shared_x, label, x, receiver_x))
        }
        (Secret::MlKem(kem_key), Unique::MlKem(public_key)) => {
            // The labeled KEM of Part 1 v185, clause 47.4: the ciphertext is the whole secret.
            if encrypted.len() != kem::CIPHERTEXT_BYTES {
                return Err(SeedError::Ciphertext);
            }
            let shared =
                kem::decapsulate_ciphertext(kem_key, encrypted).ok_or(SeedError::Ciphertext)?;
            Ok(kem::labeled_seed(
                name_hash,
                &shared,
                kdfa_label(label),
                encrypted,
                public_key,
            ))
        }
        _ => Err(SeedError::KeyType),
    }
}

/// The largest `TPM2B_ENCRYPTED_SECRET` a key opens: the ML-KEM-768 ciphertext, or the largest
/// RSA modulus (an ECC point is shorter).
pub(crate) fn encrypted_secret_limit(key: &Object) -> usize {
    if matches!(key.public.unique, Unique::MlKem(_)) {
        kem::CIPHERTEXT_BYTES
    } else {
        MAX_RSA_BYTES
    }
}

/// The label of a protocol for KDFa, which appends the zero octet itself.
fn kdfa_label(label: &[u8]) -> &[u8] {
    label.strip_suffix(&[0]).unwrap_or(label)
}

/// Seed (one digest of `name_hash`, the `nameAlg` of the storage key) and encrypted secret.
pub(crate) fn make_seed(
    public: &TpmtPublic,
    label: &[u8],
    rng: &mut Drbg,
    name_hash: HashAlg,
) -> Option<(Zeroizing<Vec<u8>>, Vec<u8>)> {
    match &public.unique {
        Unique::Rsa(modulus) => {
            let mut seed = Zeroizing::new(vec![0u8; name_hash.size()]);
            rng.generate(&mut seed[..])?;
            let padding = RsaPadding::OaepWith(name_hash);
            let cipher = rsa_encrypt_public(modulus, padding, label, &seed[..], rng)?;
            Some((seed, cipher))
        }
        Unique::Ecc { x, y } => {
            let PublicParams::Ecc { curve, .. } = &public.params else {
                return None;
            };
            let ephemeral = ecc_generate_extra_random_bits(rng, *curve)?;
            let (ephemeral_x, ephemeral_y) = ecc_public(&ephemeral)?;
            let (shared_x, _) = ecc_ecdh(&ephemeral, x, y)?;
            let seed = kdfe(name_hash, &shared_x, label, &ephemeral_x, x);
            let mut point = Writer::new();
            point.tpm2b(&ephemeral_x);
            point.tpm2b(&ephemeral_y);
            Some((seed, point.finish()))
        }
        Unique::MlKem(public_key) => {
            let (shared, ciphertext) = kem::encapsulate_public(public_key, rng)?;
            let seed = kem::labeled_seed(
                name_hash,
                &shared,
                kdfa_label(label),
                &ciphertext,
                public_key,
            );
            Some((seed, ciphertext))
        }
        _ => None,
    }
}

/// Single block of KDFe over `name_hash` (one digest, the size of the seed), with the ephemeral point
/// as party U.
fn kdfe(
    name_hash: HashAlg,
    shared_x: &[u8],
    label: &[u8],
    ephemeral_x: &[u8],
    receiver_x: &[u8],
) -> Zeroizing<Vec<u8>> {
    let block = hash(
        name_hash,
        &[
            &1u32.to_be_bytes(),
            shared_x,
            label,
            ephemeral_x,
            receiver_x,
        ],
    );
    Zeroizing::new(block.as_slice().to_vec())
}

fn protect(
    seed: &[u8],
    symmetric: SymDef,
    name_hash: HashAlg,
    name: &[u8],
    credential: &[u8],
) -> Vec<u8> {
    let mut clear = Zeroizing::new(Vec::with_capacity(2 + credential.len()));
    clear.extend_from_slice(&(credential.len() as u16).to_be_bytes());
    clear.extend_from_slice(credential);
    protect_seed_blob(seed, symmetric, name_hash, name, &clear)
}

/// `outerHMAC || CFB(clear)` of a credential or an outer duplication wrapper, keyed by KDFa and
/// HMAC of `name_hash` (the `nameAlg` of the storage key).
pub(crate) fn protect_seed_blob(
    seed: &[u8],
    symmetric: SymDef,
    name_hash: HashAlg,
    name: &[u8],
    clear: &[u8],
) -> Vec<u8> {
    let encrypted = storage_cfb(name_hash, seed, symmetric, name, &[0; 16], clear, true);
    let integrity = hmac(
        name_hash,
        &integrity_key(seed, name_hash),
        &[&encrypted, name],
    );
    let mut blob = Writer::new();
    blob.tpm2b(integrity.as_slice());
    blob.bytes(&encrypted);
    blob.finish()
}

pub(crate) fn self_test() -> bool {
    // Independent Python SHA-256 vector for the credential KDFe transcript.
    kdfe(
        HashAlg::Sha256,
        &[1; 32],
        IDENTITY_LABEL,
        &[2; 32],
        &[3; 32],
    )[..]
        == [
            0x20, 0x73, 0xa3, 0x30, 0x6e, 0xed, 0xcf, 0xb1, 0x78, 0x18, 0x3d, 0x8a, 0x72, 0x1f,
            0x94, 0x9b, 0xba, 0x9c, 0x3d, 0x45, 0x01, 0xb6, 0xc2, 0x83, 0x94, 0x87, 0xbb, 0xdd,
            0x3b, 0x97, 0x30, 0x70,
        ]
}

fn integrity_key(seed: &[u8], name_hash: HashAlg) -> Zeroizing<Vec<u8>> {
    kdfa(
        name_hash,
        seed,
        b"INTEGRITY",
        &[],
        &[],
        (name_hash.size() * 8) as u32,
    )
}

pub(crate) fn open_seed_blob(
    seed: &[u8],
    symmetric: SymDef,
    name_hash: HashAlg,
    name: &[u8],
    tag: &[u8],
    encrypted: &[u8],
) -> Option<Zeroizing<Vec<u8>>> {
    hmac_verify(
        name_hash,
        &integrity_key(seed, name_hash),
        &[encrypted, name],
        tag,
    )
    .then(|| storage_cfb(name_hash, seed, symmetric, name, &[0; 16], encrypted, false))
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
            protect(
                &[1; 32],
                SymDef::Aes128Cfb,
                HashAlg::Sha256,
                &name,
                &[3; 32]
            ),
            [
                0x00, 0x20, 0x89, 0x46, 0xb6, 0x7b, 0x4c, 0xa4, 0x60, 0xb3, 0x6c, 0xa8, 0xd6, 0x9b,
                0x3e, 0xec, 0x7a, 0x89, 0xb6, 0xaf, 0x4d, 0x25, 0x33, 0xe6, 0xb2, 0x6e, 0x6e, 0x2b,
                0x98, 0x6c, 0x52, 0x33, 0x16, 0xaf, 0xe0, 0x19, 0x6e, 0x1c, 0x3b, 0x03, 0x9d, 0x55,
                0x07, 0x2e, 0x43, 0x2c, 0x21, 0x68, 0x6d, 0xa1, 0xf5, 0x62, 0x54, 0xb1, 0x54, 0x06,
                0x81, 0xc3, 0xc1, 0xf3, 0xec, 0x67, 0xcb, 0xec, 0x35, 0x37, 0x53, 0x0b,
            ]
        );
        assert_eq!(
            protect(
                &[1; 32],
                SymDef::Aes256Cfb,
                HashAlg::Sha256,
                &name,
                &[3; 32]
            ),
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

    fn hx(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    }

    /// Python `hashlib`/`hmac` (KDFa, KDFe, the HMAC) and `openssl enc -aes-NNN-cfb` with an
    /// all-zero IV, for the hashes of a SHA-384 and a SHA-512 storage key (the same script gives the
    /// SHA-256 vectors above): seed, credential and Name octets repeat 1, 3 and 2.
    #[test]
    fn wrapping_and_kdfe_with_sha384_and_sha512_match_external_vectors() {
        for (name_hash, size, label_alg, aes128, aes256, kdfe_vector) in [
            (
                HashAlg::Sha384,
                48,
                0x000cu16,
                "00303aadf6b38d436b483137fe08deab01c0191173b1ff17b0486e02df87c0eaeda0ce82c3526af94b457bae072df27a71bf6d87b1262b6d1674edad0b488bff06d70083f253897a39145103275bda2b5b5c3a634516d2762f4b95b65b46f8e25b48d4f0",
                "0030ac472990f793d31ef0ec5c49c9aa5dbc183f2d095507fb91d8daa1ecc4475819c4b98b276adaa690947765c85c10aa74838b304d6c07bfacb320ac08cf2a61aadcfbbabd5b07bc8e92908ac8758aeb3ce6412b010c834baef33b2134407cf849b758",
                "df029d3d20b3c581fa63a5c4047aeeb55187278fc1d31ca69923206ee9247c208fcfc14eccda448e30acf88b02ca25b9",
            ),
            (
                HashAlg::Sha512,
                64,
                0x000du16,
                "0040baaa86e488921b15c7f4b452d672283f6545f96fa3327deefc8772884bf1563b2579621d9d35c95a89a57c8564e69ef7cc73dfb2295d3802b4436e96e89397511df80d5ff7fa6e6153d901cae5596dcd8996009070b014441b660108c4599e17eb034ef10537b46c487502ac450b2bd82729fa6fdf76e4483c72ca5ba55497f5db94",
                "004098773f454f784bbc6ab4571c466f4827d4a940d733241af5fde838fd348f7023825ea60a6c50d8fe3d3061e40253c32fbb70f52642c6dfe17c358c5a8c37b0d40cd79d1e88e85d3ca7e8baf30bf5463f86a6d6114f18e5966908782ee374ac2ce13b1707f513060c86ad6b843a0a31f648a52adacaf147543ce618689e4c903b20c4",
                "8d109dabdcda010696edc4efa9ff0fbe88d50fffac84f03e644eccb8e7e8ab82b71c6ee6079edcd5952beeb4542ae53a911ba067c5cef9369abedc87dee002ca",
            ),
        ] {
            let mut name = vec![2u8; 2 + size];
            name[..2].copy_from_slice(&label_alg.to_be_bytes());
            let (seed, credential) = (vec![1u8; size], vec![3u8; size]);
            assert_eq!(
                protect(&seed, SymDef::Aes128Cfb, name_hash, &name, &credential),
                hx(aes128)
            );
            assert_eq!(
                protect(&seed, SymDef::Aes256Cfb, name_hash, &name, &credential),
                hx(aes256)
            );
            // The wrapper opens with the same hash and with no other.
            let blob = hx(aes128);
            let (tag, encrypted) = (&blob[2..2 + size], &blob[2 + size..]);
            assert!(
                open_seed_blob(&seed, SymDef::Aes128Cfb, name_hash, &name, tag, encrypted)
                    .is_some()
            );
            for other in [HashAlg::Sha256, HashAlg::Sha384, HashAlg::Sha512] {
                if other != name_hash {
                    assert!(
                        open_seed_blob(&seed, SymDef::Aes128Cfb, other, &name, tag, encrypted)
                            .is_none()
                    );
                }
            }
            // KDFe: one block of the hash, so the seed is one digest long.
            let seed = kdfe(name_hash, &[1; 32], IDENTITY_LABEL, &[2; 32], &[3; 32]);
            assert_eq!(&seed[..], &hx(kdfe_vector)[..]);
            assert_eq!(seed.len(), size);
        }
    }

    /// OAEP with the hash of the storage key (Part 1, clause 24.4): ciphertexts made by `openssl
    /// pkeyutl -encrypt` with `rsa_oaep_md` and `rsa_mgf1_md` of SHA-384 and SHA-512 and the label
    /// `IDENTITY\0` open with the private key; the TPM-made ciphertexts (a fixed DRBG) were opened
    /// by `openssl pkeyutl -decrypt` with the same options when the test was made.
    #[test]
    fn rsa_oaep_with_sha384_and_sha512_matches_openssl() {
        use crate::crypto::{rsa_decrypt_private, rsa_encrypt_public, rsa_from_primes};
        let n = hx(
            "ae3451e2a5d2e73736a6c13474a86900a15b129437e69a4ef927fb7af5f2d0469351299f436e73c39a1c27a0aa88f2e8e3c3b764fefe6b3f15d4737ec7ba6de6a45d652a62d393c0ebddd10e0b5c9ce2af0a91dc198ba6eecf5864f17d3600a2be76b272e3bfc911cd922312e67f16c6ad74dfbe9e547e23ed4115a3bac9b635024b9a38b747bfc9e5e52193a14a34318900aae82e74b2c3a1facd9d2e062cae5973f9b7ae319703658fddb654666bc84b4c1c8ce60977105933d9bb466510d1df9eb7833eef6fcb6d5350fef430f4d516cc820ffd21695867cfa1bd321660d96105a8c14a53f9656931b8d1c631ba60d7f3f21c3e62db83abded544f91491f1",
        );
        let pq = [hx("ed0c2e2b53452856f41a9c9da199d45fb98187ea9b079920c3fb2de272a11d8d6b2640be8f46c6b1b415c58c1ca39717f2f340ec67161841e0e4f927245b2f3fb159ecca8f485335a17e3a05b36d573af0f60e80a27005f88e1b2e09d1c4d3ed4ec9c13cab7e47de20ac28ada8fcb3909f5e2abff5543975aaa8d5d7f5d32fef"), hx("bc21e22703c02f659103c8dee87c776eff7eddeed1e93d64c5aef3862e9ed437fbc52836d8a73f3872a91b9cc9a80346bb73a7d1a73ee3de1ee109b86ab6b9e5637f0f72398725b0504d3cd4899c848e312fe596d40a14c003f6538db84cd0a9e22557457858766859b747c976a77faf0b53a4a8346c6122e9507bd421567c1f")].concat();
        let key = rsa_from_primes(&pq, &n).unwrap();
        let mut rng = Drbg::new(&[3u8; 48]).unwrap();
        for (hash_alg, openssl_ct, message, tpm_ct) in [
            (
                HashAlg::Sha384,
                "93b5ce364750d3f8a986801f8264444ea6b7d85b49594a3ff545009aa40b3b362ccc94ce0c227b7ba6e7f4006114556c4d75c2d835513bacf0d86dee5579cf8dc6b28dda5767b2d3df7aebeb8bd459c3ccd78eb97ac9f5eaf5bfb5d3406fd981f5e3b6f20bd814dacdfe6e0fe6e665a5a81f5b0e66af7af36d184a1db56c886368a349100c1d2d4e0b6e6545bc9bce7fcd4ef7b80d903c8d1899e9d08e5a4ad27e33b7c10b8ea9b1559d7bd4e8e94a8054846924b349969350937845516c904f427a928f8e3e0f83b1714ad0119cd8ebb79d701a1e37140ce2044f161a7d42549669be6763e9c60dd7b93b9c06fd18e432dc1139d530d038d6317f6cef53ea64",
                "303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f",
                "1f268c3ed31f1e5f397784a8819520eee5d01765edfbf8b7f8c3b7a4d4cb0a290b1839229520aaa163d0522a95d0cdd8792f20fd94901022758844c22cc87f5ca01564ff1264ad1f52cb4a3d5055c1e8776ce126cd27d4ec31ba13bd2d3049c3db6bae6e7d2c0e2f93a62802438868b134ecc7f477b120b21149c57c51655aae83b2e22fdf842b310fab4aa2ac95b1c7c023bbe33889a1d81a62009dd85c7861663473fe119262bfe0762cc4f9bb2bf39ee6bf96d7abd9986f530ce5afaccbec78bbb8762fa392c6e38ab5e3281da67ce05a10b73de867c92894e3218cdf2899d8614be69b3f837ac6505ce2604406d27d8d45ead6b6aab9a01ee408e66cfee0",
            ),
            (
                HashAlg::Sha512,
                "75509c32a64ab161cd4a57292a63d78fc9c55abc6d835e829b70a6e73c1e8dd4bceb78b746c3400775629d8f8442ce6a4cef216df010a0f9c31cae3778501ce14601874a9bf16c2acb14ad59f23018c277cebd684f17dbf280db98ec5fef3b68ecdb2b653643d676a0a6c77fd5a35db0e5c5c860462e48df13130722a819992dd289b14ddd1ecf34ed347702ed7bfb281546242e13e2237c45b05b9e0411b564eb6825237ce3ec6cf5f17d0e0067d80d936ca1a9653e8f0a01068e127cdda35205ebc0ff1846d469f7c1f31d1c5712719e386988a32142605e0a00f47a9dce623595d5d4dfb45efb809b68e7df07efdcb9402dda723c70d63e08f3a05327ebd1",
                "303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f",
                "a9031503b0cabf627ee579f96c9436fe81a6e098fdf12aa46fdc2bf1b68e8ca4f5cd3936f54116d82c850f90395954969e38ce47384785ddad00543dbceac4baef60c7451bf821cd6bc22e8e239729ee272f96398ba44751efbe297ae0c3a13a1a24d74f6560c7b2c80dbaf6d0f20e4f8b16a96fbed78b0e5fe5fdcd5b74b77ca59825206f7c19b0581103b6990488779e17950a33847f9878ee282b8366b596d907b2be56cddfd0a502eff0d0a64c129f5add782b04c3c9855bbfb021bff766c898d8981776a29b7f1bdb4063ec78b52f2bb9fec5f1ef44b3c1d45ebb09616210e22af38c3862b2e11c7294a16cb3ba88784ff87598032b7136717085485920",
            ),
        ] {
            let message = hx(message);
            let padding = RsaPadding::OaepWith(hash_alg);
            let opened =
                rsa_decrypt_private(&key, padding, IDENTITY_LABEL, &hx(openssl_ct), &mut rng);
            assert_eq!(opened.as_deref().map(|m| m.as_slice()), Some(&message[..]));
            // Another hash (or SHA-256) does not open it.
            for other in [
                RsaPadding::Oaep,
                RsaPadding::OaepWith(HashAlg::Sha384),
                RsaPadding::OaepWith(HashAlg::Sha512),
            ] {
                if other != padding {
                    assert!(
                        rsa_decrypt_private(&key, other, IDENTITY_LABEL, &hx(openssl_ct), &mut rng)
                            .is_none()
                    );
                }
            }
            let mut fixed = Drbg::new(&[3u8; 48]).unwrap();
            let ct = rsa_encrypt_public(&n, padding, IDENTITY_LABEL, &message, &mut fixed).unwrap();
            assert_eq!(ct, hx(tpm_ct));
            let reopened =
                rsa_decrypt_private(&key, padding, IDENTITY_LABEL, &ct, &mut rng).unwrap();
            assert_eq!(&reopened[..], &message[..]);
        }
    }
}
