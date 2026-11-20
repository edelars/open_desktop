//! Policy sessions and the policy commands (S7-23).
//! The dispatcher validates policy authorization with [`Tpm::policy_use_check`].
//!
//! A policy session (`TPM_SE_POLICY`) or trial session (`TPM_SE_TRIAL`) is started by
//! `TPM2_StartAuthSession` (`session.rs`) with a zero `policyDigest` (Part 3 v184, clause 11.1.1).
//! SHA-256 is the only session hash. Most assertions extend the digest with
//! `H(policyDigest || commandCode || data)`; PolicyOR and PolicyAuthorize start from zero.
//! A trial session calculates the digest and cannot authorize (Part 3 §23.1).
//! The context keeps one deferred restriction for cpHash, nameHash, templateHash, or pHash.
//! `TPM2_PolicyRestart` resets the policy state and keeps the session nonce.
//!
//! Using a policy session for an authorization (Part 3, clause 5.6, step 8 and Part 1, clause
//! 17.7): a deferred hash restriction must match the command (`TPM_RC_POLICY_FAIL`),
//! `commandCode` set must match
//! (`TPM_RC_POLICY_CC`), `policyDigest` must equal the `authPolicy` of the entity
//! (`TPM_RC_POLICY_FAIL`), `pcrUpdateCounter` set must equal the current one
//! (`TPM_RC_PCR_CHANGED`), and `commandLocality` must allow the command's locality
//! (`TPM_RC_LOCALITY`); with `isAuthValueNeeded` the HMAC key is `sessionKey || authValue`
//! (an empty HMAC is refused), with `isPasswordNeeded` the `hmac` field is the authValue in clear,
//! otherwise the HMAC key is `sessionKey` alone (nothing to prove when it is empty). A policy
//! failure is not a dictionary-attack failure (Part 1, clause 17.8.3). An ADMIN-role authorization
//! with a policy session needs `TPM2_PolicyCommandCode` in the policy (Part 3, clause 5.6, step 6).
//!
//! `PolicySecret` supports session expiration and returns a NULL ticket and an empty
//! timeout. Negative expiration needs the reusable ticket path (`PolicyTicket`).
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
//! `isPasswordNeeded` (keyed with `sessionKey`).

use sha2::{Digest, Sha256};

use crate::codes::{alg, cc, handle, st};
use crate::crypto::DIGEST;
use crate::dispatch::{COMMANDS, Invocation, handle_name};
use crate::marshal::{MAX_DATA, Reader, TpmlDigest, TpmlPcrSelection, TpmtTicket, Writer, limits};
use crate::nv::{NvIndex, attr as nv_attr};
use crate::object::is_object_hierarchy;
use crate::pcr::Pcrs;
use crate::response::{Output, Rc};
use crate::session::{Session, SessionKind};
use crate::state::{TimePoint, Tpm};

/// The policy context of a session (Part 1, clause 17.7.8) for the SHA-256 session hash.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct PolicyState {
    pub digest: [u8; DIGEST],
    pub command_code: Option<u32>,
    pub hash_restriction: Option<PolicyHashRestriction>,
    /// Zero for unrestricted, 1..=31 for the locality mask, 32..=255 for an extended locality.
    pub command_locality: u8,
    pub pcr_update_counter: Option<u32>,
    pub deadline: Option<TimePoint>,
    pub nv_written: Option<bool>,
    pub auth_value_needed: bool,
    pub password_needed: bool,
}

/// Deferred hash restrictions share a single TPM policy-session field.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PolicyHashRestriction {
    CpHash([u8; DIGEST]),
    NameHash([u8; DIGEST]),
    TemplateHash([u8; DIGEST]),
    ParametersHash([u8; DIGEST]),
}

/// TPM_EO operations on same-width, big-endian inputs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Comparison {
    Eq,
    Ne,
    SignedGt,
    UnsignedGt,
    SignedLt,
    UnsignedLt,
    SignedGe,
    UnsignedGe,
    SignedLe,
    UnsignedLe,
    BitSet,
    BitClear,
}

impl Comparison {
    fn from_wire(operation: u16, param: u8) -> Result<Self, Rc> {
        match operation {
            0 => Ok(Self::Eq),
            1 => Ok(Self::Ne),
            2 => Ok(Self::SignedGt),
            3 => Ok(Self::UnsignedGt),
            4 => Ok(Self::SignedLt),
            5 => Ok(Self::UnsignedLt),
            6 => Ok(Self::SignedGe),
            7 => Ok(Self::UnsignedGe),
            8 => Ok(Self::SignedLe),
            9 => Ok(Self::UnsignedLe),
            10 => Ok(Self::BitSet),
            11 => Ok(Self::BitClear),
            _ => Err(Rc::VALUE.in_param(param)),
        }
    }

    fn holds(self, a: &[u8], b: &[u8]) -> bool {
        let signed = || {
            let negative = |bytes: &[u8]| bytes.first().is_some_and(|byte| byte & 0x80 != 0);
            negative(b).cmp(&negative(a)).then_with(|| a.cmp(b))
        };
        match self {
            Self::Eq => a == b,
            Self::Ne => a != b,
            Self::SignedGt => signed().is_gt(),
            Self::UnsignedGt => a > b,
            Self::SignedLt => signed().is_lt(),
            Self::UnsignedLt => a < b,
            Self::SignedGe => !signed().is_lt(),
            Self::UnsignedGe => a >= b,
            Self::SignedLe => !signed().is_gt(),
            Self::UnsignedLe => a <= b,
            Self::BitSet => a.iter().zip(b).all(|(a, b)| a & b == *b),
            Self::BitClear => a.iter().zip(b).all(|(a, b)| a & b == 0),
        }
    }
}

/// Shared wire parameters and argument digest of the immediate comparison assertions.
struct ComparisonArgs<'a> {
    operand: &'a [u8],
    offset: u16,
    operation: u16,
    comparison: Comparison,
}

impl<'a> ComparisonArgs<'a> {
    fn unmarshal(p: &mut Reader<'a>) -> Result<Self, Rc> {
        p.set_param(1);
        let operand = p.tpm2b(limits::MAX_DIGEST_SIZE)?;
        p.set_param(2);
        let offset = p.u16()?;
        p.set_param(3);
        let operation = p.u16()?;
        let comparison = Comparison::from_wire(operation, 3)?;
        p.finish()?;
        Ok(Self {
            operand,
            offset,
            operation,
            comparison,
        })
    }

    fn digest(&self) -> [u8; DIGEST] {
        let mut args = Sha256::new();
        args.update(self.operand);
        args.update(self.offset.to_be_bytes());
        args.update(self.operation.to_be_bytes());
        args.finalize().into()
    }
}

impl PolicyState {
    /// The initial state: a Zero Digest and no restriction.
    pub(crate) fn new() -> PolicyState {
        PolicyState {
            digest: [0; DIGEST],
            command_code: None,
            hash_restriction: None,
            command_locality: 0,
            pcr_update_counter: None,
            deadline: None,
            nv_written: None,
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

    fn restrict_deadline(&mut self, limit: Option<TimePoint>) {
        if let Some(limit) = limit
            && self
                .deadline
                .is_none_or(|old| old.epoch == limit.epoch && old.ms > limit.ms)
        {
            // A restriction from an older epoch remains expired even after a nonce roll.
            self.deadline = Some(limit);
        }
    }

    fn restrict_cp_hash(&mut self, hash: [u8; DIGEST]) -> Result<(), Rc> {
        let restriction = PolicyHashRestriction::CpHash(hash);
        if self.hash_restriction.is_some_and(|old| old != restriction) {
            return Err(Rc::CPHASH);
        }
        self.hash_restriction = Some(restriction);
        Ok(())
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
    pub(crate) fn check_policy_session(
        &self,
        h: u32,
        idx: usize,
    ) -> Result<(usize, SessionKind), Rc> {
        let n = (idx + 1) as u8;
        if handle::handle_type(h) != handle::HT_POLICY_SESSION {
            return Err(Rc::VALUE.in_handle(n));
        }
        self.sessions
            .slot_of(h)
            .and_then(|slot| self.sessions.get(slot).map(|session| (slot, session.kind)))
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

    /// `TPM2_PolicyCounterTimer` (Part 3 v185, §23.10).
    pub(crate) fn cmd_policy_counter_timer(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        let args = ComparisonArgs::unmarshal(p)?;

        let slot = self.policy_slot(inv.handles[0])?;
        let session = self.sessions.get(slot).ok_or(Rc::reference_handle(0))?;
        let mut deadline = None;
        if session.kind != SessionKind::Trial {
            if args.comparison == Comparison::UnsignedLt
                && args.offset == 0
                && let Ok(seconds) = <[u8; 8]>::try_from(args.operand)
            {
                let seconds = u64::from_be_bytes(seconds);
                if seconds > i32::MAX as u64 {
                    return Err(Rc::VALUE.in_param(1));
                }
                let start = session.nonce_time.ok_or(Rc::EXPIRED)?;
                let limit = TimePoint {
                    epoch: start.epoch,
                    ms: start.ms.saturating_add(seconds * 1000),
                };
                if limit.has_passed(self.time_point()) {
                    return Err(Rc::EXPIRED);
                }
                deadline = Some(limit);
            } else {
                let mut time = Writer::new();
                self.time_info().marshal(&mut time);
                let at_offset = time
                    .buf
                    .get(usize::from(args.offset)..)
                    .ok_or(Rc::VALUE.in_param(2))?;
                let value = at_offset
                    .get(..args.operand.len())
                    .ok_or(Rc::RANGE.in_param(1))?;
                if !args.comparison.holds(value, args.operand) {
                    return Err(Rc::POLICY);
                }
            }
        }
        let (state, _) = self.policy_mut(slot)?;
        state.restrict_deadline(deadline);
        state.extend(&[&cc::POLICY_COUNTER_TIMER.to_be_bytes(), &args.digest()]);
        Ok(Output::default())
    }

    /// Both NV policy assertions require read authorization but skip the data checks in trial mode.
    fn nv_policy_input(&self, inv: &Invocation) -> Result<(usize, SessionKind, &NvIndex), Rc> {
        let (slot, kind) = self.check_policy_session(inv.handles[2], 2)?;
        let index = self
            .persistent
            .nv
            .get(inv.handles[1])
            .ok_or(Rc::HANDLE.in_handle(2))?;
        if kind != SessionKind::Trial && index.attributes & nv_attr::READLOCKED != 0 {
            return Err(Rc::NV_LOCKED);
        }
        Ok((slot, kind, index))
    }

    /// `TPM2_PolicyNV` (Part 3 v185, §23.9). The read and comparison are immediate.
    pub(crate) fn cmd_policy_nv(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        let args = ComparisonArgs::unmarshal(p)?;
        let (slot, kind, index) = self.nv_policy_input(inv)?;
        if kind != SessionKind::Trial {
            let offset = usize::from(args.offset);
            let value = index
                .data
                .get(offset..offset + args.operand.len())
                .ok_or(Rc::NV_RANGE)?;
            if !args.comparison.holds(value, args.operand) {
                return Err(Rc::POLICY);
            }
        }
        let name = index.name();
        let (state, _) = self.policy_mut(slot)?;
        state.extend(&[&cc::POLICY_NV.to_be_bytes(), &args.digest(), &name]);
        Ok(Output::default())
    }

    /// `TPM2_PolicyAuthorizeNV` (Part 3 v185, §23.22). The NV index starts with a raw
    /// `TPMT_HA` and may contain additional bytes following that value.
    pub(crate) fn cmd_policy_authorize_nv(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.finish()?;
        let (slot, kind, index) = self.nv_policy_input(inv)?;
        if kind != SessionKind::Trial {
            let mut data = Reader::new(&index.data);
            // SHA-256 is the only supported authHash for policy sessions.
            if data.u16().map_err(|rc| rc.in_handle(2))? != alg::SHA256 {
                return Err(Rc::HASH.in_handle(2));
            }
            let approved = data.bytes(DIGEST).map_err(|rc| rc.in_handle(2))?;
            let session = self.sessions.get(slot).ok_or(Rc::reference_handle(2))?;
            if approved != session.policy.digest {
                return Err(Rc::VALUE.in_handle(2));
            }
        }
        let name = index.name();
        let (state, _) = self.policy_mut(slot)?;
        state.digest = [0; DIGEST];
        state.extend(&[&cc::POLICY_AUTHORIZE_NV.to_be_bytes(), &name]);
        Ok(Output::default())
    }

    /// `TPM2_PolicyNvWritten` (Part 3 v185, §23.20) defers the comparison until authorization.
    pub(crate) fn cmd_policy_nv_written(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let value = p.u8()?;
        let written = match value {
            0 => false,
            1 => true,
            _ => return Err(Rc::VALUE.in_param(1)),
        };
        p.finish()?;
        let slot = self.policy_slot(inv.handles[0])?;
        let (state, kind) = self.policy_mut(slot)?;
        if kind != SessionKind::Trial {
            if state.nv_written.is_some_and(|old| old != written) {
                return Err(Rc::VALUE.in_param(1));
            }
            state.nv_written = Some(written);
        }
        state.extend(&[&cc::POLICY_NV_WRITTEN.to_be_bytes(), &[value]]);
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

    /// `TPM2_PolicyAuthValue` and `TPM2_PolicyPassword` (Part 3, clauses 23.17–18).
    /// They extend the same digest, with the last assertion selecting HMAC or password.
    pub(crate) fn cmd_policy_auth_value(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
        password: bool,
    ) -> Result<Output, Rc> {
        p.finish()?;
        let slot = self.policy_slot(inv.handles[0])?;
        let (state, _) = self.policy_mut(slot)?;
        state.extend(&[&cc::POLICY_AUTH_VALUE.to_be_bytes()]);
        state.auth_value_needed = !password;
        state.password_needed = password;
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

    /// `TPM2_PolicyCpHash` (Part 3, clause 23.13).
    pub(crate) fn cmd_policy_cp_hash(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let cp_hash: [u8; DIGEST] = p
            .tpm2b(limits::MAX_DIGEST_SIZE)?
            .try_into()
            .map_err(|_| Rc::SIZE.in_param(1))?;
        p.finish()?;
        let slot = self.policy_slot(inv.handles[0])?;
        let (state, kind) = self.policy_mut(slot)?;
        if kind == SessionKind::Policy {
            state.restrict_cp_hash(cp_hash)?;
        }
        state.extend(&[&cc::POLICY_CP_HASH.to_be_bytes(), &cp_hash]);
        Ok(Output::default())
    }

    /// Deferred assertions over command Names, a creation template or parameters
    /// (Part 3 §§23.14, 23.21, 23.24).
    pub(crate) fn cmd_policy_hash_restriction(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
        code: u32,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let input = p.tpm2b(limits::MAX_DIGEST_SIZE)?;
        p.finish()?;
        let slot = self.policy_slot(inv.handles[0])?;
        let (state, kind) = self.policy_mut(slot)?;
        if kind == SessionKind::Policy
            && let Some(existing) = state.hash_restriction
        {
            if let (cc::POLICY_TEMPLATE, PolicyHashRestriction::TemplateHash(old)) =
                (code, existing)
                && old != input
            {
                return Err(Rc::VALUE.in_param(1));
            }
            return Err(Rc::CPHASH);
        }
        let digest: [u8; DIGEST] = input.try_into().map_err(|_| Rc::SIZE.in_param(1))?;
        let restriction = match code {
            cc::POLICY_NAME_HASH => PolicyHashRestriction::NameHash(digest),
            cc::POLICY_TEMPLATE => PolicyHashRestriction::TemplateHash(digest),
            cc::POLICY_PARAMETERS => PolicyHashRestriction::ParametersHash(digest),
            _ => return Err(Rc::COMMAND_CODE),
        };
        if kind == SessionKind::Policy {
            state.hash_restriction = Some(restriction);
        }
        state.extend(&[&code.to_be_bytes(), &digest]);
        Ok(Output::default())
    }

    /// `TPM2_PolicyLocality` (Part 3, clause 23.8).
    pub(crate) fn cmd_policy_locality(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let locality = p.u8()?;
        p.finish()?;
        let slot = self.policy_slot(inv.handles[0])?;
        let (state, kind) = self.policy_mut(slot)?;
        if kind == SessionKind::Policy {
            let old = state.command_locality;
            let allowed = if old == 0 || old == locality {
                locality
            } else if old < 32 && locality < 32 {
                old & locality
            } else {
                return Err(Rc::RANGE.in_param(1));
            };
            if allowed == 0 {
                return Err(Rc::RANGE.in_param(1));
            }
            state.command_locality = allowed;
        }
        state.extend(&[&cc::POLICY_LOCALITY.to_be_bytes(), &[locality]]);
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
        let expiration = p.u32()? as i32;
        p.finish()?;
        let slot = self.policy_slot(inv.handles[1])?;
        let name = handle_name(self, inv.handles[0]);
        let session = self.sessions.get(slot).ok_or(Rc::reference_handle(1))?;
        let (deadline, cp_hash) = if session.kind == SessionKind::Trial {
            (None, None)
        } else {
            if !nonce_tpm.is_empty() && nonce_tpm != session.nonce_tpm {
                return Err(Rc::VALUE.in_param(1));
            }
            if expiration < 0 {
                // Negative expiration also requires issuing a reusable authorization ticket.
                return Err(Rc::VALUE.in_param(4));
            }
            let deadline = self.policy_assertion_deadline(session, nonce_tpm, expiration as u32)?;
            let cp_hash = Self::policy_assertion_cp_hash(&session.policy, cp_hash_a)?;
            (deadline, cp_hash)
        };
        let (state, _) = self.policy_mut(slot)?;
        if let Some(hash) = cp_hash {
            state.hash_restriction = Some(PolicyHashRestriction::CpHash(hash));
        }
        state.restrict_deadline(deadline);
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

    /// Part 3 v185 §23.2.2. An empty nonce makes the expiration an absolute TPM Time.
    fn policy_assertion_deadline(
        &self,
        session: &Session,
        nonce_tpm: &[u8],
        seconds: u32,
    ) -> Result<Option<TimePoint>, Rc> {
        if seconds == 0 {
            return Ok(None);
        }
        let origin = session.nonce_time.ok_or(Rc::EXPIRED)?;
        let base_ms = if nonce_tpm.is_empty() { 0 } else { origin.ms };
        let limit = TimePoint {
            epoch: origin.epoch,
            ms: base_ms.saturating_add(u64::from(seconds) * 1000),
        };
        if limit.has_passed(self.time_point()) {
            return Err(Rc::EXPIRED);
        }
        Ok(Some(limit))
    }

    fn policy_assertion_cp_hash(
        state: &PolicyState,
        cp_hash: &[u8],
    ) -> Result<Option<[u8; DIGEST]>, Rc> {
        if cp_hash.is_empty() {
            return Ok(None);
        }
        if state.hash_restriction.is_some_and(
            |restriction| !matches!(restriction, PolicyHashRestriction::CpHash(old) if old == cp_hash),
        ) {
            return Err(Rc::CPHASH);
        }
        Ok(Some(cp_hash.try_into().map_err(|_| Rc::SIZE.in_param(2))?))
    }

    /// `TPM2_PolicyAuthorize` with a legacy VerifySignature ticket (Part 3 §23.16).
    pub(crate) fn cmd_policy_authorize(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let approved = p.tpm2b(limits::MAX_DIGEST_SIZE)?;
        p.set_param(2);
        let policy_ref = p.tpm2b(limits::MAX_DIGEST_SIZE)?;
        p.set_param(3);
        let key_name = p.tpm2b(MAX_DATA)?;
        p.set_param(4);
        if p.u16()? != st::VERIFIED {
            return Err(p.err(Rc::TAG));
        }
        let hierarchy = p.u32()?;
        if !is_object_hierarchy(hierarchy) {
            return Err(p.err(Rc::VALUE));
        }
        let ticket = p.tpm2b(limits::MAX_DIGEST_SIZE)?;
        p.finish()?;

        let (name_alg, name_digest) = key_name.split_at_checked(2).ok_or(Rc::SIZE.in_param(3))?;
        if name_alg != alg::SHA256.to_be_bytes() {
            return Err(Rc::HASH.in_param(3));
        }
        if name_digest.len() != DIGEST {
            return Err(Rc::SIZE.in_param(3));
        }

        let slot = self.policy_slot(inv.handles[0])?;
        let session = self.sessions.get(slot).ok_or(Rc::reference_handle(0))?;
        if session.kind == SessionKind::Policy {
            if approved != session.policy.digest {
                return Err(Rc::VALUE.in_param(1));
            }
            let mut signed = Sha256::new();
            signed.update(approved);
            signed.update(policy_ref);
            if !self.valid_verified_ticket(key_name, &signed.finalize(), hierarchy, ticket) {
                return Err(Rc::POLICY);
            }
        }
        let (state, _) = self.policy_mut(slot)?;
        state.digest = [0; DIGEST];
        state.extend(&[&cc::POLICY_AUTHORIZE.to_be_bytes(), key_name]);
        state.extend(&[policy_ref]);
        Ok(Output::default())
    }

    /// The checks of Part 3, clause 5.6, step 8, for a policy session in `slot` that authorizes
    /// a `(command code, authorized handle)` pair with `auth_policy`.
    /// `matches_hash` checks a restriction
    /// against the authorized command's actual content. `admin` is the ADMIN role,
    /// `n` the one-based session number. Returns `(isAuthValueNeeded, isPasswordNeeded)`.
    pub(crate) fn policy_use_check(
        &self,
        slot: usize,
        (code, entity): (u32, u32),
        matches_hash: impl Fn(PolicyHashRestriction) -> bool,
        auth_policy: &[u8],
        admin: bool,
        n: u8,
    ) -> Result<(bool, bool), Rc> {
        let session = self.sessions.get(slot).ok_or(Rc::reference_session(0))?;
        if session.kind == SessionKind::Trial {
            return Err(Rc::ATTRIBUTES.in_session(n));
        }
        let st = &session.policy;
        // ADMIN needs `TPM2_PolicyCommandCode` before the step 8 policy checks.
        if admin && st.command_code.is_none() {
            return Err(Rc::POLICY_FAIL.in_session(n));
        }
        if st
            .deadline
            .is_some_and(|end| end.has_passed(self.time_point()))
        {
            return Err(Rc::EXPIRED.in_session(n));
        }
        if let Some(hash) = st.hash_restriction
            && !matches!(hash, PolicyHashRestriction::TemplateHash(_))
            && !matches_hash(hash)
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
        // The dispatcher only admits commands at locality 0.
        let locality = st.command_locality;
        if locality != 0 && (locality >= 32 || locality & 1 == 0) {
            return Err(Rc::LOCALITY);
        }
        if let Some(hash @ PolicyHashRestriction::TemplateHash(_)) = st.hash_restriction
            && !matches_hash(hash)
        {
            return Err(Rc::POLICY_FAIL.in_session(n));
        }
        if let Some(written) = st.nv_written
            && self
                .persistent
                .nv
                .get(entity)
                .is_none_or(|index| (index.attributes & nv_attr::WRITTEN != 0) != written)
        {
            return Err(Rc::POLICY_FAIL.in_session(n));
        }
        // The authorization of `TPM2_PolicySecret` itself must prove the authValue (clause 23.4.1).
        if code == cc::POLICY_SECRET && !(st.auth_value_needed || st.password_needed) {
            return Err(Rc::MODE.in_session(n));
        }
        Ok((st.auth_value_needed, st.password_needed))
    }
}
