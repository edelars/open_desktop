//! Importing a TPM duplication blob (TPM Library v185 Part 1 §20.3.2, Part 3 §13.3).

use sha2::{Digest, Sha256};

use crate::codes::object_attr;
use crate::credential::{DUPLICATE_LABEL, check_storage_key, open_seed_blob, recover_seed};
use crate::crypto::DIGEST;
use crate::dispatch::Invocation;
use crate::marshal::{MAX_DATA, MAX_RSA_BYTES, Reader, SymDef, TpmtPublic, Writer};
use crate::object::{
    MAX_PRIVATE, aes128_cfb, check_public, find_object, name_of, private_blob,
    sensitive_from_duplicate,
};
use crate::response::{Output, Rc};
use crate::state::Tpm;

fn integrity_and_body(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut reader = Reader::new(bytes);
    let integrity = reader.tpm2b(DIGEST).ok()?;
    let body = reader.bytes(reader.remaining()).ok()?;
    (!body.is_empty()).then_some((integrity, body))
}

fn inner_integrity_matches(tag: &[u8], sensitive: &[u8], name: &[u8]) -> bool {
    if tag.len() != DIGEST {
        return false;
    }
    let mut hash = Sha256::new();
    hash.update(sensitive);
    hash.update(name);
    tag.iter()
        .zip(hash.finalize())
        .fold(0u8, |diff, (actual, expected)| diff | (actual ^ expected))
        == 0
}

impl Tpm {
    pub(crate) fn cmd_import(
        &mut self,
        inv: &Invocation,
        reader: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        let parent = find_object(&self.objects, &self.persistent.objects, inv.handles[0])
            .ok_or(Rc::reference_handle(0))?;
        check_storage_key(parent, 1)?;
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
        let required_key_size = match symmetric {
            SymDef::Null => 0,
            SymDef::Aes128Cfb => 16,
        };
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
            outer =
                open_seed_blob(&seed, &name, tag, ciphertext).ok_or(Rc::INTEGRITY.in_param(3))?;
            &outer[..]
        };
        let inner;
        let canonical = if symmetric == SymDef::Aes128Cfb {
            inner = aes128_cfb(key, &[0; 16], wrapped, false);
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
