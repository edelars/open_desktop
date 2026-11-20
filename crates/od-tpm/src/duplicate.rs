//! TPM duplication wrappers (TPM Library v185 Part 1 §20.3.2, Part 3 §§13.1 and 13.3).

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::codes::{alg, handle, object_attr};
use crate::credential::{
    DUPLICATE_LABEL, check_storage_key, make_seed, open_seed_blob, protect_seed_blob, recover_seed,
};
use crate::crypto::DIGEST;
use crate::dispatch::Invocation;
use crate::marshal::{MAX_DATA, MAX_RSA_BYTES, Reader, SymDef, TpmtPublic, Writer};
use crate::object::{
    MAX_PRIVATE, aes_cfb, check_public, find_object, name_of, private_blob,
    sensitive_from_duplicate, sensitive_to_duplicate,
};
use crate::response::{Output, Rc};
use crate::state::Tpm;

fn integrity_and_body(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut reader = Reader::new(bytes);
    let integrity = reader.tpm2b(DIGEST).ok()?;
    let body = reader.bytes(reader.remaining()).ok()?;
    (!body.is_empty()).then_some((integrity, body))
}

fn inner_integrity(sensitive: &[u8], name: &[u8]) -> [u8; DIGEST] {
    let mut hash = Sha256::new();
    hash.update(sensitive);
    hash.update(name);
    hash.finalize().into()
}

fn inner_integrity_matches(tag: &[u8], sensitive: &[u8], name: &[u8]) -> bool {
    if tag.len() != DIGEST {
        return false;
    }
    tag.iter()
        .zip(inner_integrity(sensitive, name))
        .fold(0u8, |diff, (actual, expected)| diff | (actual ^ expected))
        == 0
}

impl Tpm {
    pub(crate) fn cmd_duplicate(
        &mut self,
        inv: &Invocation,
        reader: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        reader.set_param(1);
        let key_in = reader.tpm2b(MAX_DATA)?;
        reader.set_param(2);
        let symmetric = SymDef::unmarshal(reader)?;
        reader.finish()?;

        let object = find_object(&self.objects, &self.persistent.objects, inv.handles[0])
            .ok_or(Rc::reference_handle(0))?;
        if object.has(object_attr::FIXED_PARENT) {
            return Err(Rc::ATTRIBUTES.in_handle(1));
        }
        if object.public.name_alg == alg::NULL {
            return Err(Rc::TYPE.in_handle(1));
        }
        let sensitive = object.sensitive.as_ref().ok_or(Rc::AUTH_UNAVAILABLE)?;
        let new_parent = if inv.handles[1] == handle::RH_NULL {
            None
        } else {
            let parent = find_object(&self.objects, &self.persistent.objects, inv.handles[1])
                .ok_or(Rc::reference_handle(1))?;
            let symmetric = check_storage_key(parent, 2)?;
            Some((parent, symmetric))
        };
        if object.has(object_attr::ENCRYPTED_DUPLICATION) {
            if symmetric == SymDef::Null {
                return Err(Rc::SYMMETRIC.in_param(2));
            }
            if new_parent.is_none() {
                return Err(Rc::HIERARCHY.in_handle(2));
            }
        }
        let key_size = usize::from(symmetric.key_bits() / 8);
        if !key_in.is_empty() && key_in.len() != key_size {
            return Err(Rc::SIZE.in_param(1));
        }

        let canonical = sensitive_to_duplicate(sensitive, object.public.type_alg())?;
        let mut generated = Zeroizing::new([0; 32]);
        let mut key_out = &[][..];
        let inner;
        let wrapped = if key_size != 0 {
            let key = if key_in.is_empty() {
                self.drbg
                    .generate(&mut generated[..key_size])
                    .ok_or(Rc::FAILURE)?;
                key_out = &generated[..key_size];
                key_out
            } else {
                key_in
            };
            // Reserve the full length so a reallocation cannot leave a copy of the sensitive area.
            let mut clear = Zeroizing::new(Vec::with_capacity(2 + DIGEST + canonical.len()));
            clear.extend_from_slice(&(DIGEST as u16).to_be_bytes());
            clear.extend_from_slice(&inner_integrity(&canonical, &object.name));
            clear.extend_from_slice(&canonical);
            inner = aes_cfb(key, &[0; 16], &clear, true);
            &inner[..]
        } else {
            &canonical[..]
        };
        let encrypted;
        let enc_seed;
        let (duplicate, secret): (&[u8], &[u8]) = if let Some((new_parent, outer_sym)) = new_parent
        {
            let (seed, secret) =
                make_seed(&new_parent.public.unique, DUPLICATE_LABEL, &mut self.drbg)
                    .ok_or(Rc::FAILURE)?;
            enc_seed = secret;
            encrypted = protect_seed_blob(&seed[..], outer_sym, &object.name, wrapped);
            (&encrypted, &enc_seed)
        } else {
            (wrapped, &[])
        };
        let mut params = Writer {
            buf: Vec::with_capacity(6 + key_out.len() + duplicate.len() + secret.len()),
        };
        params.tpm2b(key_out);
        params.tpm2b(duplicate);
        params.tpm2b(secret);
        Ok(Output {
            handles: Vec::new(),
            params: params.finish(),
        })
    }

    pub(crate) fn cmd_import(
        &mut self,
        inv: &Invocation,
        reader: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        let parent = find_object(&self.objects, &self.persistent.objects, inv.handles[0])
            .ok_or(Rc::reference_handle(0))?;
        let parent_symmetric = check_storage_key(parent, 1)?;
        let parent_sensitive = parent.sensitive.as_ref().ok_or(Rc::AUTH_UNAVAILABLE)?;

        reader.set_param(1);
        let key = reader.tpm2b(MAX_DATA)?;
        reader.set_param(2);
        let public = TpmtPublic::unmarshal_sized(reader)?;
        reader.set_param(3);
        let duplicate = reader.tpm2b(MAX_PRIVATE)?;
        reader.set_param(4);
        let encrypted_seed = reader.tpm2b(MAX_RSA_BYTES)?;
        reader.set_param(5);
        let symmetric = SymDef::unmarshal(reader)?;
        reader.finish()?;

        let fixed = object_attr::FIXED_TPM | object_attr::FIXED_PARENT;
        let encrypted_duplication = object_attr::ENCRYPTED_DUPLICATION;
        if public.attributes & fixed != 0
            || (public.attributes & encrypted_duplication != 0
                && (key.is_empty() || encrypted_seed.is_empty()))
        {
            return Err(Rc::ATTRIBUTES.in_param(2));
        }
        check_public(
            &public,
            parent.has(object_attr::FIXED_TPM),
            parent.has(encrypted_duplication),
        )?;
        let required_key_size = usize::from(symmetric.key_bits() / 8);
        if key.len() != required_key_size {
            return Err(Rc::SIZE.in_param(1));
        }

        let name = name_of(&public);
        let outer;
        let wrapped = if encrypted_seed.is_empty() {
            duplicate
        } else {
            let seed = recover_seed(parent, encrypted_seed, DUPLICATE_LABEL, &mut self.drbg)
                .map_err(|error| error.response(1, 4))?;
            let (tag, ciphertext) = integrity_and_body(duplicate).ok_or(Rc::SIZE.in_param(3))?;
            outer = open_seed_blob(&seed, parent_symmetric, &name, tag, ciphertext)
                .ok_or(Rc::INTEGRITY.in_param(3))?;
            &outer[..]
        };
        let inner;
        let canonical = if required_key_size != 0 {
            inner = aes_cfb(key, &[0; 16], wrapped, false);
            let (tag, sensitive) = integrity_and_body(&inner).ok_or(Rc::INTEGRITY.in_param(3))?;
            if !inner_integrity_matches(tag, sensitive, &name) {
                return Err(Rc::INTEGRITY.in_param(3));
            }
            sensitive
        } else {
            wrapped
        };
        let sensitive = sensitive_from_duplicate(canonical, &public).map_err(|error| {
            if error == Rc::SENSITIVE {
                error
            } else {
                error.in_param(3)
            }
        })?;
        let mut iv = [0; 16];
        self.drbg.generate(&mut iv).ok_or(Rc::FAILURE)?;
        let blob = private_blob(
            &public,
            &name,
            &sensitive,
            &sensitive.auth,
            &parent_sensitive.seed,
            parent_symmetric,
            &iv,
        )?;
        let mut params = Writer::new();
        params.tpm2b(&blob);
        Ok(Output {
            handles: Vec::new(),
            params: params.finish(),
        })
    }
}
