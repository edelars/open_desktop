//! ML-KEM-768 primitive and the KEM commands (TPM Library v185 §§14.10–14.11).

use ml_kem::ml_kem_768::{DecapsulationKey, EncapsulationKey};
use ml_kem::{B32, Decapsulate, KeyExport, Seed, TryKeyInit};
use zeroize::Zeroizing;

use crate::codes::object_attr;
use crate::dispatch::Invocation;
use crate::marshal::{Reader, Unique, Writer};
use crate::object::Secret;
use crate::random::Drbg;
use crate::response::{Output, Rc};
use crate::state::Tpm;

/// The ML-KEM-768 ciphertext (`TPM2B_KEM_CIPHERTEXT` contents; FIPS 203, Table 3).
pub(crate) const CIPHERTEXT_BYTES: usize = 1088;

pub(crate) type KemPrivate = DecapsulationKey;

pub(crate) fn generated_key(drbg: &mut Drbg) -> Result<KemPrivate, Rc> {
    let mut seed = Zeroizing::new(Seed::default());
    drbg.generate(seed.as_mut_slice()).ok_or(Rc::FAILURE)?;
    Ok(KemPrivate::from_seed(*seed))
}

pub(crate) fn private_from_seed(bytes: &[u8]) -> Result<KemPrivate, Rc> {
    let seed = Zeroizing::new(Seed::try_from(bytes).map_err(|_| Rc::SIZE)?);
    Ok(KemPrivate::from_seed(*seed))
}

pub(crate) fn private_seed(key: &KemPrivate) -> Result<Zeroizing<Vec<u8>>, Rc> {
    let seed = Zeroizing::new(key.to_seed().ok_or(Rc::FAILURE)?);
    Ok(Zeroizing::new(seed.as_slice().to_vec()))
}

pub(crate) fn public_bytes(key: &KemPrivate) -> Vec<u8> {
    key.encapsulation_key().to_bytes().as_slice().to_vec()
}

pub(crate) fn public_encoding_is_valid(bytes: &[u8]) -> bool {
    EncapsulationKey::new_from_slice(bytes).is_ok()
}

/// Encapsulation with a public key (`ek`, 1184 octets) and 32 octets of DRBG randomness: the shared
/// secret K (32 octets) and the ciphertext. `None` for an invalid public key or an exhausted DRBG.
pub(crate) fn encapsulate_public(
    public: &[u8],
    drbg: &mut Drbg,
) -> Option<(Zeroizing<Vec<u8>>, Vec<u8>)> {
    let key = EncapsulationKey::new_from_slice(public).ok()?;
    let mut m = Zeroizing::new(B32::default());
    drbg.generate(m.as_mut_slice())?;
    let (ciphertext, shared) = key.encapsulate_deterministic(&m);
    let shared = Zeroizing::new(shared);
    Some((
        Zeroizing::new(shared.as_slice().to_vec()),
        ciphertext.as_slice().to_vec(),
    ))
}

/// Decapsulation of a ciphertext of exactly [`CIPHERTEXT_BYTES`] octets; a modified ciphertext
/// gives the implicit-rejection secret of FIPS 203. `None` for another length.
pub(crate) fn decapsulate_ciphertext(
    key: &KemPrivate,
    ciphertext: &[u8],
) -> Option<Zeroizing<Vec<u8>>> {
    let shared = Zeroizing::new(key.decapsulate_slice(ciphertext).ok()?);
    Some(Zeroizing::new(shared.as_slice().to_vec()))
}

/// The seed of the ML-KEM Labeled KEM (Part 1 v185, clause 47.4, Equation 67): `KDFa(hashAlg, K,
/// label, ciphertext, publicKey, bits)` with `hashAlg` the `nameAlg` of the recipient key, K the
/// ML-KEM shared secret, the label of the protocol without its terminating zero (KDFa adds it),
/// `bits` the size of one digest of `hashAlg`, and only the contents of the two buffers as context.
pub(crate) fn labeled_seed(
    name_hash: crate::crypto::HashAlg,
    shared: &[u8],
    label: &[u8],
    ciphertext: &[u8],
    public: &[u8],
) -> Zeroizing<Vec<u8>> {
    crate::crypto::kdfa(
        name_hash,
        shared,
        label,
        ciphertext,
        public,
        (name_hash.size() * 8) as u32,
    )
}

impl Tpm {
    pub(crate) fn cmd_encapsulate(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.finish()?;
        let object = self.object(inv.handles[0]).ok_or(Rc::reference_handle(0))?;
        let Unique::MlKem(public) = &object.public.unique else {
            return Err(Rc::KEY.in_handle(1));
        };
        let key = EncapsulationKey::new_from_slice(public).map_err(|_| Rc::KEY.in_handle(1))?;
        let mut m = Zeroizing::new(B32::default());
        self.drbg.generate(m.as_mut_slice()).ok_or(Rc::FAILURE)?;
        let (ciphertext, shared) = key.encapsulate_deterministic(&m);
        let shared = Zeroizing::new(shared);
        let mut w = Writer::new();
        w.tpm2b(shared.as_slice());
        w.tpm2b(ciphertext.as_slice());
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }

    pub(crate) fn cmd_decapsulate(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let ciphertext = p.tpm2b(CIPHERTEXT_BYTES)?;
        p.finish()?;
        let object = self.object(inv.handles[0]).ok_or(Rc::reference_handle(0))?;
        if !matches!(object.public.unique, Unique::MlKem(_)) {
            return Err(Rc::KEY.in_handle(1));
        }
        if object.has(object_attr::RESTRICTED) || !object.has(object_attr::DECRYPT) {
            return Err(Rc::ATTRIBUTES.in_handle(1));
        }
        let Some(Secret::MlKem(key)) = object.sensitive.as_ref().map(|sensitive| &sensitive.secret)
        else {
            return Err(Rc::AUTH_UNAVAILABLE);
        };
        let shared = key
            .decapsulate_slice(ciphertext)
            .map_err(|_| Rc::SIZE.in_param(1))?;
        let shared = Zeroizing::new(shared);
        let mut w = Writer::new();
        w.tpm2b(shared.as_slice());
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }
}
