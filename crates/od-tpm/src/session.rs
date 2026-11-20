//! HMAC authorization sessions (Part 1 v184, clauses 16.6-17.6) and the commands
//! `TPM2_StartAuthSession` and `TPM2_FlushContext`.
//!
//! Slice A supports only what needs no key objects:
//! - session type `TPM_SE_HMAC`; slice C2 adds `TPM_SE_POLICY` and `TPM_SE_TRIAL` (the policy
//!   state and the policy commands are in `policy.rs`; a policy session has no binding, `bind`
//!   only feeds the session key, and a trial session cannot authorize anything);
//! - `tpmKey` = `TPM_RH_NULL` (no salt, so `encryptedSalt` must be empty);
//! - `bind` = `TPM_RH_NULL` (an unbound, unsalted session: the session key is the Empty Buffer) or
//!   a hierarchy authorization handle (owner, platform, endorsement, lockout): a bound session,
//!   `sessionKey = KDFa(sessionAlg, authValue_bind, "ATH", nonceTPM, nonceCaller, bits)`
//!   (Part 1, clause 17.6.10, Equation 20);
//! - `authHash` SHA-256; `symmetric` = `TPM_ALG_NULL`, AES-128-CFB or XOR(SHA-256) (slice C3,
//!   `paramenc.rs`: the attributes `decrypt`/`encrypt` need a symmetric algorithm, otherwise
//!   `TPM_RC_SYMMETRIC`).
//! - An additional HMAC session may audit successful commands (Part 1 v185, clause 17).
//!
//! Authorization HMAC (clause 17.6.5, Equation 17): `authHMAC = HMAC(sessionKey || authValue,
//! pHash || nonceNewer || nonceOlder || sessionAttributes)` with `pHash` = cpHash for a command
//! and rpHash for a response; for a command `nonceNewer` is `nonceCaller` and `nonceOlder` the
//! `nonceTPM` of the previous response, for a response the other way round. The nonce size fields
//! are not hashed. When the authorization is for the entity the session is bound to, the key is
//! `sessionKey` alone (Equation 22), in the command and in the response. When the key is empty the
//! caller may send an empty HMAC (clause 17.6.15), the response then carries an empty HMAC too.
//! The nonce size of the session is the size of the initial `nonceCaller` (at least 16 octets,
//! at most one digest); a later `nonceCaller` may be of any size up to it. The TPM rolls `nonceTPM`
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
//! entity that is neither NULL nor a hierarchy authorization (slice A binds only to those);
//! `TPM_RC_NONCE` for a `nonceCaller` larger than the session nonce; `TPM_RC_ATTRIBUTES` for the
//! reserved or audit attribute bits and `TPM_RC_SYMMETRIC` for `decrypt`/`encrypt` (the session
//! has no symmetric algorithm, `paramenc.rs`); `TPM_RC_HANDLE` with the parameter number for a flush handle that
//! names no loaded session or object; that the bound-entity test compares the handle and the
//! authValue (the reference text records "Name and authValue" and, for an NV index, the
//! authValue as well); that the authValue used in `KDFa` has trailing zeros removed (clause
//! 17.6.5 says so for authorization computations); that a session handle `0x02nnnnnn` and
//! `0x03nnnnnn` name the same slot when flushing ("the upper byte is ignored", clause 28.4).

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::codes::{alg, handle, se, session_attr};
use crate::crypto::{DIGEST, hmac_sha256, hmac_sha256_verify, kdfa_sha256};
use crate::da::DaClass;
use crate::dispatch::Invocation;
use crate::hash::{AuthValue, password_matches};
use crate::marshal::{Reader, Writer, limits};
use crate::paramenc::SessionSym;
use crate::policy::PolicyState;
use crate::response::{Output, Rc};
use crate::state::{TimePoint, Tpm};

/// Loaded and active HMAC sessions at once (`TPM_PT_HR_LOADED_MIN` and `TPM_PT_ACTIVE_SESSIONS_MAX`;
/// PTP 1.07, Table 2: at least 3 loaded and 64 active).
pub const MAX_SESSIONS: usize = 64;

/// Smallest `nonceCaller` of `TPM2_StartAuthSession` (Part 1, clause 17.6.3.2).
const MIN_NONCE: usize = 16;

/// The entity a bound session was started with.
#[derive(Clone)]
pub(crate) struct BoundEntity {
    pub handle: u32,
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
    /// `sessionKey`: empty (unbound, unsalted) or one digest.
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
    /// Set on the first successful command audited by this session.
    pub audit_digest: Option<[u8; DIGEST]>,
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
        cp_hash: &[u8; DIGEST],
        rp_hash: &[u8; DIGEST],
    ) {
        let session = self
            .get_mut(slot)
            .expect("validated audit session stays loaded");
        let first = session.audit_digest.is_none();
        let base = if reset {
            [0; DIGEST]
        } else {
            session.audit_digest.unwrap_or([0; DIGEST])
        };
        let mut hash = Sha256::new();
        hash.update(base);
        hash.update(cp_hash);
        hash.update(rp_hash);
        session.audit_digest = Some(hash.finalize().into());
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

/// The HMAC key of an authorization of the entity `(entity_handle, entity_auth)` with `session`,
/// and whether the entity's authValue went into it. A session bound to this very entity (same
/// handle, same authValue as recorded at start) uses the session key alone (Equation 22).
pub(crate) fn authorization_key(
    session: &Session,
    entity_handle: u32,
    entity_auth: &AuthValue,
) -> (Zeroizing<Vec<u8>>, bool) {
    let bound_here = session.bound.as_ref().is_some_and(|b| {
        b.handle == entity_handle && password_matches(b.auth.as_slice(), entity_auth.as_slice())
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
    cp_hash: &[u8; DIGEST],
    nonce_caller: &[u8],
    parameter_nonces: [&[u8]; 2],
    attributes: u8,
    hmac: &[u8],
) -> bool {
    if key.is_empty() && hmac.is_empty() {
        return true;
    }
    hmac_sha256_verify(
        key,
        &[
            cp_hash,
            nonce_caller,
            &session.nonce_tpm,
            parameter_nonces[0],
            parameter_nonces[1],
            &[attributes],
        ],
        hmac,
    )
}

/// The HMAC of a response acknowledgement (Equation 17 with rpHash).
pub(crate) fn response_hmac(
    key: &[u8],
    rp_hash: &[u8; DIGEST],
    nonce_tpm_new: &[u8],
    nonce_caller: &[u8],
    attributes: u8,
) -> [u8; DIGEST] {
    hmac_sha256(key, &[rp_hash, nonce_tpm_new, nonce_caller, &[attributes]])
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
    /// `TPM2_StartAuthSession` (Part 3, clause 11.1), unbound-unsalted and hierarchy-bound HMAC
    /// sessions. Handles `tpmKey`, `bind` were checked by the dispatcher.
    pub(crate) fn cmd_start_auth_session(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let nonce_caller = p.tpm2b(limits::MAX_DIGEST_SIZE)?;
        p.set_param(2);
        let encrypted_salt = p.tpm2b(limits::MAX_2B_BUFFER_SIZE)?;
        p.set_param(3);
        let session_type = p.u8()?;
        p.set_param(4);
        let symmetric = SessionSym::unmarshal(p)?;
        p.set_param(5);
        let auth_hash = p.u16()?;
        p.finish()?;
        if auth_hash != alg::SHA256 {
            return Err(Rc::HASH.in_param(5));
        }
        let kind = match session_type {
            se::HMAC => SessionKind::Hmac,
            se::POLICY => SessionKind::Policy,
            se::TRIAL => SessionKind::Trial,
            _ => return Err(Rc::VALUE.in_param(3)),
        };
        if nonce_caller.len() < MIN_NONCE {
            return Err(Rc::SIZE.in_param(1));
        }
        if !encrypted_salt.is_empty() {
            // `tpmKey` is `TPM_RH_NULL`: the salt must be the Empty Buffer.
            return Err(Rc::VALUE.in_param(2));
        }
        let bind = inv.handles[1];
        let bind_da = self.da_class_of(bind);
        let bound = if bind == handle::RH_NULL {
            None
        } else {
            let auth = self.permanent_auth(bind).ok_or(Rc::VALUE.in_handle(2))?;
            Some(BoundEntity { handle: bind, auth })
        };
        let reserved = self.context.reserved_slots();
        let mut nonce_tpm = vec![0u8; nonce_caller.len()];
        self.drbg.generate(&mut nonce_tpm).ok_or(Rc::FAILURE)?;
        let key = match &bound {
            None => AuthValue::empty(),
            Some(b) => {
                let k = kdfa_sha256(
                    b.auth.as_slice(),
                    b"ATH",
                    &nonce_tpm,
                    nonce_caller,
                    (DIGEST * 8) as u32,
                );
                AuthValue::from_slice(&k).ok_or(Rc::FAILURE)?
            }
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
                    policy: PolicyState::new(),
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
