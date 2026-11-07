//! Policy sessions and the policy commands (slice C2 of phase B, S7-23): `TPM2_PolicyRestart`,
//! `TPM2_PolicyPCR`, `TPM2_PolicyOR`, `TPM2_PolicyAuthValue`, `TPM2_PolicyCommandCode`,
//! `TPM2_PolicySecret` and `TPM2_PolicyGetDigest`; the check that a policy session authorizes a
//! command is [`Tpm::policy_use_check`], called by the dispatcher.
//!
//! A policy session (`TPM_SE_POLICY`) or trial session (`TPM_SE_TRIAL`) is started by
//! `TPM2_StartAuthSession` (`session.rs`) with a zero `policyDigest` (Part 3 v184, clause 11.1.1).
//! Every assertion extends the digest with `H(policyDigest || commandCode || data)`, SHA-256 being
//! the only session hash (Part 3, clauses 23.4-23.19). A trial session updates the digest without
//! validating (Part 3, clause 23.1) and cannot authorize. The policy context holds: the
//! `policyDigest`, `commandCode`, `cpHash`, `pcrUpdateCounter`, `isAuthValueNeeded` and
//! `isPasswordNeeded` (Part 1 v184, clause 17.7.8). `TPM2_PolicyRestart` returns all of it to the
//! initial state and keeps the session nonce.
//!
//! Using a policy session for an authorization (Part 3, clause 5.6, step 8 and Part 1, clause
//! 17.7): `cpHash` set must match the command (`TPM_RC_POLICY_FAIL`), `commandCode` set must match
//! (`TPM_RC_POLICY_CC`), `policyDigest` must equal the `authPolicy` of the entity
//! (`TPM_RC_POLICY_FAIL`), `pcrUpdateCounter` set must equal the current one
//! (`TPM_RC_PCR_CHANGED`); with `isAuthValueNeeded` the HMAC key is `sessionKey || authValue`
//! (an empty HMAC is refused), with `isPasswordNeeded` the `hmac` field is the authValue in clear,
//! otherwise the HMAC key is `sessionKey` alone (nothing to prove when it is empty). A policy
//! failure is not a dictionary-attack failure (Part 1, clause 17.8.3). An ADMIN-role authorization
//! with a policy session needs `TPM2_PolicyCommandCode` in the policy (Part 3, clause 5.6, step 6).
//!
//! Not implemented (answered `TPM_RC_COMMAND_CODE`): `PolicySigned`, `PolicyTicket`,
//! `PolicyLocality`, `PolicyNV`, `PolicyCounterTimer`, `PolicyPhysicalPresence`, `PolicyCpHash`,
//! `PolicyNameHash`, `PolicyDuplicationSelect`, `PolicyAuthorize`, `PolicyPassword`,
//! `PolicyNvWritten`, `PolicyTemplate`, `PolicyAuthorizeNV`, `PolicyCapability`,
//! `PolicyParameters`. `TPM2_PolicySecret` takes `expiration` 0 only (a non-zero value needs the
//! session timeout and the tickets: `TPM_RC_VALUE`, parameter 4) and always returns a NULL ticket
//! and an empty `timeout`. Slice C3 re-read Part 3 v184, clauses 23.2.2, 23.2.4, 23.2.5 and 23.4
//! and kept the restriction: the text fixes the timeout rule (`|expiration|` seconds after the
//! session start, or from the current `Time` for an empty `nonceTPM`; a timeout can only
//! shrink; `TPM_RC_EXPIRED` when it passed or the `timeEpoch` changed) and the ticket HMAC input
//! (`tag || cpHash || policyRef || authName || timeout || [timeEpoch] || [resetCount]`), but
//! leaves the `timeout` value, `timeEpoch` and `resetCount` encodings "implementation-specific",
//! and a negative `expiration` (ticket) is useful only with `TPM2_PolicyTicket`, which is not
//! implemented. Implementing it needs session start time, `timeEpoch` state and the ticket path
//! together; NOT VERIFIED against TPM 2.0 Library v185/v184 which of these encodings Windows
//! relies on (not before S7-30/S7-32).
//!
//! NOT VERIFIED against TPM 2.0 Library v185/v184 (Part 4 is not available): the numbers of the
//! handle and parameter modifiers of the codes below; `TPM_RC_VALUE` (handle 1/2) for a session
//! handle that is not of the policy type and `TPM_RC_REFERENCE_Hn` for a policy session that is
//! not loaded; `TPM_RC_ATTRIBUTES` (session number) for a trial session used as an authorization;
//! `TPM_RC_SIZE` for a `TPML_DIGEST` of fewer than two entries (Table 123 gives the minimum, the
//! code is the generic one) and for a `cpHashA` that is not one digest; `TPM_RC_MODE` (session
//! number) when a policy session authorizes `TPM2_PolicySecret` without `PolicyAuthValue`; the
//! order of the digest comparison and the `pcrUpdateCounter` check in `TPM2_PolicyPCR`
//! (the text gives the comparison first); `TPM_RC_AUTH_FAIL`/`TPM_RC_BAD_AUTH` for an empty HMAC
//! of a session that needs the authValue; the response HMAC of a policy session with
//! `isPasswordNeeded` (keyed with `sessionKey`); `TPM_RC_AUTH_UNAVAILABLE` for a policy session
//! on an NV index (the index attributes `POLICYREAD`/`POLICYWRITE` are refused at definition).

use sha2::{Digest, Sha256};

use crate::codes::{cc, handle, st};
use crate::crypto::DIGEST;
use crate::dispatch::{COMMANDS, Invocation, handle_name};
use crate::marshal::{Reader, TpmlDigest, TpmlPcrSelection, TpmtTicket, Writer, limits};
use crate::pcr::Pcrs;
use crate::response::{Output, Rc};
use crate::session::SessionKind;
use crate::state::Tpm;

/// The policy context of a session (Part 1, clause 17.7.8) for the SHA-256 session hash.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct PolicyState {
    pub digest: [u8; DIGEST],
    pub command_code: Option<u32>,
    pub cp_hash: Option<[u8; DIGEST]>,
    pub pcr_update_counter: Option<u32>,
    pub auth_value_needed: bool,
    pub password_needed: bool,
}

impl PolicyState {
    /// The initial state: a Zero Digest and no restriction.
    pub(crate) fn new() -> PolicyState {
        PolicyState {
            digest: [0; DIGEST],
            command_code: None,
            cp_hash: None,
            pcr_update_counter: None,
            auth_value_needed: false,
            password_needed: false,
        }
    }

    /// `policyDigest = H(policyDigest || parts...)` (Part 3, clauses 23.2.3 and 23.4-23.19).
    pub(crate) fn extend(&mut self, parts: &[&[u8]]) {
        let mut h = Sha256::new();
        h.update(self.digest);
        for p in parts {
            h.update(p);
        }
        self.digest = h.finalize().into();
    }
}

/// SHA-256 over the values of the selected PCR in ascending index order (Part 1, "Selecting
/// Multiple PCR"); the digest of the empty string when nothing is selected.
pub(crate) fn pcr_selection_digest(pcrs: &Pcrs, selection: &TpmlPcrSelection) -> [u8; DIGEST] {
    let mut h = Sha256::new();
    for sel in &selection.selections {
        for i in 0..limits::PCR_COUNT {
            if sel.selects(i)
                && let Some(v) = pcrs.value(i)
            {
                h.update(v);
            }
        }
    }
    h.finalize().into()
}

impl Tpm {
    /// `TPMI_SH_POLICY` validity of handle number `idx` (zero-based) of a command: a loaded policy
    /// or trial session.
    pub(crate) fn check_policy_session(&self, h: u32, idx: usize) -> Result<(), Rc> {
        let n = (idx + 1) as u8;
        if handle::handle_type(h) != handle::HT_POLICY_SESSION {
            return Err(Rc::VALUE.in_handle(n));
        }
        self.sessions
            .slot_of(h)
            .map(|_| ())
            .ok_or(Rc::reference_handle(idx))
    }

    fn policy_slot(&self, h: u32) -> Result<usize, Rc> {
        self.sessions.slot_of(h).ok_or(Rc::reference_handle(0))
    }

    fn policy_mut(&mut self, slot: usize) -> Result<(&mut PolicyState, SessionKind), Rc> {
        let s = self.sessions.get_mut(slot).ok_or(Rc::reference_handle(0))?;
        Ok((&mut s.policy, s.kind))
    }

    /// `TPM2_PolicyRestart` (Part 3, clause 11.2): the policy context returns to its initial
    /// state; `nonceTPM` is kept.
    pub(crate) fn cmd_policy_restart(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.finish()?;
        let slot = self.policy_slot(inv.handles[0])?;
        let (state, _) = self.policy_mut(slot)?;
        *state = PolicyState::new();
        Ok(Output::default())
    }

    /// `TPM2_PolicyPCR` (Part 3, clause 23.7).
    pub(crate) fn cmd_policy_pcr(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let pcr_digest = p.tpm2b(limits::MAX_DIGEST_SIZE)?;
        p.set_param(2);
        let pcrs = TpmlPcrSelection::unmarshal(p)?;
        p.finish()?;
        let slot = self.policy_slot(inv.handles[0])?;
        let current = pcr_selection_digest(&self.pcrs, &pcrs);
        let counter = self.pcrs.update_counter();
        let (state, kind) = self.policy_mut(slot)?;
        let mut selection = Writer::new();
        pcrs.marshal(&mut selection);
        let digest: &[u8] = if kind == SessionKind::Trial && !pcr_digest.is_empty() {
            // A trial session takes the values of the command and checks no PCR.
            pcr_digest
        } else {
            if kind == SessionKind::Policy {
                if !pcr_digest.is_empty() && pcr_digest != current {
                    return Err(Rc::VALUE.in_param(1));
                }
                match state.pcr_update_counter {
                    Some(c) if c != counter => return Err(Rc::PCR_CHANGED),
                    _ => state.pcr_update_counter = Some(counter),
                }
            }
            &current
        };
        state.extend(&[&cc::POLICY_PCR.to_be_bytes(), &selection.buf, digest]);
        Ok(Output::default())
    }

    /// `TPM2_PolicyOR` (Part 3, clause 23.6): the digest must be on the list (a trial session
    /// assumes it is); the new digest is `H(0...0 || TPM_CC_PolicyOR || digests)`.
    pub(crate) fn cmd_policy_or(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let list = TpmlDigest::unmarshal(p)?;
        p.finish()?;
        if list.digests.len() < 2 {
            return Err(Rc::SIZE.in_param(1));
        }
        let slot = self.policy_slot(inv.handles[0])?;
        let (state, kind) = self.policy_mut(slot)?;
        if kind == SessionKind::Policy && !list.digests.iter().any(|d| d[..] == state.digest[..]) {
            return Err(Rc::VALUE.in_param(1));
        }
        state.digest = [0; DIGEST];
        let joined = list.digests.concat();
        state.extend(&[&cc::POLICY_OR.to_be_bytes(), &joined]);
        Ok(Output::default())
    }

    /// `TPM2_PolicyAuthValue` (Part 3, clause 23.17): the authValue of the authorized entity must
    /// be proven by an HMAC when the session is used.
    pub(crate) fn cmd_policy_auth_value(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.finish()?;
        let slot = self.policy_slot(inv.handles[0])?;
        let (state, _) = self.policy_mut(slot)?;
        state.extend(&[&cc::POLICY_AUTH_VALUE.to_be_bytes()]);
        state.auth_value_needed = true;
        state.password_needed = false;
        Ok(Output::default())
    }

    /// `TPM2_PolicyCommandCode` (Part 3, clause 23.11).
    pub(crate) fn cmd_policy_command_code(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let code = p.u32()?;
        p.finish()?;
        let slot = self.policy_slot(inv.handles[0])?;
        let (state, kind) = self.policy_mut(slot)?;
        if kind == SessionKind::Policy {
            if !COMMANDS.iter().any(|c| c.code == code) {
                return Err(Rc::POLICY_CC.in_param(1));
            }
            match state.command_code {
                Some(c) if c != code => return Err(Rc::VALUE.in_param(1)),
                _ => state.command_code = Some(code),
            }
        }
        state.extend(&[&cc::POLICY_COMMAND_CODE.to_be_bytes(), &code.to_be_bytes()]);
        Ok(Output::default())
    }

    /// `TPM2_PolicyGetDigest` (Part 3, clause 23.19).
    pub(crate) fn cmd_policy_get_digest(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.finish()?;
        let slot = self.policy_slot(inv.handles[0])?;
        let (state, _) = self.policy_mut(slot)?;
        let mut w = Writer::new();
        w.tpm2b(&state.digest);
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }

    /// `TPM2_PolicySecret` (Part 3, clause 23.4). The authorization of `authHandle` was checked
    /// by the dispatcher (also for a trial session); this extends the digest with
    /// `PolicyUpdate(TPM_CC_PolicySecret, authEntity->Name, policyRef)`.
    pub(crate) fn cmd_policy_secret(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let nonce_tpm = p.tpm2b(limits::MAX_DIGEST_SIZE)?;
        p.set_param(2);
        let cp_hash_a = p.tpm2b(limits::MAX_DIGEST_SIZE)?;
        p.set_param(3);
        let policy_ref = p.tpm2b(limits::MAX_DIGEST_SIZE)?;
        p.set_param(4);
        let expiration = p.u32()?;
        p.finish()?;
        let slot = self.policy_slot(inv.handles[1])?;
        let name = handle_name(self, inv.handles[0]);
        let session = self.sessions.get(slot).ok_or(Rc::reference_handle(1))?;
        if !nonce_tpm.is_empty() && nonce_tpm != session.nonce_tpm {
            return Err(Rc::VALUE.in_param(1));
        }
        if expiration != 0 {
            // Needs the session timeout and the tickets (module comment).
            return Err(Rc::VALUE.in_param(4));
        }
        let cp: Option<[u8; DIGEST]> = if cp_hash_a.is_empty() {
            None
        } else {
            Some(cp_hash_a.try_into().map_err(|_| Rc::SIZE.in_param(2))?)
        };
        if let (Some(a), Some(b)) = (cp, session.policy.cp_hash)
            && a != b
        {
            return Err(Rc::CPHASH);
        }
        let (state, _) = self.policy_mut(slot)?;
        if cp.is_some() {
            state.cp_hash = cp;
        }
        // PolicyUpdate: two extensions (clause 23.2.3).
        state.extend(&[&cc::POLICY_SECRET.to_be_bytes(), &name]);
        state.extend(&[policy_ref]);
        let mut w = Writer::new();
        w.tpm2b(&[]);
        TpmtTicket {
            tag: st::AUTH_SECRET,
            hierarchy: handle::RH_NULL,
            digest: Vec::new(),
        }
        .marshal(&mut w);
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }

    /// The checks of Part 3, clause 5.6, step 8, for a policy session in `slot` that authorizes
    /// command `code` (cpHash `cp_hash`) an entity with `auth_policy`. `admin` is the ADMIN role,
    /// `n` the one-based session number. Returns `(isAuthValueNeeded, isPasswordNeeded)`.
    pub(crate) fn policy_use_check(
        &self,
        slot: usize,
        code: u32,
        cp_hash: &[u8; DIGEST],
        auth_policy: &[u8],
        admin: bool,
        n: u8,
    ) -> Result<(bool, bool), Rc> {
        let session = self.sessions.get(slot).ok_or(Rc::reference_session(0))?;
        if session.kind == SessionKind::Trial {
            return Err(Rc::ATTRIBUTES.in_session(n));
        }
        let st = &session.policy;
        // ADMIN needs `TPM2_PolicyCommandCode` in the policy (step 6).
        if admin && st.command_code.is_none() {
            return Err(Rc::POLICY_FAIL.in_session(n));
        }
        if let Some(c) = &st.cp_hash
            && c != cp_hash
        {
            return Err(Rc::POLICY_FAIL.in_session(n));
        }
        if let Some(c) = st.command_code
            && c != code
        {
            return Err(Rc::POLICY_CC.in_session(n));
        }
        if st.digest[..] != *auth_policy {
            return Err(Rc::POLICY_FAIL.in_session(n));
        }
        if let Some(c) = st.pcr_update_counter
            && c != self.pcrs.update_counter()
        {
            return Err(Rc::PCR_CHANGED);
        }
        // The authorization of `TPM2_PolicySecret` itself must prove the authValue (clause 23.4.1).
        if code == cc::POLICY_SECRET && !(st.auth_value_needed || st.password_needed) {
            return Err(Rc::MODE.in_session(n));
        }
        Ok((st.auth_value_needed, st.password_needed))
    }
}
