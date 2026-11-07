//! HMAC authorization sessions (Part 1 v184, clauses 16.6-17.6) and the commands
//! `TPM2_StartAuthSession` and `TPM2_FlushContext`.
//!
//! Slice A supports only what needs no key objects:
//! - session type `TPM_SE_HMAC` (policy and trial sessions are slice B/C);
//! - `tpmKey` = `TPM_RH_NULL` (no salt, so `encryptedSalt` must be empty);
//! - `bind` = `TPM_RH_NULL` (an unbound, unsalted session: the session key is the Empty Buffer) or
//!   a hierarchy authorization handle (owner, platform, endorsement, lockout): a bound session,
//!   `sessionKey = KDFa(sessionAlg, authValue_bind, "ATH", nonceTPM, nonceCaller, bits)`
//!   (Part 1, clause 17.6.10, Equation 20);
//! - `authHash` SHA-256, `symmetric` = `TPM_ALG_NULL`: no parameter encryption (the attributes
//!   `decrypt`/`encrypt` are refused) and no audit.
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
//! Sessions are volatile: every `Startup` and `_TPM_Init` flushes them (clause 17.6.17, Part 3,
//! clause 9.3.1). No context saving (`TPM2_ContextSave`) exists yet, so a loaded session is the
//! only form of an active session, and `TPM_PT_ACTIVE_SESSIONS_MAX` equals the loaded capacity.
//!
//! NOT VERIFIED against TPM 2.0 Library v185/v184 (the open text names the rule, not the code or
//! the detail): `TPM_RC_VALUE` with the parameter number for a policy or trial session type, for
//! a non-empty `encryptedSalt` with a NULL `tpmKey`, and with the handle number for a `bind`
//! entity that is neither NULL nor a hierarchy authorization (slice A binds only to those);
//! `TPM_RC_NONCE` for a `nonceCaller` larger than the session nonce; `TPM_RC_ATTRIBUTES` for the
//! reserved or audit attribute bits and `TPM_RC_SYMMETRIC` for `decrypt`/`encrypt` (the session
//! has no symmetric algorithm); `TPM_RC_HANDLE` with the parameter number for a flush handle that
//! names no loaded session or object; that the bound-entity test compares the handle and the
//! authValue (the reference text records "Name and authValue" and, for an NV index, the
//! authValue as well); that the authValue used in `KDFa` has trailing zeros removed (clause
//! 17.6.5 says so for authorization computations); that a session handle `0x02nnnnnn` and
//! `0x03nnnnnn` name the same slot when flushing ("the upper byte is ignored", clause 28.4).

use zeroize::Zeroizing;

use crate::codes::{alg, handle, se, session_attr};
use crate::crypto::{DIGEST, hmac_sha256, hmac_sha256_verify, kdfa_sha256};
use crate::dispatch::Invocation;
use crate::hash::{AuthValue, password_matches};
use crate::marshal::{Reader, Writer, limits};
use crate::response::{Output, Rc};
use crate::state::Tpm;

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

/// One loaded HMAC session.
pub(crate) struct Session {
    /// `nonceTPM` of the last response; its size is the session nonce size.
    pub nonce_tpm: Vec<u8>,
    /// `sessionKey`: empty (unbound, unsalted) or one digest.
    pub key: AuthValue,
    pub bound: Option<BoundEntity>,
}

/// Session slots; the handle of slot `n` is `0x02000000 + n` (`HMAC_SESSION_FIRST + n`).
pub(crate) struct Sessions {
    slots: Vec<Option<Session>>,
}

impl Default for Sessions {
    fn default() -> Self {
        Sessions {
            slots: (0..MAX_SESSIONS).map(|_| None).collect(),
        }
    }
}

/// Handle of the session in `slot`.
pub(crate) fn session_handle(slot: usize) -> u32 {
    (u32::from(handle::HT_HMAC_SESSION) << 24) | slot as u32
}

impl Sessions {
    pub(crate) fn clear(&mut self) {
        for s in &mut self.slots {
            *s = None;
        }
    }

    /// Slot of a loaded session named by a handle of type `0x02`; `None` for another type, a slot
    /// out of range or an unloaded slot.
    pub(crate) fn slot_of(&self, h: u32) -> Option<usize> {
        if handle::handle_type(h) != handle::HT_HMAC_SESSION {
            return None;
        }
        let slot = (h & 0x00FF_FFFF) as usize;
        self.is_loaded(slot).then_some(slot)
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
        if let Some(s) = self.slots.get_mut(slot) {
            *s = None;
        }
    }

    pub(crate) fn insert(&mut self, s: Session) -> Option<usize> {
        let slot = self.slots.iter().position(Option::is_none)?;
        self.slots[slot] = Some(s);
        Some(slot)
    }

    pub(crate) fn loaded(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    pub(crate) fn loaded_handles(&self) -> Vec<u32> {
        (0..MAX_SESSIONS)
            .filter(|i| self.is_loaded(*i))
            .map(session_handle)
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

/// Checks the HMAC of a command authorization (Equation 17). An empty key accepts an empty HMAC.
pub(crate) fn verify_command_hmac(
    session: &Session,
    key: &[u8],
    cp_hash: &[u8; DIGEST],
    nonce_caller: &[u8],
    attributes: u8,
    hmac: &[u8],
) -> bool {
    if key.is_empty() && hmac.is_empty() {
        return true;
    }
    hmac_sha256_verify(
        key,
        &[cp_hash, nonce_caller, &session.nonce_tpm, &[attributes]],
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
    // `auditExclusive`/`auditReset` are only legal together with `audit`; audit itself needs the
    // session digest, which slice A does not keep.
    if attributes
        & (session_attr::AUDIT | session_attr::AUDIT_EXCLUSIVE | session_attr::AUDIT_RESET)
        != 0
    {
        return Err(Rc::ATTRIBUTES.in_session(num));
    }
    // Parameter encryption needs a symmetric algorithm in the session; slice A has none.
    if attributes & (session_attr::DECRYPT | session_attr::ENCRYPT) != 0 {
        return Err(Rc::SYMMETRIC.in_session(num));
    }
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
        let symmetric = p.u16()?;
        if symmetric != alg::NULL {
            // AES/XOR/... : parameter encryption is not implemented (the rest of the structure
            // is not parsed).
            return Err(Rc::SYMMETRIC.in_param(4));
        }
        p.set_param(5);
        let auth_hash = p.u16()?;
        p.finish()?;
        if auth_hash != alg::SHA256 {
            return Err(Rc::HASH.in_param(5));
        }
        if session_type != se::HMAC {
            return Err(Rc::VALUE.in_param(3));
        }
        if nonce_caller.len() < MIN_NONCE {
            return Err(Rc::SIZE.in_param(1));
        }
        if !encrypted_salt.is_empty() {
            // `tpmKey` is `TPM_RH_NULL`: the salt must be the Empty Buffer.
            return Err(Rc::VALUE.in_param(2));
        }
        let bind = inv.handles[1];
        let bound = if bind == handle::RH_NULL {
            None
        } else {
            let auth = self.permanent_auth(bind).ok_or(Rc::VALUE.in_handle(2))?;
            Some(BoundEntity { handle: bind, auth })
        };
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
            .insert(Session {
                nonce_tpm: nonce_tpm.clone(),
                key,
                bound,
            })
            .ok_or(Rc::SESSION_HANDLES)?;
        let mut handles = Writer::new();
        handles.u32(session_handle(slot));
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
                if !self.sessions.is_loaded(slot) {
                    return Err(Rc::HANDLE.in_param(1));
                }
                self.sessions.release(slot);
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
