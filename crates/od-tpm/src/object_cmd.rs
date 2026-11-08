//! Object commands (slice B of phase B, S7-23): `TPM2_CreatePrimary`, `Create`, `Load`,
//! `LoadExternal`, `ReadPublic`, `Unseal`, `EvictControl`, `Sign`, `VerifySignature`. The model,
//! the template rules and the private-area protection are in `object.rs`.
//!
//! Authorization is checked by the dispatcher before a handler runs: the first handle of
//! `CreatePrimary` (a hierarchy), `Create`, `Load`, `Unseal`, `Sign` (the object, role USER) and
//! `EvictControl` (owner or platform) needs a password or HMAC session; `LoadExternal`,
//! `ReadPublic` and `VerifySignature` need none. The authValue of an object is available only if
//! it has a private part and `userWithAuth` is SET; with `userWithAuth` CLEAR only a policy
//! session authorizes (`TPM_RC_POLICY_FAIL` for a password or HMAC, Part 3, clause 5.6, step 7),
//! and without a private part nothing does (`TPM_RC_AUTH_UNAVAILABLE`); see `policy.rs`.
//!
//! Tickets (Part 2 v184, clauses 10.7.3 and 10.7.4): the creation ticket is
//! `HMAC(proof, TPM_ST_CREATION || name || H(creationData))` and the verified ticket
//! `HMAC(proof, TPM_ST_VERIFIED || digest || keyName)`, the proof being that of the hierarchy of
//! the object; the Null hierarchy gets the NULL ticket. The `validation` ticket of `TPM2_Sign` is
//! always a NULL ticket in this TPM (`TPM2_Hash` produces only NULL tickets, `hash.rs`), so a
//! restricted signing key cannot sign: `TPM_RC_TICKET`.
//!
//! NOT VERIFIED against TPM 2.0 Library v185/v184 (see also `object.rs`): the index modifiers of
//! the codes below (`TPM_RC_ATTRIBUTES`/`TPM_RC_KEY`/`TPM_RC_HIERARCHY` with a handle number,
//! `TPM_RC_SIZE`/`TPM_RC_SCHEME`/`TPM_RC_TICKET` with a parameter number); `TPM_RC_VALUE` for a
//! non-empty `inPrivate` of `TPM2_LoadExternal` (sensitive-area loading is not implemented);
//! `TPM_RC_TYPE` for a sealed-data object loaded without private part and for `TPM2_Unseal` of
//! an object of another type (Part 3 names it); `TPM_RC_KEY` for `TPM2_VerifySignature` with a
//! key that has `sign` CLEAR; `TPM_RC_SCHEME` for a signature of another type than the key;
//! `TPM_RC_SIZE` for a digest of the wrong length; `TPM_RC_HIERARCHY` for loading an external
//! object into a disabled hierarchy; the response order of `TPM2_CreatePrimary` (Part 3, Table
//! 175) is as in the open text.

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::codes::{alg, handle, object_attr, st};
use crate::crypto::{ecc_sign, ecc_verify, hmac_sha256, kdfa_sha256, rsa_sign, rsa_verify};
use crate::dispatch::Invocation;
use crate::hash::AuthValue;
use crate::marshal::{
    MAX_DATA, PublicParams, Reader, Scheme, TpmlPcrSelection, TpmsCreationData,
    TpmsSensitiveCreate, TpmtPublic, TpmtSignature, TpmtTicket, TpmtTkHashcheck, Unique, Writer,
    limits,
};
use crate::object::{
    MAX_PRIVATE, Object, Secret, check_creation, check_external_public, check_public,
    creation_pcr_digest, derive_object, find_object, is_object_hierarchy, open_private,
    primary_drbg, private_blob,
};
use crate::response::{Output, Rc};
use crate::state::Tpm;

/// `TPMA_LOCALITY` of locality 0 (Part 2, Table 41, bit 0).
const LOCALITY_ZERO: u8 = 1;

/// What a child needs from its parent, copied so that the parent is not borrowed while the TPM
/// DRBG is used.
struct ParentCtx {
    seed: Zeroizing<Vec<u8>>,
    name: Vec<u8>,
    qualified_name: Vec<u8>,
    hierarchy: u32,
    fixed_tpm: bool,
    enc_dup: bool,
}

impl Tpm {
    /// The loaded object a handle names (transient or persistent).
    pub(crate) fn object(&self, h: u32) -> Option<&Object> {
        find_object(&self.objects, &self.persistent.objects, h)
    }

    /// `TPMI_DH_OBJECT` validity of handle number `idx` (zero-based) of a command, before
    /// authorization: a loaded transient object or an existing persistent object. A loaded
    /// sequence object is `TPM_RC_SEQUENCE` (Part 3, clause 12.4.1), an unloaded transient
    /// handle `TPM_RC_REFERENCE_Hn`, a missing persistent handle `TPM_RC_HANDLE`, any other
    /// handle type `TPM_RC_VALUE`. A persistent object of a disabled hierarchy cannot be used
    /// (`TPM_RC_HIERARCHY`; Part 3, clause 24.2.1), unless `evicting`.
    pub(crate) fn check_object_handle(&self, h: u32, idx: usize, evicting: bool) -> Result<(), Rc> {
        let n = (idx + 1) as u8;
        let found = match handle::handle_type(h) {
            handle::HT_TRANSIENT => {
                if self.objects.get(h).is_none() {
                    if self.seq_slot(h, idx).is_ok() {
                        return Err(Rc::SEQUENCE);
                    }
                    return Err(Rc::reference_handle(idx));
                }
                self.objects.get(h)
            }
            handle::HT_PERSISTENT => match self.persistent.objects.get(h) {
                Some(o) => Some(o),
                None => return Err(Rc::HANDLE.in_handle(n)),
            },
            _ => return Err(Rc::VALUE.in_handle(n)),
        };
        if let Some(o) = found
            && !evicting
            && !self.permanent_enabled(o.hierarchy)
        {
            return Err(Rc::HIERARCHY.in_handle(n));
        }
        Ok(())
    }

    /// The authValue of an object used as an authorization handle (Part 3, clause 5.6, steps 1,
    /// 5 and 7): `TPM_RC_AUTH_UNAVAILABLE` if the object has no private part; for a password or
    /// HMAC session (`policy` false) `TPM_RC_POLICY_FAIL` when the USER role needs a policy
    /// (`userWithAuth` CLEAR) and `TPM_RC_AUTH_TYPE` when the ADMIN role does (`adminWithPolicy`
    /// SET); a policy session passes here and is checked against the `authPolicy` by the
    /// dispatcher.
    pub(crate) fn object_auth(
        &self,
        h: u32,
        idx: usize,
        admin: bool,
        policy: bool,
    ) -> Result<AuthValue, Rc> {
        let o = self.object(h).ok_or(Rc::reference_handle(idx))?;
        let s = o.sensitive.as_ref().ok_or(Rc::AUTH_UNAVAILABLE)?;
        if !policy {
            if admin {
                if o.has(object_attr::ADMIN_WITH_POLICY) {
                    return Err(Rc::AUTH_TYPE);
                }
            } else if !o.has(object_attr::USER_WITH_AUTH) {
                return Err(Rc::POLICY_FAIL.in_session((idx + 1) as u8));
            }
        }
        Ok(s.auth.clone())
    }

    /// The Name of the object behind a handle, for a cpHash.
    pub(crate) fn object_name(&self, h: u32) -> Option<Vec<u8>> {
        self.object(h).map(|o| o.name.clone())
    }

    /// The current authValue of an object, for a response HMAC (regardless of `userWithAuth`).
    pub(crate) fn object_auth_now(&self, h: u32) -> Option<AuthValue> {
        self.object(h)
            .and_then(|o| o.sensitive.as_ref().map(|s| s.auth.clone()))
    }

    /// The seed of a hierarchy (Primary Seed).
    fn hierarchy_seed(&self, hierarchy: u32) -> Zeroizing<Vec<u8>> {
        let seed: &[u8] = match hierarchy {
            handle::RH_PLATFORM => &self.persistent.platform_seed[..],
            handle::RH_OWNER => &self.persistent.owner_seed[..],
            handle::RH_ENDORSEMENT => &self.persistent.endorsement_seed[..],
            _ => &self.hv.null_seed[..],
        };
        Zeroizing::new(seed.to_vec())
    }

    /// The proof value of a hierarchy; `None` for the Null hierarchy (its tickets are NULL
    /// tickets). Part 3, clause 24.6 allows deriving proofs from the Primary Seeds ("if derived
    /// from the Primary Seeds, the derivation of ehProof shall use both the SPS and EPS"): here
    /// `proof = KDFa(SHA-256, seed, "ODTPM-<H>PROOF", NULL, NULL, 256)` and for the endorsement
    /// hierarchy the storage seed is the context, so `TPM2_Clear` (new SPS) changes `shProof`
    /// and `ehProof` and nothing needs to be stored. NOT VERIFIED: that this satisfies the
    /// "second HMAC with the EPS as key" shape of the clause (it uses KDFa/HMAC with the EPS as
    /// key and the SPS as context).
    pub(crate) fn proof(&self, hierarchy: u32) -> Option<Zeroizing<Vec<u8>>> {
        let p = &self.persistent;
        Some(match hierarchy {
            handle::RH_PLATFORM => {
                kdfa_sha256(&p.platform_seed[..], b"ODTPM-PHPROOF", &[], &[], 256)
            }
            handle::RH_OWNER => kdfa_sha256(&p.owner_seed[..], b"ODTPM-SHPROOF", &[], &[], 256),
            handle::RH_ENDORSEMENT => kdfa_sha256(
                &p.endorsement_seed[..],
                b"ODTPM-EHPROOF",
                &p.owner_seed[..],
                &[],
                256,
            ),
            _ => return None,
        })
    }

    /// A ticket with `parts` under the proof of `hierarchy`, or the NULL ticket for the Null
    /// hierarchy.
    fn ticket(&self, tag: u16, hierarchy: u32, parts: &[&[u8]]) -> TpmtTicket {
        match self.proof(hierarchy) {
            Some(proof) => TpmtTicket {
                tag,
                hierarchy,
                digest: hmac_sha256(&proof, parts).to_vec(),
            },
            None => TpmtTicket {
                tag,
                hierarchy: handle::RH_NULL,
                digest: Vec::new(),
            },
        }
    }

    fn parent_ctx(&self, h: u32) -> Result<ParentCtx, Rc> {
        let o = self.object(h).ok_or(Rc::reference_handle(0))?;
        // The parent must be a storage key with both parts loaded.
        let s = o
            .sensitive
            .as_ref()
            .filter(|_| o.is_storage())
            .ok_or(Rc::TYPE.in_handle(1))?;
        Ok(ParentCtx {
            seed: s.seed.clone(),
            name: o.name.clone(),
            qualified_name: o.qualified_name.clone(),
            hierarchy: o.hierarchy,
            fixed_tpm: o.has(object_attr::FIXED_TPM),
            enc_dup: o.has(object_attr::ENCRYPTED_DUPLICATION),
        })
    }

    /// `creationData` (marshaled `TPMS_CREATION_DATA`), `creationHash` and the creation ticket
    /// for a new object.
    fn creation_artifacts(
        &self,
        obj: &Object,
        selection: &TpmlPcrSelection,
        outside_info: &[u8],
        parent: (u16, &[u8], &[u8]),
    ) -> (Vec<u8>, Vec<u8>, TpmtTicket) {
        let data = TpmsCreationData {
            pcr_select: selection.clone(),
            pcr_digest: creation_pcr_digest(&self.pcrs, selection),
            locality: LOCALITY_ZERO,
            parent_name_alg: parent.0,
            parent_name: parent.1.to_vec(),
            parent_qualified_name: parent.2.to_vec(),
            outside_info: outside_info.to_vec(),
        };
        let mut w = Writer::new();
        data.marshal(&mut w);
        let bytes = w.finish();
        let hash = Sha256::digest(&bytes).to_vec();
        let ticket = self.ticket(
            st::CREATION,
            obj.hierarchy,
            &[&st::CREATION.to_be_bytes(), &obj.name, &hash],
        );
        (bytes, hash, ticket)
    }

    /// `TPM2_CreatePrimary` (Part 3, clause 24.1): creates and loads a primary object.
    pub(crate) fn cmd_create_primary(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let sens = TpmsSensitiveCreate::unmarshal_sized(p)?;
        p.set_param(2);
        let template = TpmtPublic::unmarshal_sized(p)?;
        p.set_param(3);
        let outside = p.tpm2b(MAX_DATA)?;
        p.set_param(4);
        let selection = TpmlPcrSelection::unmarshal(p)?;
        p.finish()?;
        let hierarchy = inv.handles[0];
        // A primary's parent is the hierarchy: fixedTPM, not encryptedDuplication.
        check_public(&template, true, false)?;
        check_creation(&template, &sens)?;
        if self.objects.free_slots() == 0 {
            return Err(Rc::OBJECT_MEMORY);
        }
        let seed = self.hierarchy_seed(hierarchy);
        let mut drbg = primary_drbg(&seed, &template, &sens.data)?;
        let d = derive_object(&template, &sens.user_auth, &sens.data, &mut drbg)?;
        let hierarchy_handle = hierarchy.to_be_bytes();
        let obj = Object::new(d.public, Some(d.sensitive), hierarchy, &hierarchy_handle);
        let (creation, creation_hash, ticket) = self.creation_artifacts(
            &obj,
            &selection,
            outside,
            (alg::NULL, &hierarchy_handle, &hierarchy_handle),
        );
        let mut params = Writer::new();
        obj.public.marshal_sized(&mut params);
        params.tpm2b(&creation);
        params.tpm2b(&creation_hash);
        ticket.marshal(&mut params);
        params.tpm2b(&obj.name);
        let h = self.objects.insert(obj).ok_or(Rc::OBJECT_MEMORY)?;
        let mut handles = Writer::new();
        handles.u32(h);
        Ok(Output {
            handles: handles.finish(),
            params: params.finish(),
        })
    }

    /// `TPM2_Create` (Part 3, clause 12.1): a new object under a storage parent, returned as a
    /// public area and an `outPrivate` blob (see the limitation in `object.rs`).
    pub(crate) fn cmd_create(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        let parent = self.parent_ctx(inv.handles[0])?;
        p.set_param(1);
        let sens = TpmsSensitiveCreate::unmarshal_sized(p)?;
        p.set_param(2);
        let template = TpmtPublic::unmarshal_sized(p)?;
        p.set_param(3);
        let outside = p.tpm2b(MAX_DATA)?;
        p.set_param(4);
        let selection = TpmlPcrSelection::unmarshal(p)?;
        p.finish()?;
        check_public(&template, parent.fixed_tpm, parent.enc_dup)?;
        check_creation(&template, &sens)?;
        let d = derive_object(&template, &sens.user_auth, &sens.data, &mut self.drbg)?;
        let mut iv = [0u8; 16];
        self.drbg.generate(&mut iv).ok_or(Rc::FAILURE)?;
        let obj = Object::new(
            d.public,
            Some(d.sensitive),
            parent.hierarchy,
            &parent.qualified_name,
        );
        let sensitive = obj.sensitive.as_ref().ok_or(Rc::FAILURE)?;
        let blob = private_blob(&obj.public, &obj.name, sensitive, &parent.seed, &iv)?;
        let (creation, creation_hash, ticket) = self.creation_artifacts(
            &obj,
            &selection,
            outside,
            (alg::SHA256, &parent.name, &parent.qualified_name),
        );
        let mut params = Writer::new();
        params.tpm2b(&blob);
        obj.public.marshal_sized(&mut params);
        params.tpm2b(&creation);
        params.tpm2b(&creation_hash);
        ticket.marshal(&mut params);
        Ok(Output {
            handles: Vec::new(),
            params: params.finish(),
        })
    }

    /// `TPM2_Load` (Part 3, clause 12.2): the integrity is checked before the decryption, then
    /// the private value is matched against the public area.
    pub(crate) fn cmd_load(&mut self, inv: &Invocation, p: &mut Reader<'_>) -> Result<Output, Rc> {
        let parent = self.parent_ctx(inv.handles[0])?;
        p.set_param(1);
        let blob = p.tpm2b(MAX_PRIVATE)?;
        if blob.is_empty() {
            return Err(Rc::SIZE.in_param(1));
        }
        p.set_param(2);
        let public = TpmtPublic::unmarshal_sized(p)?;
        p.finish()?;
        check_public(&public, parent.fixed_tpm, parent.enc_dup)?;
        if self.objects.free_slots() == 0 {
            return Err(Rc::OBJECT_MEMORY);
        }
        let sensitive = open_private(&public, &parent.seed, blob)?;
        let obj = Object::new(
            public,
            Some(sensitive),
            parent.hierarchy,
            &parent.qualified_name,
        );
        let mut params = Writer::new();
        params.tpm2b(&obj.name);
        let h = self.objects.insert(obj).ok_or(Rc::OBJECT_MEMORY)?;
        let mut handles = Writer::new();
        handles.u32(h);
        Ok(Output {
            handles: handles.finish(),
            params: params.finish(),
        })
    }

    /// `TPM2_LoadExternal` (Part 3, clause 12.3) for a public area only. A non-empty `inPrivate`
    /// (loading a sensitive area, the "crypto accelerator" use) is not implemented: `TPM_RC_VALUE`.
    pub(crate) fn cmd_load_external(&mut self, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let sensitive_size = p.u16()?;
        if sensitive_size != 0 {
            return Err(Rc::VALUE.in_param(1));
        }
        p.set_param(2);
        let public = TpmtPublic::unmarshal_sized(p)?;
        p.set_param(3);
        let hierarchy = p.u32()?;
        p.finish()?;
        if !is_object_hierarchy(hierarchy) {
            return Err(Rc::VALUE.in_param(3));
        }
        check_external_public(&public)?;
        if !self.permanent_enabled(hierarchy) {
            return Err(Rc::HIERARCHY.in_param(3));
        }
        if self.objects.free_slots() == 0 {
            return Err(Rc::OBJECT_MEMORY);
        }
        let obj = Object::new(public, None, hierarchy, &[]);
        let mut params = Writer::new();
        params.tpm2b(&obj.name);
        let h = self.objects.insert(obj).ok_or(Rc::OBJECT_MEMORY)?;
        let mut handles = Writer::new();
        handles.u32(h);
        Ok(Output {
            handles: handles.finish(),
            params: params.finish(),
        })
    }

    /// `TPM2_ReadPublic` (Part 3, clause 12.4): the public area, Name and Qualified Name; never
    /// anything of the sensitive area.
    pub(crate) fn cmd_read_public(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.finish()?;
        let o = self.object(inv.handles[0]).ok_or(Rc::reference_handle(0))?;
        let mut w = Writer::new();
        o.public.marshal_sized(&mut w);
        w.tpm2b(&o.name);
        w.tpm2b(&o.qualified_name);
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }

    /// `TPM2_Unseal` (Part 3, clause 12.7): the data of a sealed-data object.
    pub(crate) fn cmd_unseal(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.finish()?;
        let o = self.object(inv.handles[0]).ok_or(Rc::reference_handle(0))?;
        // Part 3, clause 12.7.1: ATTRIBUTES if restricted, decrypt or sign is SET; TYPE if the
        // object is not a keyed hash.
        if o.public.attributes
            & (object_attr::RESTRICTED | object_attr::DECRYPT | object_attr::SIGN)
            != 0
        {
            return Err(Rc::ATTRIBUTES.in_handle(1));
        }
        if o.public.type_alg() != alg::KEYEDHASH {
            return Err(Rc::TYPE.in_handle(1));
        }
        let Some(Secret::Data(data)) = o.sensitive.as_ref().map(|s| &s.secret) else {
            return Err(Rc::AUTH_UNAVAILABLE);
        };
        // `Output::params` is a plain buffer: the data of a sealed object leaves the TPM in it.
        let mut w = Writer {
            buf: Vec::with_capacity(2 + data.len()),
        };
        w.tpm2b(data);
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }

    /// `TPM2_EvictControl` (Part 3, clause 28.5): makes a transient object persistent or evicts a
    /// persistent one. The persistent state is in memory only (slice C writes it to the
    /// snapshot).
    pub(crate) fn cmd_evict_control(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let persistent_handle = p.u32()?;
        p.finish()?;
        // `TPMI_DH_PERSISTENT` (Part 2, Table 52).
        if handle::handle_type(persistent_handle) != handle::HT_PERSISTENT {
            return Err(Rc::VALUE.in_param(1));
        }
        let auth = inv.handles[0];
        let object = inv.handles[1];
        let owner = auth == handle::RH_OWNER;
        if handle::handle_type(object) == handle::HT_TRANSIENT {
            let o = self.objects.get(object).ok_or(Rc::reference_handle(1))?;
            // Clause 28.5.1, transient object: ATTRIBUTES (Null hierarchy, public only, stClear),
            // HIERARCHY (proper hierarchy for the authorization), RANGE, NV_DEFINED, NV_SPACE.
            if o.hierarchy == handle::RH_NULL
                || o.sensitive.is_none()
                || o.has(object_attr::ST_CLEAR)
            {
                return Err(Rc::ATTRIBUTES.in_handle(2));
            }
            let proper = if owner {
                matches!(o.hierarchy, handle::RH_OWNER | handle::RH_ENDORSEMENT)
            } else {
                o.hierarchy == handle::RH_PLATFORM
            };
            if !proper {
                return Err(Rc::HIERARCHY.in_handle(2));
            }
            let in_range = if owner {
                (handle::PERSISTENT_FIRST..handle::PLATFORM_PERSISTENT).contains(&persistent_handle)
            } else {
                (handle::PLATFORM_PERSISTENT..=handle::PERSISTENT_LAST).contains(&persistent_handle)
            };
            if !in_range {
                return Err(Rc::RANGE.in_param(1));
            }
            if self.persistent.objects.get(persistent_handle).is_some() {
                return Err(Rc::NV_DEFINED);
            }
            if self.persistent.objects.is_full() {
                return Err(Rc::NV_SPACE);
            }
            let copy = o.clone();
            self.persistent.objects.insert(persistent_handle, copy);
        } else {
            // Persistent object: RANGE for the owner outside its range, HANDLE if the two
            // handles differ.
            if owner && !(handle::PERSISTENT_FIRST..handle::PLATFORM_PERSISTENT).contains(&object) {
                return Err(Rc::RANGE.in_handle(2));
            }
            if object != persistent_handle {
                return Err(Rc::HANDLE.in_param(1));
            }
            self.persistent.objects.remove(object);
        }
        Ok(Output::default())
    }

    /// `TPM2_Sign` (Part 3, clause 20.2) with RSASSA or ECDSA over a SHA-256 digest.
    pub(crate) fn cmd_sign(&mut self, inv: &Invocation, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let digest = p.tpm2b(limits::MAX_DIGEST_SIZE)?;
        p.set_param(2);
        let in_scheme = Scheme::unmarshal(p, &[alg::RSASSA, alg::ECDSA], Rc::SCHEME)?;
        p.set_param(3);
        let validation = TpmtTkHashcheck::unmarshal(p)?;
        p.finish()?;
        if !is_object_hierarchy(validation.hierarchy) {
            return Err(Rc::VALUE.in_param(3));
        }
        let o = find_object(&self.objects, &self.persistent.objects, inv.handles[0])
            .ok_or(Rc::reference_handle(0))?;
        if o.has(object_attr::X509_SIGN) {
            return Err(Rc::ATTRIBUTES.in_handle(1));
        }
        if !o.has(object_attr::SIGN) {
            return Err(Rc::KEY.in_handle(1));
        }
        let key_scheme = match &o.public.params {
            PublicParams::Rsa { scheme, .. } | PublicParams::Ecc { scheme, .. } => *scheme,
            PublicParams::KeyedHash { .. } => return Err(Rc::KEY.in_handle(1)),
        };
        // The key's scheme wins; inScheme must repeat it or be NULL. A key with a NULL scheme
        // signs with inScheme.
        let scheme = if key_scheme.alg != alg::NULL {
            if in_scheme.alg != alg::NULL && in_scheme.alg != key_scheme.alg {
                return Err(Rc::SCHEME.in_param(2));
            }
            key_scheme
        } else {
            in_scheme
        };
        let type_ok = matches!(
            (&o.public.params, scheme.alg),
            (PublicParams::Rsa { .. }, alg::RSASSA) | (PublicParams::Ecc { .. }, alg::ECDSA)
        );
        if !type_ok {
            return Err(Rc::SCHEME.in_param(2));
        }
        if digest.len() != limits::MAX_DIGEST_SIZE {
            return Err(Rc::SIZE.in_param(1));
        }
        // Only NULL tickets exist (module comment): a restricted key, or any other ticket,
        // cannot be validated. NOT VERIFIED: this is stricter than Part 3 (clause 20.2), where the
        // ticket is checked only for a restricted key and an unrestricted key signs with any
        // `validation`; here a non-NULL ticket is refused for an unrestricted key too (safe side,
        // recorded in docs/specs/VTPM.md).
        if o.has(object_attr::RESTRICTED)
            || !validation.digest.is_empty()
            || validation.hierarchy != handle::RH_NULL
        {
            return Err(Rc::TICKET.in_param(3));
        }
        let sensitive = o.sensitive.as_ref().ok_or(Rc::AUTH_UNAVAILABLE)?;
        let signature = match &sensitive.secret {
            Secret::Rsa(key) => TpmtSignature::Rsassa {
                hash: scheme.hash,
                sig: rsa_sign(key, digest, &mut self.drbg).ok_or(Rc::FAILURE)?,
            },
            Secret::Ecc(key) => {
                let (r, s) = ecc_sign(key, digest).ok_or(Rc::FAILURE)?;
                TpmtSignature::Ecdsa {
                    hash: scheme.hash,
                    r,
                    s,
                }
            }
            Secret::Data(_) => return Err(Rc::KEY.in_handle(1)),
        };
        let mut w = Writer::new();
        signature.marshal(&mut w);
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }

    /// `TPM2_VerifySignature` (Part 3, clause 20.1): a failed check is `TPM_RC_SIGNATURE`; on
    /// success the verified ticket (NULL for the Null hierarchy).
    pub(crate) fn cmd_verify_signature(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let digest = p.tpm2b(limits::MAX_DIGEST_SIZE)?;
        p.set_param(2);
        let signature = TpmtSignature::unmarshal(p)?;
        p.finish()?;
        let o = self.object(inv.handles[0]).ok_or(Rc::reference_handle(0))?;
        if !o.has(object_attr::SIGN) {
            return Err(Rc::KEY.in_handle(1));
        }
        let type_ok = matches!(
            (&o.public.params, &signature),
            (PublicParams::Rsa { .. }, TpmtSignature::Rsassa { .. })
                | (PublicParams::Ecc { .. }, TpmtSignature::Ecdsa { .. })
        );
        if !type_ok {
            return Err(Rc::SCHEME.in_param(2));
        }
        if digest.len() != limits::MAX_DIGEST_SIZE {
            return Err(Rc::SIZE.in_param(1));
        }
        let valid = match (&o.public.unique, &signature) {
            (Unique::Rsa(n), TpmtSignature::Rsassa { sig, .. }) => rsa_verify(n, digest, sig),
            (Unique::Ecc { x, y }, TpmtSignature::Ecdsa { r, s, .. }) => {
                ecc_verify(x, y, digest, r, s)
            }
            _ => false,
        };
        if !valid {
            return Err(Rc::SIGNATURE);
        }
        let ticket = self.ticket(
            st::VERIFIED,
            o.hierarchy,
            &[&st::VERIFIED.to_be_bytes(), digest, &o.name],
        );
        let mut w = Writer::new();
        ticket.marshal(&mut w);
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }
}
