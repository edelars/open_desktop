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

const CIPHERTEXT_BYTES: usize = 1088;

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
