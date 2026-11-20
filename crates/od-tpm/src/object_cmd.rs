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
//! the object. Creation in the Null hierarchy uses the reset-bound nullProof. Verification in
//! the Null hierarchy returns an empty ticket. The `validation` ticket of `TPM2_Sign` is
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
//! 175) is as in the open text. Slice 1b (SHA-384/SHA-512 signatures), also NOT VERIFIED against the
//! Part 3 text: `TPM_RC_SCHEME` (not `TPM_RC_HASH`) for an `inScheme` whose hash differs from the
//! hash of the key's scheme; `TPM_RC_SIZE` for a digest whose width differs from the signing hash
//! (parameter 1 of `TPM2_Sign` and `TPM2_VerifySignature`, parameter 2 of the digest commands);
//! that `TPM2_VerifySignature` does not compare the hash label with the key's scheme (only
//! `TPM2_VerifyDigestSignature` and the verify sequence do, `TPM_RC_SCHEME`); the hash of the
//! `nvDigest` of `TPM2_NV_Certify` (the hash of the signing scheme).

use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::codes::{alg, handle, object_attr, st};
use crate::crypto::{
    HashAlg, ecc_sign, hmac_sha256, hmac_sha256_state, hmac_sha256_verify, kdfa_sha256, rsa_sign,
};
use crate::da::DaProtection;
use crate::dispatch::{AuthRole, Invocation};
use crate::hash::AuthValue;
use crate::hierarchy::LEGACY_SEED_BYTES;
use crate::marshal::SIGNING_HASHES;
use crate::marshal::{
    MAX_DATA, PublicParams, Reader, Scheme, SymDef, TpmlPcrSelection, TpmsCreationData,
    TpmsSensitiveCreate, TpmtPublic, TpmtSignature, TpmtTicket, TpmtTkHashcheck, Writer,
    digest_size, limits,
};
use crate::object::{
    MAX_PRIVATE, Object, ParentKey, Secret, check_creation, check_external_public, check_public,
    creation_pcr_digest, derive_object, find_object, is_object_hierarchy, name_hash_alg,
    open_private, primary_drbg, private_blob, qualified_name_of,
};
use crate::random::Drbg;
use crate::response::{Output, Rc};
use crate::state::Tpm;

/// `TPMA_LOCALITY` of locality 0 (Part 2, Table 41, bit 0).
const LOCALITY_ZERO: u8 = 1;
const CREATION_TAG: [u8; 2] = st::CREATION.to_be_bytes();

fn creation_ticket_parts<'a>(name: &'a [u8], creation_hash: &'a [u8]) -> [&'a [u8]; 3] {
    [&CREATION_TAG, name, creation_hash]
}

/// The fixed scheme of a signing key for the digest commands and sequences: RSASSA or ECDSA with
/// SHA-256, SHA-384 or SHA-512 (`TPM_RC_SCHEME` for a NULL scheme or any other).
pub(crate) fn fixed_digest_scheme(object: &Object, handle_no: u8) -> Result<Scheme, Rc> {
    if !object.has(object_attr::SIGN) {
        return Err(Rc::KEY.in_handle(handle_no));
    }
    match object.public.params {
        PublicParams::Rsa { scheme, .. } | PublicParams::Ecc { scheme, .. }
            if matches!(scheme.alg, alg::RSASSA | alg::ECDSA)
                && SIGNING_HASHES.contains(&scheme.hash) =>
        {
            Ok(scheme)
        }
        _ => Err(Rc::SCHEME.in_handle(handle_no)),
    }
}

/// The scheme `TPM2_Sign` and the attestation commands sign with (Part 3, clauses 18.1 and 20.2).
/// The key's own scheme wins and `inScheme` must repeat it, both the algorithm and the hash, or be
/// NULL; a key with a NULL scheme signs with `inScheme`. Another combination, a scheme that does
/// not fit the key type (RSASSA for an ECC key, ECDSA for an RSA key) and a NULL `inScheme` of a
/// key without a scheme are `TPM_RC_SCHEME` for parameter `param`.
///
/// NOT VERIFIED against the Part 3 text: that a hash of `inScheme` that differs from the hash of
/// the key's scheme is `TPM_RC_SCHEME` and not `TPM_RC_HASH`.
pub(crate) fn select_signing_scheme(
    params: &PublicParams,
    in_scheme: Scheme,
    param: u8,
) -> Result<Scheme, Rc> {
    let key_scheme = match params {
        PublicParams::Rsa { scheme, .. } | PublicParams::Ecc { scheme, .. } => *scheme,
        PublicParams::KeyedHash { .. } | PublicParams::MlKem { .. } => {
            return Err(Rc::SCHEME.in_param(param));
        }
    };
    let scheme = if key_scheme.alg != alg::NULL {
        if in_scheme.alg != alg::NULL && in_scheme != key_scheme {
            return Err(Rc::SCHEME.in_param(param));
        }
        key_scheme
    } else {
        in_scheme
    };
    let type_ok = matches!(
        (params, scheme.alg),
        (PublicParams::Rsa { .. }, alg::RSASSA) | (PublicParams::Ecc { .. }, alg::ECDSA)
    );
    if !type_ok || !SIGNING_HASHES.contains(&scheme.hash) {
        return Err(Rc::SCHEME.in_param(param));
    }
    Ok(scheme)
}

/// The digest width of the hash a signature is labelled with (`None` for the NULL signature).
pub(crate) fn signature_digest_size(signature: &TpmtSignature) -> Option<usize> {
    match signature {
        TpmtSignature::Rsassa { hash, .. } | TpmtSignature::Ecdsa { hash, .. } => {
            digest_size(*hash)
        }
        TpmtSignature::Null => None,
    }
}

pub(crate) fn check_signature_scheme(
    signature: &TpmtSignature,
    scheme: Scheme,
    param: u8,
) -> Result<(), Rc> {
    let sig_scheme = match signature {
        TpmtSignature::Rsassa { hash, .. } => (alg::RSASSA, *hash),
        TpmtSignature::Ecdsa { hash, .. } => (alg::ECDSA, *hash),
        TpmtSignature::Null => return Err(Rc::SCHEME.in_param(param)),
    };
    if sig_scheme != (scheme.alg, scheme.hash) {
        return Err(Rc::SCHEME.in_param(param));
    }
    Ok(())
}

pub(crate) fn sign_digest_output(
    object: &Object,
    digest: &[u8],
    scheme: Scheme,
    drbg: &mut Drbg,
) -> Result<Output, Rc> {
    let sensitive = object.sensitive.as_ref().ok_or(Rc::AUTH_UNAVAILABLE)?;
    let signature = match &sensitive.secret {
        Secret::Rsa(key) => TpmtSignature::Rsassa {
            hash: scheme.hash,
            sig: rsa_sign(
                key,
                HashAlg::from_alg(scheme.hash).ok_or(Rc::HASH)?,
                digest,
                drbg,
            )
            .ok_or(Rc::FAILURE)?,
        },
        Secret::Ecc(key) => {
            let (r, s) = ecc_sign(key, digest).ok_or(Rc::FAILURE)?;
            TpmtSignature::Ecdsa {
                hash: scheme.hash,
                r,
                s,
            }
        }
        Secret::Data(_) | Secret::MlKem(_) => return Err(Rc::KEY.in_handle(1)),
    };
    let mut w = Writer::new();
    signature.marshal(&mut w);
    Ok(Output {
        handles: Vec::new(),
        params: w.finish(),
    })
}

/// The implemented selectors of `TPMT_TK_VERIFIED` (Part 2 v185, Table 113).
#[derive(Clone, Copy)]
enum VerifiedKind {
    Legacy,
    Message,
    Digest { hash: u16 },
}

impl VerifiedKind {
    fn tag(self) -> u16 {
        match self {
            Self::Legacy => st::VERIFIED,
            Self::Message => st::MESSAGE_VERIFIED,
            Self::Digest { .. } => st::DIGEST_VERIFIED,
        }
    }

    fn metadata(self) -> Option<[u8; 2]> {
        match self {
            Self::Legacy | Self::Message => None,
            Self::Digest { hash } => Some(hash.to_be_bytes()),
        }
    }
}

pub(crate) fn verified_ticket_mac(proof: &[u8], tag: u16) -> Hmac<Sha256> {
    let mut mac = hmac_sha256_state(proof);
    mac.update(&tag.to_be_bytes());
    mac
}

pub(crate) struct VerifiedTicket<'a> {
    kind: VerifiedKind,
    hierarchy: u32,
    hmac: &'a [u8],
}

impl<'a> VerifiedTicket<'a> {
    pub(crate) fn signed_hash_alg(&self, key_name_alg: u16) -> Option<u16> {
        match self.kind {
            VerifiedKind::Legacy => Some(key_name_alg),
            VerifiedKind::Message => None,
            VerifiedKind::Digest { hash } => Some(hash),
        }
    }

    pub(crate) fn unmarshal(p: &mut Reader<'a>) -> Result<Self, Rc> {
        let tag = p.u16()?;
        if !matches!(
            tag,
            st::VERIFIED | st::MESSAGE_VERIFIED | st::DIGEST_VERIFIED
        ) {
            return Err(p.err(Rc::TAG));
        }
        let hierarchy = p.u32()?;
        if !is_object_hierarchy(hierarchy) {
            return Err(p.err(Rc::VALUE));
        }
        let kind = match tag {
            st::MESSAGE_VERIFIED => VerifiedKind::Message,
            st::DIGEST_VERIFIED => {
                let hash = p.u16()?;
                if digest_size(hash).is_none() {
                    return Err(p.err(Rc::HASH));
                }
                VerifiedKind::Digest { hash }
            }
            _ => VerifiedKind::Legacy,
        };
        let hmac = p.tpm2b(limits::MAX_PCR_DIGEST_SIZE)?;
        Ok(Self {
            kind,
            hierarchy,
            hmac,
        })
    }

    pub(crate) fn message_output(hierarchy: u32, hmac: &'a [u8]) -> Output {
        Self {
            kind: VerifiedKind::Message,
            hierarchy,
            hmac,
        }
        .output()
    }

    fn output(&self) -> Output {
        let mut w = Writer::new();
        w.u16(self.kind.tag());
        w.u32(self.hierarchy);
        if let Some(meta) = self.kind.metadata() {
            w.bytes(&meta);
        }
        w.tpm2b(self.hmac);
        Output {
            handles: Vec::new(),
            params: w.finish(),
        }
    }
}

/// What a child needs from its parent, copied so that the parent is not borrowed while the TPM
/// DRBG is used.
struct ParentCtx {
    seed: Zeroizing<Vec<u8>>,
    symmetric: SymDef,
    name_alg: u16,
    name: Vec<u8>,
    qualified_name: Vec<u8>,
    hierarchy: u32,
    fixed_tpm: bool,
    enc_dup: bool,
}

impl ParentCtx {
    fn key(&self) -> ParentKey<'_> {
        ParentKey {
            name_alg: self.name_alg,
            seed: &self.seed,
            symmetric: self.symmetric,
        }
    }
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
    /// 3, 5 and 7): `TPM_RC_AUTH_UNAVAILABLE` if the object has no private part, then DA lockout is
    /// checked for sessions that require its authValue. For a password or
    /// HMAC session (`policy` false) `TPM_RC_POLICY_FAIL` when the USER role needs a policy
    /// (`userWithAuth` CLEAR) and `TPM_RC_AUTH_TYPE` when the ADMIN role does (`adminWithPolicy`
    /// SET). The DUP role always requires policy. A policy session is checked
    /// against the `authPolicy` by the dispatcher.
    pub(crate) fn object_auth(
        &self,
        object: &Object,
        idx: usize,
        role: AuthRole,
        policy: bool,
        da: DaProtection,
    ) -> Result<AuthValue, Rc> {
        let s = object.sensitive.as_ref().ok_or(Rc::AUTH_UNAVAILABLE)?;
        self.da_check(da)?;
        if !policy {
            match role {
                AuthRole::Dup => return Err(Rc::AUTH_TYPE),
                AuthRole::Admin if object.has(object_attr::ADMIN_WITH_POLICY) => {
                    return Err(Rc::AUTH_TYPE);
                }
                AuthRole::User if !object.has(object_attr::USER_WITH_AUTH) => {
                    return Err(Rc::POLICY_FAIL.in_session((idx + 1) as u8));
                }
                _ => {}
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

    /// Selects primary derivation material while retaining the legacy templates' identities.
    fn hierarchy_seed(&self, hierarchy: u32, params: &PublicParams) -> &[u8] {
        let seed: &[u8] = match hierarchy {
            handle::RH_PLATFORM => &self.persistent.platform_seed[..],
            handle::RH_OWNER => &self.persistent.owner_seed[..],
            handle::RH_ENDORSEMENT => &self.persistent.endorsement_seed[..],
            _ => &self.hv.null_seed[..],
        };
        match params {
            PublicParams::MlKem { .. } => seed,
            PublicParams::Rsa { .. }
            | PublicParams::Ecc { .. }
            | PublicParams::KeyedHash { .. } => &seed[..LEGACY_SEED_BYTES],
        }
    }

    /// The proof value of a permanent object hierarchy. Part 3, clause 24.6 allows deriving
    /// proofs from the Primary Seeds ("if derived
    /// from the Primary Seeds, the derivation of ehProof shall use both the SPS and EPS"): here
    /// `proof = KDFa(SHA-256, seed, "ODTPM-<H>PROOF", NULL, NULL, 256)` and for the endorsement
    /// hierarchy the storage seed is the context, so `TPM2_Clear` (new SPS) changes `shProof`
    /// and `ehProof` and nothing needs to be stored. NOT VERIFIED: that this satisfies the
    /// "second HMAC with the EPS as key" shape of the clause (it uses KDFa/HMAC with the EPS as
    /// key and the SPS as context).
    pub(crate) fn proof(&self, hierarchy: u32) -> Option<Zeroizing<Vec<u8>>> {
        let p = &self.persistent;
        Some(match hierarchy {
            handle::RH_PLATFORM => kdfa_sha256(
                &p.platform_seed[..LEGACY_SEED_BYTES],
                b"ODTPM-PHPROOF",
                &[],
                &[],
                256,
            ),
            handle::RH_OWNER => kdfa_sha256(
                &p.owner_seed[..LEGACY_SEED_BYTES],
                b"ODTPM-SHPROOF",
                &[],
                &[],
                256,
            ),
            handle::RH_ENDORSEMENT => kdfa_sha256(
                &p.endorsement_seed[..LEGACY_SEED_BYTES],
                b"ODTPM-EHPROOF",
                &p.owner_seed[..LEGACY_SEED_BYTES],
                &[],
                256,
            ),
            _ => return None,
        })
    }
    /// Proof for temporary objects and session contexts, changed by each TPM Reset.
    pub(crate) fn null_proof(&self) -> Zeroizing<Vec<u8>> {
        kdfa_sha256(
            &self.hv.null_seed[..LEGACY_SEED_BYTES],
            b"ODTPM-NULLPROOF",
            &[],
            &[],
            256,
        )
    }

    pub(crate) fn hierarchy_proof(&self, hierarchy: u32) -> Option<Zeroizing<Vec<u8>>> {
        if hierarchy == handle::RH_NULL {
            Some(self.null_proof())
        } else {
            self.proof(hierarchy)
        }
    }

    pub(crate) fn valid_creation_ticket(
        &self,
        name: &[u8],
        creation_hash: &[u8],
        hierarchy: u32,
        digest: &[u8],
    ) -> bool {
        self.hierarchy_proof(hierarchy).is_some_and(|proof| {
            hmac_sha256_verify(&proof, &creation_ticket_parts(name, creation_hash), digest)
        })
    }

    pub(crate) fn valid_verified_ticket(
        &self,
        key_name: &[u8],
        signed_value: &[&[u8]],
        ticket: &VerifiedTicket<'_>,
    ) -> bool {
        if !self.permanent_enabled(ticket.hierarchy) {
            return false;
        }
        self.proof(ticket.hierarchy).is_some_and(|proof| {
            let mut mac = verified_ticket_mac(&proof, ticket.kind.tag());
            for part in signed_value {
                mac.update(part);
            }
            mac.update(key_name);
            if let Some(metadata) = ticket.kind.metadata() {
                mac.update(&metadata);
            }
            mac.verify_slice(ticket.hmac).is_ok()
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
        let s = o.sensitive.as_ref().ok_or(Rc::TYPE.in_handle(1))?;
        let (symmetric, _) = o
            .public
            .storage_parameters()
            .filter(|(symmetric, _)| *symmetric != SymDef::Null)
            .ok_or(Rc::TYPE.in_handle(1))?;
        Ok(ParentCtx {
            seed: s.seed.clone(),
            symmetric,
            name_alg: o.public.name_alg,
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
        let mut selection = selection.clone();
        self.pcrs.filter_selection(&mut selection);
        // The creation hash and the PCR digest use the hash of the new object's nameAlg; the ticket
        // is an HMAC-SHA-256 under the hierarchy proof, like every ticket of this TPM.
        let name_hash = name_hash_alg(obj.public.name_alg).unwrap_or(HashAlg::Sha256);
        let pcr_digest = creation_pcr_digest(&self.pcrs, &selection, name_hash);
        let data = TpmsCreationData {
            pcr_select: selection,
            pcr_digest,
            locality: LOCALITY_ZERO,
            parent_name_alg: parent.0,
            parent_name: parent.1.to_vec(),
            parent_qualified_name: parent.2.to_vec(),
            outside_info: outside_info.to_vec(),
        };
        let mut w = Writer::new();
        data.marshal(&mut w);
        let bytes = w.finish();
        let hash = crate::crypto::hash(name_hash, &[&bytes])
            .as_slice()
            .to_vec();
        let proof = self
            .hierarchy_proof(obj.hierarchy)
            .expect("a newly created object has a hierarchy proof");
        let ticket = TpmtTicket {
            tag: st::CREATION,
            hierarchy: obj.hierarchy,
            digest: hmac_sha256(&proof, &creation_ticket_parts(&obj.name, &hash)).to_vec(),
        };
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
        let seed = self.hierarchy_seed(hierarchy, &template.params);
        let mut drbg = primary_drbg(seed, &template, &sens.data)?;
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
        // Library v185 Part 3 §12.1.1 requires this match when creating a fixed Storage child.
        if template.attributes & object_attr::FIXED_PARENT != 0
            && let Some((symmetric, _)) = template.storage_parameters()
        {
            if template.name_alg != parent.name_alg {
                return Err(Rc::HASH.in_param(2));
            }
            if symmetric != parent.symmetric {
                return Err(Rc::SYMMETRIC.in_param(2));
            }
        }
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
        let blob = private_blob(
            &obj.public,
            &obj.name,
            sensitive,
            &sensitive.auth,
            parent.key(),
            &iv,
        )?;
        let (creation, creation_hash, ticket) = self.creation_artifacts(
            &obj,
            &selection,
            outside,
            (parent.name_alg, &parent.name, &parent.qualified_name),
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
        let sensitive = open_private(&public, parent.key(), blob)?;
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

    /// Rewraps a loaded child with a new authValue. The current object and its authorization
    /// stay available until the caller unloads or replaces it (Part 3 v185, §12.8).
    pub(crate) fn cmd_object_change_auth(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        // The limit is one digest of the object's nameAlg, known once the handle is resolved.
        let new_auth = p.tpm2b(limits::MAX_HASH_SIZE)?;
        let size_error = p.err(Rc::SIZE);
        p.finish()?;

        let object = find_object(&self.objects, &self.persistent.objects, inv.handles[0])
            .ok_or(Rc::reference_handle(0))?;
        let auth = AuthValue::from_slice_trimmed_up_to(
            new_auth,
            name_hash_alg(object.public.name_alg)?.size(),
        )
        .ok_or(size_error)?;
        let sensitive = object.sensitive.as_ref().ok_or(Rc::AUTH_UNAVAILABLE)?;
        let parent = find_object(&self.objects, &self.persistent.objects, inv.handles[1])
            .ok_or(Rc::reference_handle(1))?;
        let parent_sensitive = parent.sensitive.as_ref().ok_or(Rc::TYPE.in_handle(2))?;
        let (parent_symmetric, _) = parent
            .public
            .storage_parameters()
            .filter(|(symmetric, _)| *symmetric != SymDef::Null)
            .ok_or(Rc::TYPE.in_handle(2))?;
        if object.qualified_name
            != qualified_name_of(object.public.name_alg, &parent.qualified_name, &object.name)
        {
            return Err(Rc::PARENT);
        }

        let mut iv = [0u8; 16];
        self.drbg.generate(&mut iv).ok_or(Rc::FAILURE)?;
        let blob = private_blob(
            &object.public,
            &object.name,
            sensitive,
            &auth,
            ParentKey {
                name_alg: parent.public.name_alg,
                seed: &parent_sensitive.seed,
                symmetric: parent_symmetric,
            },
            &iv,
        )?;
        let mut params = Writer::new();
        params.tpm2b(&blob);
        Ok(Output {
            handles: Vec::new(),
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

    /// `TPM2_Sign` (Part 3 v185 §20.5) with RSASSA or ECDSA over a SHA-256, SHA-384 or SHA-512
    /// digest, of the hash of the signing scheme.
    pub(crate) fn cmd_sign(&mut self, inv: &Invocation, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let digest = p.tpm2b(limits::MAX_HASH_SIZE)?;
        p.set_param(2);
        let in_scheme = Scheme::unmarshal_signing(p, Rc::SCHEME)?;
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
        if matches!(
            o.public.params,
            PublicParams::KeyedHash { .. } | PublicParams::MlKem { .. }
        ) {
            return Err(Rc::KEY.in_handle(1));
        }
        // The key's scheme wins; inScheme must repeat it or be NULL. A key with a NULL scheme
        // signs with inScheme.
        let scheme = select_signing_scheme(&o.public.params, in_scheme, 2)?;
        if digest_size(scheme.hash) != Some(digest.len()) {
            return Err(Rc::SIZE.in_param(1));
        }
        self.check_signing_ticket(o, digest, &validation)?;
        sign_digest_output(o, digest, scheme, &mut self.drbg)
    }

    /// `TPM2_SignDigest` (Part 3 v185 §20.7) uses the key's fixed scheme and signs a prehash.
    pub(crate) fn cmd_sign_digest(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        // Neither RSASSA nor ECDSA accepts a signature context (Part 2 v185, Table 220).
        p.tpm2b(0)?;
        p.set_param(2);
        let digest = p.tpm2b(limits::MAX_HASH_SIZE)?;
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
        let scheme = fixed_digest_scheme(o, 1)?;
        if digest_size(scheme.hash) != Some(digest.len()) {
            return Err(Rc::SIZE.in_param(2));
        }
        self.check_signing_ticket(o, digest, &validation)?;
        sign_digest_output(o, digest, scheme, &mut self.drbg)
    }

    fn check_signing_ticket(
        &self,
        o: &Object,
        digest: &[u8],
        ticket: &TpmtTkHashcheck,
    ) -> Result<(), Rc> {
        if !o.has(object_attr::RESTRICTED)
            && ticket.hierarchy == handle::RH_NULL
            && ticket.digest.is_empty()
        {
            return Ok(());
        }
        // Currently every hash produces a different digest width. The preceding length check and
        // the HASHCHECK over those bytes therefore bind the digest to the signing scheme's hash.
        if !self.valid_hashcheck_ticket(digest, ticket) {
            return Err(Rc::TICKET.in_param(3));
        }
        Ok(())
    }

    /// `TPM2_VerifySignature` (Part 3, clause 20.1): a failed check is `TPM_RC_SIGNATURE`; on
    /// success the verified ticket (NULL for the Null hierarchy).
    pub(crate) fn cmd_verify_signature(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let digest = p.tpm2b(limits::MAX_HASH_SIZE)?;
        p.set_param(2);
        // A hash outside SHA-256, SHA-384 and SHA-512 is `TPM_RC_HASH` for the signature here.
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
        // The digest has the width of the hash the signature is labelled with.
        if signature_digest_size(&signature) != Some(digest.len()) {
            return Err(Rc::SIZE.in_param(1));
        }
        if !o.verify_signature(digest, &signature) {
            return Err(Rc::SIGNATURE);
        }
        Ok(self.verified_signature_output(o, digest, VerifiedKind::Legacy))
    }

    /// `TPM2_VerifyDigestSignature` (Part 3 v185 §20.4) verifies the supplied prehash.
    pub(crate) fn cmd_verify_digest_signature(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        // Neither RSASSA nor ECDSA accepts a signature context (Part 2 v185, Table 220).
        p.tpm2b(0)?;
        p.set_param(2);
        let digest = p.tpm2b(limits::MAX_HASH_SIZE)?;
        p.set_param(3);
        let signature = TpmtSignature::unmarshal(p)?;
        p.finish()?;

        let o = self.object(inv.handles[0]).ok_or(Rc::reference_handle(0))?;
        let key_scheme = fixed_digest_scheme(o, 1)?;
        check_signature_scheme(&signature, key_scheme, 3)?;
        if digest_size(key_scheme.hash) != Some(digest.len()) {
            return Err(Rc::SIZE.in_param(2));
        }
        if !o.verify_signature(digest, &signature) {
            return Err(Rc::SIGNATURE);
        }
        Ok(self.verified_signature_output(
            o,
            digest,
            VerifiedKind::Digest {
                hash: key_scheme.hash,
            },
        ))
    }

    fn verified_signature_output(&self, o: &Object, digest: &[u8], kind: VerifiedKind) -> Output {
        let tag = kind.tag().to_be_bytes();
        let metadata = kind.metadata();
        let ticket = self.ticket(
            kind.tag(),
            o.hierarchy,
            &[&tag, digest, &o.name, metadata.as_slice().as_flattened()],
        );
        VerifiedTicket {
            kind,
            hierarchy: ticket.hierarchy,
            hmac: &ticket.digest,
        }
        .output()
    }
}
