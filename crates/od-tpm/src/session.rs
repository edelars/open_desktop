//! HMAC authorization sessions (Part 1 v184, clauses 16.6-17.6) and the commands
//! `TPM2_StartAuthSession` and `TPM2_FlushContext`.
//!
//! Slice A supports only what needs no key objects:
//! - session type `TPM_SE_HMAC`; slice C2 adds `TPM_SE_POLICY` and `TPM_SE_TRIAL` (the policy
//!   state and the policy commands are in `policy.rs`; a policy session has no binding, `bind`
//!   only feeds the session key, and a trial session cannot authorize anything);
//! - `tpmKey` = `TPM_RH_NULL` (no salt, so `encryptedSalt` must be empty) or a loaded RSA or ECC
//!   decrypt key (a salted session, slice 4): `encryptedSalt` is opened like the seed of a credential
//!   with the label "SECRET" (`decrypt_session_salt`);
//! - `bind` = `TPM_RH_NULL` (an unbound, unsalted session: the session key is the Empty Buffer) or
//!   a hierarchy authorization handle (owner, platform, endorsement, lockout), a loaded object or a
//!   defined NV index: a bound session. A bound or salted session has
//!   `sessionKey = KDFa(sessionAlg, authValue_bind || salt, "ATH", nonceTPM, nonceCaller, bits)`
//!   (Part 1, clause 17.6.10, Equation 20). The salt is used only here: the session key is what
//!   `TPM2_ContextSave` keeps, so a salted session round-trips through a context like a bound one;
//! - `authHash` SHA-256, SHA-384 or SHA-512 (hash agility, step D: `Session::auth_hash` selects the hash of
//!   every HMAC, cpHash, rpHash, `KDFa` and audit digest of the session; since step E a policy or
//!   trial session also takes SHA-256, SHA-384 or SHA-512 and its policy digest follows the hash, see
//!   `policy.rs`; any other hash is `TPM_RC_HASH`);
//!   `symmetric` = `TPM_ALG_NULL`, AES-128-CFB or XOR(SHA-256) (slice C3,
//!   `paramenc.rs`: the attributes `decrypt`/`encrypt` need a symmetric algorithm, otherwise
//!   `TPM_RC_SYMMETRIC`).
//! - An HMAC session may audit successful commands, either as an additional session or while it
//!   authorizes a handle (Part 1 rev 01.83, clauses 19.6.1 and 20.1); the first audited use of a
//!   bound session drops the binding (`Sessions::extend_audit`), the authValue then joins the key.
//!
//! Documented narrowing: the reference accepts a PCR as `bind` (`TPMI_DH_ENTITY`); here it is
//! `TPM_RC_VALUE`.
//! The binding is the Name of the entity together with its authValue (Session.c
//! `SessionComputeBoundEntity` at start, `IsSessionBindEntity` at authorization; Part 1, clauses
//! 19.6.10-19.6.11): an NV index whose Name changed (first write) or another object that has the
//! bound handle and authValue is not the bound entity and gets the key `sessionKey || authValue`.
//!
//! Authorization HMAC (clause 17.6.5, Equation 17): `authHMAC = HMAC(sessionKey || authValue,
//! pHash || nonceNewer || nonceOlder || sessionAttributes)` with `pHash` = cpHash for a command
//! and rpHash for a response; for a command `nonceNewer` is `nonceCaller` and `nonceOlder` the
//! `nonceTPM` of the previous response, for a response the other way round. The nonce size fields
//! are not hashed. When the authorization is for the entity the session is bound to, the key is
//! `sessionKey` alone (Equation 22), in the command and in the response. When the key is empty the
//! caller may send an empty HMAC (clause 17.6.15), the response then carries an empty HMAC too.
//! The nonce size of the session is the size of the initial `nonceCaller` (at least 16 octets,
//! at most one digest of `authHash`); a later `nonceCaller` may be of any size up to it. The TPM rolls `nonceTPM`
//! on every successful use; a command that fails leaves the session unchanged. `continueSession`
//! CLEAR flushes the session after a successful command (clause 17.6.17).
//!
//! Sessions are volatile: every `Startup` and `_TPM_Init` flushes the loaded ones (clause 17.6.17,
//! Part 3, clause 9.3.1). With slice C2 a session can also be saved by `TPM2_ContextSave`
//! (`context.rs`); a saved session keeps its slot (and so its handle), counts as active and is
//! tracked in `ContextState`.
//!
//! NOT VERIFIED against TPM 2.0 Library v185/v184 (the open text names the rule, not the code or
//! the detail): `TPM_RC_VALUE` with the parameter number for an unknown session type, for
//! a non-empty `encryptedSalt` with a NULL `tpmKey`, and with the handle number for a `bind`
//! entity that is none of those listed above (an undefined NV index);
//! `TPM_RC_NONCE` for a `nonceCaller` larger than the session nonce; `TPM_RC_ATTRIBUTES` for the
//! reserved or audit attribute bits and `TPM_RC_SYMMETRIC` for `decrypt`/`encrypt` (the session
//! has no symmetric algorithm, `paramenc.rs`); `TPM_RC_HANDLE` with the parameter number for a flush handle that
//! names no loaded session or object; that the authValue used in `KDFa` has trailing zeros removed (clause
//! 17.6.5 says so for authorization computations); that a session handle `0x02nnnnnn` and
//! `0x03nnnnnn` name the same slot when flushing ("the upper byte is ignored", clause 28.4).

use zeroize::Zeroizing;

use crate::codes::{handle, object_attr, se, session_attr};
use crate::credential::{SeedError, recover_seed};
use crate::crypto::{DIGEST, HashAlg, TpmDigest, hash, hmac, hmac_verify, kdfa};
use crate::da::DaClass;
use crate::dispatch::{Invocation, handle_name};
use crate::hash::{AuthValue, password_matches};
use crate::marshal::{MAX_RSA_BYTES, Reader, Writer, limits};
use crate::object::find_object;
use crate::paramenc::SessionSym;
use crate::policy::PolicyState;
use crate::response::{Output, Rc};
use crate::state::{TimePoint, Tpm};

/// Loaded and active HMAC sessions at once (`TPM_PT_HR_LOADED_MIN` and `TPM_PT_ACTIVE_SESSIONS_MAX`;
/// PTP 1.07, Table 2: at least 3 loaded and 64 active).
pub const MAX_SESSIONS: usize = 64;

/// Smallest `nonceCaller` of `TPM2_StartAuthSession` (Part 1, clause 17.6.3.2; a shorter one is
/// `TPM_RC_SIZE`, Part 3, clause 11.1).
pub(crate) const MIN_NONCE: usize = 16;

/// Label of the secret of a salted session (Part 1, clause 19.6.13), the NUL included like every
/// label of the TPM's key derivation.
const SECRET_LABEL: &[u8] = b"SECRET\0";

/// The entity a bound session was started with. The reference (Session.c,
/// `SessionComputeBoundEntity` / `IsSessionBindEntity`; Part 1, clauses 19.6.10 and 19.6.11)
/// records the Name of the entity together with its authValue, so that another entity, or the same
/// handle after its Name changed (an NV index after its first write), is not the bound one.
#[derive(Clone)]
pub(crate) struct BoundEntity {
    pub handle: u32,
    /// Name of the entity at the start of the session (`handle_name`): the handle itself for a
    /// hierarchy.
    pub name: Vec<u8>,
    pub auth: AuthValue,
}

/// `TPM_SE_*` of a loaded session.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SessionKind {
    Hmac,
    Policy,
    /// A trial policy session: computes a `policyDigest`, never authorizes.
    Trial,
}

impl SessionKind {
    /// `TPM_HT_HMAC_SESSION` or `TPM_HT_POLICY_SESSION`, the first octet of the handle.
    pub(crate) fn handle_type(self) -> u8 {
        match self {
            SessionKind::Hmac => handle::HT_HMAC_SESSION,
            SessionKind::Policy | SessionKind::Trial => handle::HT_POLICY_SESSION,
        }
    }

    pub(crate) fn is_policy(self) -> bool {
        self != SessionKind::Hmac
    }
}

/// One loaded session.
#[derive(Clone)]
pub(crate) struct Session {
    /// `nonceTPM` of the last response; its size is the session nonce size.
    pub nonce_tpm: Vec<u8>,
    /// Time when this nonce was generated, absent in older saved policy contexts.
    pub nonce_time: Option<TimePoint>,
    /// `authHash` of `TPM2_StartAuthSession` (SHA-256, SHA-384 or SHA-512, for an HMAC, policy or
    /// trial session alike).
    pub auth_hash: HashAlg,
    /// `sessionKey`: empty (unbound, unsalted) or one digest of `auth_hash`.
    pub key: AuthValue,
    pub bound: Option<BoundEntity>,
    /// DA class of the entity whose secret was used to derive sessionKey.
    /// This outlives any association that omits an authValue in an HMAC.
    pub bind_da: DaClass,
    pub kind: SessionKind,
    /// The policy context (`policy.rs`); the initial state for an HMAC session.
    pub policy: PolicyState,
    /// `symmetric` of `TPM2_StartAuthSession`: how the session encrypts parameters.
    pub symmetric: SessionSym,
    /// Set on the first successful command audited by this session, one digest of `auth_hash`.
    pub audit_digest: Option<TpmDigest>,
}

/// Session slots; the handle of slot `n` is `0x02000000 + n` (`HMAC_SESSION_FIRST + n`).
pub(crate) struct Sessions {
    slots: Vec<Option<Session>>,
    /// The slot remains reserved while this session is context-saved.
    pub exclusive_audit: Option<usize>,
}

impl Default for Sessions {
    fn default() -> Self {
        Sessions {
            slots: (0..MAX_SESSIONS).map(|_| None).collect(),
            exclusive_audit: None,
        }
    }
}

/// Handle of the session of `kind` in `slot` (`0x02nnnnnn` HMAC, `0x03nnnnnn` policy).
pub(crate) fn session_handle(kind: SessionKind, slot: usize) -> u32 {
    (u32::from(kind.handle_type()) << 24) | slot as u32
}

impl Sessions {
    pub(crate) fn clear(&mut self) {
        for s in &mut self.slots {
            *s = None;
        }
        self.exclusive_audit = None;
    }

    /// Slot of a loaded session named by a handle: type `0x02` names an HMAC session, `0x03` a
    /// policy or trial session; `None` for another type, a slot out of range, an unloaded slot
    /// or a session of the other family.
    pub(crate) fn slot_of(&self, h: u32) -> Option<usize> {
        let slot = (h & 0x00FF_FFFF) as usize;
        let s = self.get(slot)?;
        (s.kind.handle_type() == handle::handle_type(h)).then_some(slot)
    }

    pub(crate) fn is_loaded(&self, slot: usize) -> bool {
        self.slots.get(slot).is_some_and(Option::is_some)
    }

    pub(crate) fn get(&self, slot: usize) -> Option<&Session> {
        self.slots.get(slot).and_then(Option::as_ref)
    }

    pub(crate) fn get_mut(&mut self, slot: usize) -> Option<&mut Session> {
        self.slots.get_mut(slot).and_then(Option::as_mut)
    }

    pub(crate) fn release(&mut self, slot: usize) {
        self.park(slot);
        if self.exclusive_audit == Some(slot) {
            self.exclusive_audit = None;
        }
    }
    /// Temporarily removes a session whose slot ContextSave will reserve.
    pub(crate) fn park(&mut self, slot: usize) {
        if let Some(s) = self.slots.get_mut(slot) {
            *s = None;
        }
    }
    pub(crate) fn extend_audit(
        &mut self,
        slot: usize,
        reset: bool,
        cp_hash: &[u8],
        rp_hash: &[u8],
    ) {
        let session = self
            .get_mut(slot)
            .expect("validated audit session stays loaded");
        let first = session.audit_digest.is_none();
        // auditDigest' = H_authHash(auditDigest || cpHash || rpHash), starting from zeros of the
        // digest size (Part 1 rev 01.83, clause 19.6.1); `cp_hash` and `rp_hash` are those of
        // `session.auth_hash`.
        let zero = [0u8; 64];
        let base: &[u8] = match (&session.audit_digest, reset) {
            (Some(d), false) => d.as_slice(),
            _ => &zero[..session.auth_hash.size()],
        };
        session.audit_digest = Some(hash(session.auth_hash, &[base, cp_hash, rp_hash]));
        if first {
            session.bound = None;
        }
        if first || reset {
            self.exclusive_audit = Some(slot);
        } else if self.exclusive_audit != Some(slot) {
            self.exclusive_audit = None;
        }
    }

    /// Stores `s` in the lowest free slot that is not `reserved` (the slots of saved sessions).
    pub(crate) fn insert(&mut self, s: Session, reserved: &[usize]) -> Option<usize> {
        let slot = self
            .slots
            .iter()
            .enumerate()
            .position(|(i, x)| x.is_none() && !reserved.contains(&i))?;
        self.slots[slot] = Some(s);
        Some(slot)
    }

    /// Stores `s` in the given free `slot` (`TPM2_ContextLoad` of a session keeps its handle).
    pub(crate) fn insert_at(&mut self, slot: usize, s: Session) -> bool {
        match self.slots.get_mut(slot) {
            Some(x @ None) => {
                *x = Some(s);
                true
            }
            _ => false,
        }
    }

    pub(crate) fn loaded(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    pub(crate) fn loaded_handles(&self) -> Vec<u32> {
        (0..MAX_SESSIONS)
            .filter_map(|i| self.get(i).map(|s| session_handle(s.kind, i)))
            .collect()
    }
}

/// The HMAC key of an authorization of the entity `(entity_name, entity_auth)` with `session`,
/// and whether the entity's authValue went into it. A session bound to this very entity (same
/// Name and same authValue as recorded at start, the reference's `IsSessionBindEntity`) uses the
/// session key alone (Equation 22); `entity_name` is the current Name of the entity, read only for
/// a bound session. Any other entity, even one with the bound handle and authValue, gets the key
/// `sessionKey || authValue`.
pub(crate) fn authorization_key(
    session: &Session,
    entity_name: &[u8],
    entity_auth: &AuthValue,
) -> (Zeroizing<Vec<u8>>, bool) {
    let bound_here = session.bound.as_ref().is_some_and(|b| {
        b.name == entity_name && password_matches(b.auth.as_slice(), entity_auth.as_slice())
    });
    let mut key = Zeroizing::new(session.key.as_slice().to_vec());
    if !bound_here {
        key.extend_from_slice(entity_auth.as_slice());
    }
    (key, !bound_here)
}

/// Checks command authorization (Part 1 v185, Equation 17). `parameter_nonces` holds the
/// other decrypt and encrypt sessions' old TPM nonces, in that order, for the first slot.
/// An empty key accepts an empty HMAC.
pub(crate) fn verify_command_hmac(
    session: &Session,
    key: &[u8],
    cp_hash: &[u8],
    nonce_caller: &[u8],
    parameter_nonces: [&[u8]; 2],
    attributes: u8,
    tag: &[u8],
) -> bool {
    if key.is_empty() && tag.is_empty() {
        return true;
    }
    hmac_verify(
        session.auth_hash,
        key,
        &[
            cp_hash,
            nonce_caller,
            &session.nonce_tpm,
            parameter_nonces[0],
            parameter_nonces[1],
            &[attributes],
        ],
        tag,
    )
}

/// The HMAC of a response acknowledgement (Equation 17 with rpHash), with the `authHash` `alg` of
/// the session; `rp_hash` is the rpHash of `alg`.
pub(crate) fn response_hmac(
    alg: HashAlg,
    key: &[u8],
    rp_hash: &[u8],
    nonce_tpm_new: &[u8],
    nonce_caller: &[u8],
    attributes: u8,
) -> TpmDigest {
    hmac(
        alg,
        key,
        &[rp_hash, nonce_tpm_new, nonce_caller, &[attributes]],
    )
}

/// Checks the attribute octet of an HMAC session in a command (`n` is the session number).
pub(crate) fn check_hmac_session_attributes(attributes: u8, n: usize) -> Result<(), Rc> {
    let num = n as u8;
    if attributes & session_attr::RESERVED != 0 {
        return Err(Rc::ATTRIBUTES.in_session(num));
    }
    if attributes & session_attr::AUDIT == 0
        && attributes & (session_attr::AUDIT_EXCLUSIVE | session_attr::AUDIT_RESET) != 0
    {
        return Err(Rc::ATTRIBUTES.in_session(num));
    }
    // `decrypt` and `encrypt` are checked against the symmetric algorithm of the session and the
    // command by the dispatcher (`paramenc.rs`).
    Ok(())
}

impl Tpm {
    /// `TPM2_StartAuthSession` (Part 3, clause 11.1): HMAC, policy and trial sessions, unbound or
    /// bound, unsalted or salted. Handles `tpmKey`, `bind` were checked by the dispatcher.
    pub(crate) fn cmd_start_auth_session(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        // The limit of `nonceCaller` is one digest of `authHash` (parameter 5, not parsed yet):
        // read up to the largest digest and check the size once `authHash` is known.
        let nonce_caller = p.tpm2b(limits::MAX_HASH_SIZE)?;
        let oversized = nonce_caller.len() > DIGEST;
        let rest = (|| -> Result<_, Rc> {
            p.set_param(2);
            let encrypted_salt = p.tpm2b(limits::MAX_2B_BUFFER_SIZE)?;
            p.set_param(3);
            let session_type = p.u8()?;
            p.set_param(4);
            let symmetric = SessionSym::unmarshal(p)?;
            p.set_param(5);
            let auth_hash = p.u16()?;
            p.finish()?;
            Ok((encrypted_salt, session_type, symmetric, auth_hash))
        })();
        // A `nonceCaller` of more than 32 octets was refused first, before the rest of the
        // parameters, while only SHA-256 existed: it still is, unless `authHash` is a longer hash.
        let (encrypted_salt, session_type, symmetric, auth_hash) = match rest {
            Ok(parts) => parts,
            Err(_) if oversized => return Err(Rc::SIZE.in_param(1)),
            Err(e) => return Err(e),
        };
        let auth_alg = HashAlg::from_alg(auth_hash);
        if oversized && !matches!(auth_alg, Some(HashAlg::Sha384 | HashAlg::Sha512)) {
            return Err(Rc::SIZE.in_param(1));
        }
        let auth_alg = auth_alg.ok_or(Rc::HASH.in_param(5))?;
        let kind = match session_type {
            se::HMAC => SessionKind::Hmac,
            se::POLICY => SessionKind::Policy,
            se::TRIAL => SessionKind::Trial,
            _ => return Err(Rc::VALUE.in_param(3)),
        };
        if nonce_caller.len() > auth_alg.size() {
            return Err(Rc::SIZE.in_param(1));
        }
        if nonce_caller.len() < MIN_NONCE {
            return Err(Rc::SIZE.in_param(1));
        }
        let tpm_key = inv.handles[0];
        let salt = if tpm_key == handle::RH_NULL {
            if !encrypted_salt.is_empty() {
                // `tpmKey` is `TPM_RH_NULL`: the salt must be the Empty Buffer.
                return Err(Rc::VALUE.in_param(2));
            }
            Zeroizing::new(Vec::new())
        } else {
            self.decrypt_session_salt(tpm_key, encrypted_salt)?
        };
        let bind = inv.handles[1];
        let bind_da = self.da_class_of(bind);
        let bound = if bind == handle::RH_NULL {
            None
        } else {
            let auth = self.bind_auth(bind).ok_or(Rc::VALUE.in_handle(2))?;
            let name = handle_name(self, bind);
            Some(BoundEntity {
                handle: bind,
                name,
                auth,
            })
        };
        let reserved = self.context.reserved_slots();
        let mut nonce_tpm = vec![0u8; nonce_caller.len()];
        self.drbg.generate(&mut nonce_tpm).ok_or(Rc::FAILURE)?;
        // sessionKey = KDFa(authHash, authValue_bind || salt, "ATH", nonceTPM, nonceCaller, bits)
        // when the session is bound or the salt is not empty, the Empty Buffer otherwise (Part 1,
        // Equation 20; the reference's condition is `bind != TPM_RH_NULL || seed.size != 0`): a
        // salt that decrypted to the Empty Buffer with a NULL `bind` leaves the key empty.
        let key = if bound.is_none() && salt.is_empty() {
            AuthValue::empty()
        } else {
            let mut material = Zeroizing::new(Vec::new());
            if let Some(b) = &bound {
                material.extend_from_slice(b.auth.as_slice());
            }
            material.extend_from_slice(&salt);
            let k = kdfa(
                auth_alg,
                &material,
                b"ATH",
                &nonce_tpm,
                nonce_caller,
                (auth_alg.size() * 8) as u32,
            );
            AuthValue::from_slice_up_to(&k, auth_alg.size()).ok_or(Rc::FAILURE)?
        };
        let slot = self
            .sessions
            .insert(
                Session {
                    nonce_tpm: nonce_tpm.clone(),
                    nonce_time: (kind == SessionKind::Policy).then(|| self.time_point()),
                    key,
                    // A policy session is not bound: `bind` only feeds the session key
                    // (Part 3, clause 11.1.1: "the session is not bound").
                    bound: if kind.is_policy() { None } else { bound },
                    bind_da,
                    kind,
                    auth_hash: auth_alg,
                    policy: PolicyState::new(auth_alg),
                    symmetric,
                    audit_digest: None,
                },
                &reserved,
            )
            .ok_or(Rc::SESSION_HANDLES)?;
        let mut handles = Writer::new();
        handles.u32(session_handle(kind, slot));
        let mut params = Writer::new();
        params.tpm2b(&nonce_tpm);
        Ok(Output {
            handles: handles.finish(),
            params: params.finish(),
        })
    }

    /// The authValue of the entity a session is bound to: a hierarchy, a loaded object (the Empty
    /// Buffer for an object with no private part, whose sensitive area is zero) or an NV index.
    fn bind_auth(&self, h: u32) -> Option<AuthValue> {
        match handle::handle_type(h) {
            handle::HT_PERMANENT => self.permanent_auth(h),
            handle::HT_NV_INDEX => self.nv_auth(h),
            handle::HT_TRANSIENT | handle::HT_PERSISTENT if self.object(h).is_some() => {
                Some(self.object_auth_now(h).unwrap_or_else(AuthValue::empty))
            }
            _ => None,
        }
    }

    /// The salt of `TPM2_StartAuthSession` (Part 3, clause 11.1, Part 1, clause 19.6.13): the
    /// `encryptedSalt` is made for `tpmKey` like the seed of a credential, with the label
    /// "SECRET\0": RSA-OAEP with the `nameAlg` of the key, or the ephemeral point of an ECDH whose
    /// KDFe (`nameAlg`, party U the ephemeral X, party V the X of `tpmKey`) gives one digest.
    /// `TPM_RC_KEY` when `tpmKey` is no loaded decrypt key with a private part (the spec lists
    /// "does not reference a decrypt key"), `TPM_RC_HASH` when its `nameAlg` is no hash we know, and
    /// `TPM_RC_VALUE` with parameter 2 for an empty or undecryptable secret, a size that does not
    /// match the key, a wrong label or an off-curve point (the spec says `TPM_RC_VALUE` for the size
    /// and for a salt longer than the digest of `nameAlg`; the other decrypt failures are mapped the
    /// same way). An `encryptedSalt` longer than any key's secret is `TPM_RC_SIZE`.
    /// Checked against the text: `TPM_RC_KEY` for a key without `decrypt` (Part 3, clause 11.1:
    /// "tpmKey does not reference a decrypt key").
    /// NOT VERIFIED against TPM 2.0 Library v185/v184: the other codes and their order, `TPM_RC_KEY`
    /// for a key with no private part or of another asymmetric type, the use of the shared KDFe of
    /// `credential.rs` for the salt.
    fn decrypt_session_salt(
        &mut self,
        tpm_key: u32,
        encrypted_salt: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, Rc> {
        let key = find_object(&self.objects, &self.persistent.objects, tpm_key)
            .ok_or(Rc::reference_handle(0))?;
        if key.sensitive.is_none() || !key.has(object_attr::DECRYPT) {
            return Err(Rc::KEY.in_handle(1));
        }
        let name_hash = HashAlg::from_alg(key.public.name_alg).ok_or(Rc::HASH.in_handle(1))?;
        if encrypted_salt.len() > MAX_RSA_BYTES {
            return Err(Rc::SIZE.in_param(2));
        }
        if encrypted_salt.is_empty() {
            return Err(Rc::VALUE.in_param(2));
        }
        recover_seed(key, encrypted_salt, SECRET_LABEL, &mut self.drbg, name_hash).map_err(
            |error| match error {
                SeedError::NoPrivate | SeedError::KeyType => Rc::KEY.in_handle(1),
                SeedError::Ciphertext | SeedError::Point => Rc::VALUE.in_param(2),
            },
        )
    }

    /// `TPM2_FlushContext` (Part 3, clause 28.4): `flushHandle` is a parameter. A session handle
    /// (type `0x02` or `0x03`, the type octet is ignored) must name a loaded session, a transient
    /// handle a loaded object (the sequence objects of phase A); anything else is a value error
    /// (`TPMI_DH_CONTEXT`), a persistent object cannot be flushed.
    pub(crate) fn cmd_flush_context(&mut self, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let h = p.u32()?;
        p.finish()?;
        match handle::handle_type(h) {
            handle::HT_HMAC_SESSION | handle::HT_POLICY_SESSION => {
                let slot = (h & 0x00FF_FFFF) as usize;
                if slot >= MAX_SESSIONS {
                    return Err(Rc::VALUE.in_param(1));
                }
                if self.sessions.is_loaded(slot) {
                    self.sessions.release(slot);
                } else {
                    if !self.context.forget_saved_session(slot) {
                        return Err(Rc::HANDLE.in_param(1));
                    }
                    self.sessions.release(slot);
                }
            }
            handle::HT_TRANSIENT => match self.seq_slot(h, 0) {
                Ok(slot) => self.sequences.release(slot),
                Err(_) => {
                    if !self.objects.release(h) {
                        return Err(Rc::HANDLE.in_param(1));
                    }
                }
            },
            _ => return Err(Rc::VALUE.in_param(1)),
        }
        Ok(Output::default())
    }
}
