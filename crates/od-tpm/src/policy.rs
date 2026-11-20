//! Policy sessions and the policy commands (S7-23).
//! The dispatcher validates policy authorization with [`Tpm::policy_use_check`].
//!
//! A policy session (`TPM_SE_POLICY`) or trial session (`TPM_SE_TRIAL`) is started by
//! `TPM2_StartAuthSession` (`session.rs`) with a zero `policyDigest` (Part 3 v184, clause 11.1.1).
//! The session hash is the `authHash` of the session (SHA-256, SHA-384 or SHA-512): the policy
//! digest, the operands, the cpHash, nameHash and PolicyPCR digests and every comparison with the
//! `authPolicy` of an entity are one digest of that hash long. Most assertions extend the digest
//! with `H(policyDigest || commandCode || data)`; PolicyOR and PolicyAuthorize start from zero.
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
//! A negative `PolicySecret` expiration can issue a reusable authorization ticket.
//! `PolicyTicket` authenticates the ticket and reuses its original assertion. The authorization
//! tickets (PolicySecret, PolicySigned, PolicyTicket) and the verified tickets of PolicyAuthorize are
//! HMAC-SHA-256 under the hierarchy proof whatever the hash of the session (as the context
//! integrity values), so their format does not depend on the `authHash`.
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

use crate::codes::{alg, cc, handle, object_attr, st};
use crate::crypto::{HashAlg, TpmDigest, hash, hmac_sha256, hmac_sha256_verify};
use crate::dispatch::{COMMANDS, Invocation, handle_name};
use crate::marshal::{
    PublicParams, Reader, Scheme, TpmlDigest, TpmlPcrSelection, TpmtSignature, TpmtTicket, Writer,
    digest_size, limits,
};
use crate::nv::{NvIndex, attr as nv_attr};
use crate::object::is_object_hierarchy;
use crate::object_cmd::VerifiedTicket;
use crate::pcr::Pcrs;
use crate::response::{Output, Rc};
use crate::session::{Session, SessionKind};
use crate::state::{TimePoint, Tpm};

/// The top bit of the private eight-octet timeout marks an assertion without nonceTPM.
const RESET_BOUND_TIMEOUT: u64 = 1 << 63;
/// `TPM2B_NAME` limit: `sizeof(TPMU_NAME)`, which is `sizeof(TPMT_HA)` (Part 2, Table 80 `TPMU_NAME`
/// and Table 81 `TPM2B_NAME`): the 2-octet algorithm identifier and the digest of the largest hash,
/// SHA-512 (64 octets), so 66. A SHA-384 Name has 50 octets and a SHA-256 Name 34, both within it.
const MAX_NAME_SIZE: usize = 2 + limits::MAX_HASH_SIZE;

/// The policy context of a session (Part 1, clause 17.7.8) for the session hash `hash`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct PolicyState {
    /// The `authHash` of the session: the size of every digest below and the hash of `extend`.
    pub hash: HashAlg,
    pub digest: TpmDigest,
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
// The shared `Hash` postfix mirrors the TPM command names (CpHash, NameHash, ...); renaming would touch dispatch.rs, context.rs and tests_c2.rs.
#[allow(clippy::enum_variant_names)]
pub(crate) enum PolicyHashRestriction {
    CpHash(TpmDigest),
    NameHash(TpmDigest),
    TemplateHash(TpmDigest),
    ParametersHash(TpmDigest),
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
    /// `limit` is the digest size of the session hash.
    fn unmarshal(p: &mut Reader<'a>, limit: usize) -> Result<Self, Rc> {
        p.set_param(1);
        let operand = p.tpm2b(limit)?;
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

    fn digest(&self, alg: HashAlg) -> TpmDigest {
        hash(
            alg,
            &[
                self.operand,
                &self.offset.to_be_bytes(),
                &self.operation.to_be_bytes(),
            ],
        )
    }
}

struct AuthorizationParameters<'a> {
    nonce: &'a [u8],
    cp_hash: &'a [u8],
    policy_ref: &'a [u8],
    expiration: i32,
}

impl<'a> AuthorizationParameters<'a> {
    /// `limit` is the digest size of the session hash.
    fn unmarshal(p: &mut Reader<'a>, limit: usize) -> Result<Self, Rc> {
        p.set_param(1);
        let nonce = p.tpm2b(limit)?;
        p.set_param(2);
        let cp_hash = p.tpm2b(limit)?;
        p.set_param(3);
        let policy_ref = p.tpm2b(limit)?;
        p.set_param(4);
        let expiration = p.u32()? as i32;
        Ok(Self {
            nonce,
            cp_hash,
            policy_ref,
            expiration,
        })
    }
}

struct AuthorizationRestrictions {
    deadline: Option<TimePoint>,
    cp_hash: Option<TpmDigest>,
}

impl PolicyState {
    /// The initial state of a session of hash `hash`: a Zero Digest of its size and no
    /// restriction.
    pub(crate) fn new(hash: HashAlg) -> PolicyState {
        PolicyState {
            hash,
            digest: TpmDigest::zeros(hash.size()),
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
        let mut all: Vec<&[u8]> = Vec::with_capacity(parts.len() + 1);
        all.push(self.digest.as_slice());
        all.extend_from_slice(parts);
        let next = hash(self.hash, &all);
        self.digest = next;
    }

    /// The policyDigest returns to the Zero Digest (PolicyOR, PolicyAuthorize, PolicyAuthorizeNV).
    fn reset_digest(&mut self) {
        self.digest = TpmDigest::zeros(self.hash.size());
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

    fn restrict_cp_hash(&mut self, cp_hash: TpmDigest) -> Result<(), Rc> {
        let restriction = PolicyHashRestriction::CpHash(cp_hash);
        if self.hash_restriction.is_some_and(|old| old != restriction) {
            return Err(Rc::CPHASH);
        }
        self.hash_restriction = Some(restriction);
        Ok(())
    }

    fn apply_authorization(
        &mut self,
        code: u32,
        name: &[u8],
        policy_ref: &[u8],
        deadline: Option<TimePoint>,
        cp_hash: Option<TpmDigest>,
    ) {
        if let Some(cp_hash) = cp_hash {
            self.hash_restriction = Some(PolicyHashRestriction::CpHash(cp_hash));
        }
        self.restrict_deadline(deadline);
        // PolicyUpdate (Part 3, §23.2.3) has two extensions, including an empty policyRef.
        self.extend(&[&code.to_be_bytes(), name]);
        self.extend(&[policy_ref]);
    }
}

/// The digest with `alg` over the values of the selected PCR in ascending index order (Part 1,
/// "Selecting Multiple PCR"); the digest of the empty string when nothing is selected
/// (`PCRComputeCurrentDigest` hashes whatever the selection holds, also nothing). `TPM2_Quote`
/// passes the hash of the signing scheme (Part 3, clause 18.4.1; the reference takes
/// `inScheme.details.any.hashAlg` after `CryptSelectSignScheme`); a creation record uses the
/// `nameAlg` of the object instead (`creation_pcr_digest`).
pub(crate) fn pcr_selection_digest_with(
    alg: HashAlg,
    pcrs: &Pcrs,
    selection: &TpmlPcrSelection,
) -> TpmDigest {
    pcrs.selection_digest_with(alg, selection)
        .unwrap_or_else(|| hash(alg, &[]))
}

/// The hash that digests the authorization message of `TPM2_PolicySigned` (the `sigHashAlg` of the
/// signature, `None` for the null signature of a trial session). A SHA-256 signature is accepted
/// by a session of any hash, as before hash agility; a SHA-384 or SHA-512 signature only by a
/// session of the same hash, otherwise `TPM_RC_HASH` (the caller sets parameter 5).
fn signature_hash(signature: &TpmtSignature, session: HashAlg) -> Result<Option<HashAlg>, Rc> {
    match signature {
        TpmtSignature::Rsassa { hash, .. } | TpmtSignature::Ecdsa { hash, .. } => {
            match HashAlg::from_alg(*hash) {
                Some(HashAlg::Sha256) => Ok(Some(HashAlg::Sha256)),
                Some(wide) if wide == session => Ok(Some(wide)),
                _ => Err(Rc::HASH),
            }
        }
        _ => Ok(None),
    }
}

impl Tpm {
    /// The `authHash` of the session `h`, SHA-256 for a handle that is not a loaded session: the
    /// command then fails on its handle after its parameters were unmarshaled, as it always did.
    fn session_hash(&self, h: u32) -> HashAlg {
        self.sessions
            .slot_of(h)
            .and_then(|slot| self.sessions.get(slot))
            .map_or(HashAlg::Sha256, |session| session.auth_hash)
    }

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
        *state = PolicyState::new(state.hash);
        Ok(Output::default())
    }

    /// `TPM2_PolicyCounterTimer` (Part 3 v185, §23.10).
    pub(crate) fn cmd_policy_counter_timer(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        let alg = self.session_hash(inv.handles[0]);
        let args = ComparisonArgs::unmarshal(p, alg.size())?;

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
        state.extend(&[
            &cc::POLICY_COUNTER_TIMER.to_be_bytes(),
            args.digest(alg).as_slice(),
        ]);
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
        let alg = self.session_hash(inv.handles[2]);
        let args = ComparisonArgs::unmarshal(p, alg.size())?;
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
        state.extend(&[
            &cc::POLICY_NV.to_be_bytes(),
            args.digest(alg).as_slice(),
            &name,
        ]);
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
            let session = self.sessions.get(slot).ok_or(Rc::reference_handle(2))?;
            let session_hash = session.auth_hash;
            let mut data = Reader::new(&index.data);
            // The `TPMT_HA` must carry the hash of the session: its digest is that long.
            if data.u16().map_err(|rc| rc.in_handle(2))? != session_hash.alg() {
                return Err(Rc::HASH.in_handle(2));
            }
            let approved = data
                .bytes(session_hash.size())
                .map_err(|rc| rc.in_handle(2))?;
            if approved != session.policy.digest.as_slice() {
                return Err(Rc::VALUE.in_handle(2));
            }
        }
        let name = index.name();
        let (state, _) = self.policy_mut(slot)?;
        state.reset_digest();
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
        // pcrDigest is one digest of the session hash (the size limit) or empty.
        let alg = self.session_hash(inv.handles[0]);
        p.set_param(1);
        let pcr_digest = p.tpm2b(alg.size())?;
        p.set_param(2);
        let mut pcrs = TpmlPcrSelection::unmarshal(p)?;
        p.finish()?;
        let (slot, kind) = self.check_policy_session(inv.handles[0], 0)?;
        if !pcr_digest.is_empty() && pcr_digest.len() != alg.size() {
            return Err(Rc::VALUE.in_param(1));
        }
        if kind == SessionKind::Policy {
            self.pcrs.filter_selection(&mut pcrs);
        }
        let current;
        let digest: &[u8] = if kind == SessionKind::Trial && !pcr_digest.is_empty() {
            // A trial policy can target a different allocation. Keep the original selectors.
            pcr_digest
        } else {
            // The PCR values come from the selected banks (SHA-256 or SHA-384) and are hashed
            // with the hash of the session.
            current = self
                .pcrs
                .selection_digest_with(alg, &pcrs)
                .unwrap_or_else(|| hash(alg, &[]));
            current.as_slice()
        };
        let counter = self.pcrs.update_counter();
        let (state, _) = self.policy_mut(slot)?;
        if kind == SessionKind::Policy {
            if !pcr_digest.is_empty() && pcr_digest != digest {
                return Err(Rc::VALUE.in_param(1));
            }
            match state.pcr_update_counter {
                Some(c) if c != counter => return Err(Rc::PCR_CHANGED),
                _ => state.pcr_update_counter = Some(counter),
            }
        }
        let mut selection = Writer::new();
        pcrs.marshal(&mut selection);
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
        let alg = self.session_hash(inv.handles[0]);
        let list = TpmlDigest::unmarshal_with_limit(p, alg.size())?;
        p.finish()?;
        if list.digests.len() < 2 {
            return Err(Rc::SIZE.in_param(1));
        }
        let slot = self.policy_slot(inv.handles[0])?;
        let (state, kind) = self.policy_mut(slot)?;
        if kind == SessionKind::Policy
            && !list
                .digests
                .iter()
                .any(|d| d.as_slice() == state.digest.as_slice())
        {
            return Err(Rc::VALUE.in_param(1));
        }
        state.reset_digest();
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
        let alg = self.session_hash(inv.handles[0]);
        p.set_param(1);
        let cp_hash =
            TpmDigest::exact(p.tpm2b(alg.size())?, alg.size()).ok_or(Rc::SIZE.in_param(1))?;
        p.finish()?;
        let slot = self.policy_slot(inv.handles[0])?;
        let (state, kind) = self.policy_mut(slot)?;
        if kind == SessionKind::Policy {
            state.restrict_cp_hash(cp_hash)?;
        }
        state.extend(&[&cc::POLICY_CP_HASH.to_be_bytes(), cp_hash.as_slice()]);
        Ok(Output::default())
    }

    /// `TPM2_PolicyDuplicationSelect` (Part 3 v185 §23.15).
    pub(crate) fn cmd_policy_duplication_select(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        // The Names can describe another TPM when calculating a trial policy.
        p.set_param(1);
        let object_name = p.tpm2b(MAX_NAME_SIZE)?;
        p.set_param(2);
        let parent_name = p.tpm2b(MAX_NAME_SIZE)?;
        p.set_param(3);
        let include = p.u8()?;
        if include > 1 {
            return Err(Rc::VALUE.in_param(3));
        }
        p.finish()?;
        let slot = self.policy_slot(inv.handles[0])?;
        let (state, kind) = self.policy_mut(slot)?;
        if kind == SessionKind::Policy {
            if state.hash_restriction.is_some() {
                return Err(Rc::CPHASH);
            }
            if state.command_code.is_some_and(|code| code != cc::DUPLICATE) {
                return Err(Rc::POLICY_CC);
            }
            let names = hash(state.hash, &[object_name, parent_name]);
            state.hash_restriction = Some(PolicyHashRestriction::NameHash(names));
            state.command_code = Some(cc::DUPLICATE);
        }
        let selected_object = if include == 1 { object_name } else { &[] };
        state.extend(&[
            &cc::POLICY_DUPLICATION_SELECT.to_be_bytes(),
            selected_object,
            parent_name,
            &[include],
        ]);
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
        let alg = self.session_hash(inv.handles[0]);
        p.set_param(1);
        let input = p.tpm2b(alg.size())?;
        p.finish()?;
        let slot = self.policy_slot(inv.handles[0])?;
        let (state, kind) = self.policy_mut(slot)?;
        if kind == SessionKind::Policy
            && let Some(existing) = state.hash_restriction
        {
            if let (cc::POLICY_TEMPLATE, PolicyHashRestriction::TemplateHash(old)) =
                (code, existing)
                && old.as_slice() != input
            {
                return Err(Rc::VALUE.in_param(1));
            }
            return Err(Rc::CPHASH);
        }
        let digest = TpmDigest::exact(input, alg.size()).ok_or(Rc::SIZE.in_param(1))?;
        let restriction = match code {
            cc::POLICY_NAME_HASH => PolicyHashRestriction::NameHash(digest),
            cc::POLICY_TEMPLATE => PolicyHashRestriction::TemplateHash(digest),
            cc::POLICY_PARAMETERS => PolicyHashRestriction::ParametersHash(digest),
            _ => return Err(Rc::COMMAND_CODE),
        };
        if kind == SessionKind::Policy {
            state.hash_restriction = Some(restriction);
        }
        state.extend(&[&code.to_be_bytes(), digest.as_slice()]);
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
        w.tpm2b(state.digest.as_slice());
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
        let alg = self.session_hash(inv.handles[1]);
        let parameters = AuthorizationParameters::unmarshal(p, alg.size())?;
        p.finish()?;
        let slot = self.policy_slot(inv.handles[1])?;
        let name = handle_name(self, inv.handles[0]);
        let session = self.sessions.get(slot).ok_or(Rc::reference_handle(1))?;
        let restrictions = self.policy_authorization_restrictions(session, &parameters)?;
        let hierarchy = if parameters.expiration < 0 && restrictions.deadline.is_some() {
            self.secret_ticket_hierarchy(inv.handles[0])?
        } else {
            handle::RH_NULL
        };
        let response = self.policy_authorization_response(
            st::AUTH_SECRET,
            hierarchy,
            &name,
            &parameters,
            restrictions.deadline,
        )?;
        let (state, _) = self.policy_mut(slot)?;
        state.apply_authorization(
            cc::POLICY_SECRET,
            &name,
            parameters.policy_ref,
            restrictions.deadline,
            restrictions.cp_hash,
        );
        Ok(response)
    }

    /// `TPM2_PolicySigned` (Part 3 v185, §23.3) uses a public key as the policy authority.
    pub(crate) fn cmd_policy_signed(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        let alg = self.session_hash(inv.handles[1]);
        let parameters = AuthorizationParameters::unmarshal(p, alg.size())?;
        p.set_param(5);
        let signature = TpmtSignature::unmarshal_optional(p)?;
        // The signed message is hashed with the hash of the signature; see `signature_hash`.
        let sig_hash = signature_hash(&signature, alg).map_err(|rc| rc.in_param(5))?;
        p.finish()?;

        let slot = self.policy_slot(inv.handles[1])?;
        let session = self.sessions.get(slot).ok_or(Rc::reference_handle(1))?;
        let restrictions = self.policy_authorization_restrictions(session, &parameters)?;
        let key = self.object(inv.handles[0]).ok_or(Rc::reference_handle(0))?;
        if session.kind != SessionKind::Trial {
            let (fixed, supplied) = match (&key.public.params, &signature) {
                (PublicParams::Rsa { scheme, .. }, TpmtSignature::Rsassa { hash, .. }) => (
                    *scheme,
                    Scheme {
                        alg: alg::RSASSA,
                        hash: *hash,
                    },
                ),
                (PublicParams::Ecc { scheme, .. }, TpmtSignature::Ecdsa { hash, .. }) => (
                    *scheme,
                    Scheme {
                        alg: alg::ECDSA,
                        hash: *hash,
                    },
                ),
                _ => return Err(Rc::POLICY_FAIL),
            };
            if !key.has(object_attr::SIGN) || (fixed != Scheme::NULL && fixed != supplied) {
                return Err(Rc::POLICY_FAIL);
            }
            // aHash = H_sigHash(nonceTPM || expiration || cpHashA || policyRef); the message uses no
            // TPM2B lengths. A real session always has a signature here (the match above refuses
            // the null one).
            let sig_hash = sig_hash.ok_or(Rc::POLICY_FAIL)?;
            let a_hash = hash(
                sig_hash,
                &[
                    parameters.nonce,
                    &parameters.expiration.to_be_bytes(),
                    parameters.cp_hash,
                    parameters.policy_ref,
                ],
            );
            if !key.verify_signature(a_hash.as_slice(), &signature) {
                return Err(Rc::POLICY_FAIL);
            }
        }
        let name = key.name.clone();
        let response = self.policy_authorization_response(
            st::AUTH_SIGNED,
            key.hierarchy,
            &name,
            &parameters,
            restrictions.deadline,
        )?;
        let (state, _) = self.policy_mut(slot)?;
        state.apply_authorization(
            cc::POLICY_SIGNED,
            &name,
            parameters.policy_ref,
            restrictions.deadline,
            restrictions.cp_hash,
        );
        Ok(response)
    }

    fn policy_authorization_restrictions(
        &self,
        session: &Session,
        parameters: &AuthorizationParameters<'_>,
    ) -> Result<AuthorizationRestrictions, Rc> {
        if session.kind == SessionKind::Trial {
            return Ok(AuthorizationRestrictions {
                deadline: None,
                cp_hash: None,
            });
        }
        if !parameters.nonce.is_empty() && parameters.nonce != session.nonce_tpm {
            return Err(Rc::VALUE.in_param(1));
        }
        let deadline = self.policy_assertion_deadline(
            session,
            parameters.nonce,
            parameters.expiration.unsigned_abs(),
        )?;
        let cp_hash = Self::policy_assertion_cp_hash(&session.policy, parameters.cp_hash)?;
        Ok(AuthorizationRestrictions { deadline, cp_hash })
    }

    fn policy_authorization_response(
        &self,
        tag: u16,
        hierarchy: u32,
        name: &[u8],
        parameters: &AuthorizationParameters<'_>,
        deadline: Option<TimePoint>,
    ) -> Result<Output, Rc> {
        let mut ticket = TpmtTicket {
            tag,
            hierarchy: handle::RH_NULL,
            digest: Vec::new(),
        };
        let mut w = Writer::new();
        if let Some(limit) = deadline
            && parameters.expiration < 0
        {
            if limit.ms >= RESET_BOUND_TIMEOUT {
                return Err(Rc::VALUE.in_param(4));
            }
            let timeout = limit.ms
                | if parameters.nonce.is_empty() {
                    RESET_BOUND_TIMEOUT
                } else {
                    0
                };
            let proof = self.hierarchy_proof(hierarchy).ok_or(Rc::FAILURE)?;
            let transcript = self.auth_ticket_transcript(
                tag,
                timeout,
                parameters.cp_hash,
                parameters.policy_ref,
                name,
            );
            ticket.hierarchy = hierarchy;
            ticket.digest = hmac_sha256(&proof, &[&transcript]).to_vec();
            w.tpm2b(&timeout.to_be_bytes());
        } else {
            w.tpm2b(&[]);
        }
        ticket.marshal(&mut w);
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }

    /// `TPM2_PolicyTicket` (Part 3 v185, §23.5) reuses a previous authorization.
    pub(crate) fn cmd_policy_ticket(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        let alg = self.session_hash(inv.handles[0]);
        p.set_param(1);
        let timeout = p.tpm2b(8)?;
        p.set_param(2);
        let cp_hash_a = p.tpm2b(alg.size())?;
        p.set_param(3);
        let policy_ref = p.tpm2b(alg.size())?;
        p.set_param(4);
        // The Name of the authority can be that of a SHA-384 or SHA-512 object.
        let name = p.tpm2b(MAX_NAME_SIZE)?;
        p.set_param(5);
        let tag = p.u16()?;
        let code = match tag {
            st::AUTH_SECRET => cc::POLICY_SECRET,
            st::AUTH_SIGNED => cc::POLICY_SIGNED,
            _ => return Err(Rc::TAG.in_param(5)),
        };
        let hierarchy = p.u32()?;
        if !is_object_hierarchy(hierarchy) {
            return Err(Rc::VALUE.in_param(5));
        }
        let tag_digest = p.tpm2b(limits::MAX_DIGEST_SIZE)?;
        p.finish()?;
        let (slot, kind) = self.check_policy_session(inv.handles[0], 0)?;
        if kind == SessionKind::Trial {
            // Part 3 v185 §23.1 applies: compute the digest without using the ticket.
            let (state, _) = self.policy_mut(slot)?;
            state.apply_authorization(code, name, policy_ref, None, None);
            return Ok(Output::default());
        }
        let timeout = u64::from_be_bytes(timeout.try_into().map_err(|_| Rc::SIZE.in_param(1))?);
        let session = self.sessions.get(slot).ok_or(Rc::reference_handle(0))?;
        let deadline = TimePoint {
            epoch: session.nonce_time.ok_or(Rc::EXPIRED)?.epoch,
            ms: timeout & !RESET_BOUND_TIMEOUT,
        };
        if deadline.has_passed(self.time_point()) {
            return Err(Rc::EXPIRED);
        }
        let cp_hash = Self::policy_assertion_cp_hash(&session.policy, cp_hash_a)?;
        let proof = self
            .hierarchy_proof(hierarchy)
            .ok_or(Rc::TICKET.in_param(5))?;
        let transcript = self.auth_ticket_transcript(tag, timeout, cp_hash_a, policy_ref, name);
        if !hmac_sha256_verify(&proof, &[&transcript], tag_digest) {
            return Err(Rc::TICKET.in_param(5));
        }
        let (state, _) = self.policy_mut(slot)?;
        state.apply_authorization(code, name, policy_ref, Some(deadline), cp_hash);
        Ok(Output::default())
    }

    fn secret_ticket_hierarchy(&self, auth_handle: u32) -> Result<u32, Rc> {
        if let Some(object) = self.object(auth_handle) {
            return Ok(object.hierarchy);
        }
        if let Some(index) = self.persistent.nv.get(auth_handle) {
            return Ok(if index.attributes & nv_attr::PLATFORMCREATE != 0 {
                handle::RH_PLATFORM
            } else {
                handle::RH_OWNER
            });
        }
        Ok(match auth_handle {
            handle::RH_PLATFORM | handle::RH_ENDORSEMENT | handle::RH_NULL => auth_handle,
            handle::RH_OWNER | handle::RH_LOCKOUT => handle::RH_OWNER,
            // Sequence objects have no persistent owner.
            _ if self.seq_slot(auth_handle, 0).is_ok() => handle::RH_NULL,
            _ => return Err(Rc::VALUE.in_handle(1)),
        })
    }

    /// Canonical encoding of the logical transcript in Part 3 v185 §23.2.5.
    /// Lengths distinguish optional fields with otherwise identical concatenations.
    fn auth_ticket_transcript(
        &self,
        tag: u16,
        timeout: u64,
        cp_hash: &[u8],
        policy_ref: &[u8],
        name: &[u8],
    ) -> Vec<u8> {
        let mut w = Writer::new();
        w.u16(tag);
        w.tpm2b(cp_hash);
        w.tpm2b(policy_ref);
        w.tpm2b(name);
        w.u64(timeout);
        if timeout & !RESET_BOUND_TIMEOUT != 0 {
            w.bytes(&self.time_epoch);
            if timeout & RESET_BOUND_TIMEOUT != 0 {
                w.u64(self.persistent.total_reset_count);
            }
        }
        w.finish()
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
    ) -> Result<Option<TpmDigest>, Rc> {
        if cp_hash.is_empty() {
            return Ok(None);
        }
        if state.hash_restriction.is_some_and(|restriction| {
            !matches!(restriction, PolicyHashRestriction::CpHash(old) if old.as_slice() == cp_hash)
        }) {
            return Err(Rc::CPHASH);
        }
        Ok(Some(
            TpmDigest::exact(cp_hash, state.hash.size()).ok_or(Rc::SIZE.in_param(2))?,
        ))
    }

    /// `TPM2_PolicyAuthorize` with a legacy or digest verification ticket (Part 3 v185 §23.16).
    pub(crate) fn cmd_policy_authorize(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        // A real session takes an approved policy of exactly one digest of its hash; a trial one
        // may describe a policy of another TPM, so the wire limit of the two fields is at least one
        // SHA-384 digest.
        let alg = self.session_hash(inv.handles[0]);
        let field_limit = alg.size().max(limits::MAX_PCR_DIGEST_SIZE);
        p.set_param(1);
        let approved = p.tpm2b(field_limit)?;
        p.set_param(2);
        let policy_ref = p.tpm2b(field_limit)?;
        p.set_param(3);
        let key_name = p.tpm2b(MAX_NAME_SIZE)?;
        p.set_param(4);
        let ticket = VerifiedTicket::unmarshal(p)?;
        p.finish()?;

        let (name_alg, name_digest) = key_name.split_at_checked(2).ok_or(Rc::SIZE.in_param(3))?;
        let name_alg = u16::from_be_bytes([name_alg[0], name_alg[1]]);
        let name_size = digest_size(name_alg).ok_or(Rc::HASH.in_param(3))?;
        if name_digest.len() != name_size {
            return Err(Rc::SIZE.in_param(3));
        }

        let slot = self.policy_slot(inv.handles[0])?;
        let session = self.sessions.get(slot).ok_or(Rc::reference_handle(0))?;
        if session.kind == SessionKind::Policy {
            if approved != session.policy.digest.as_slice() {
                return Err(Rc::VALUE.in_param(1));
            }
            let valid = match ticket.signed_hash_alg(name_alg) {
                None => self.valid_verified_ticket(key_name, &[approved, policy_ref], &ticket),
                Some(signed_alg) => {
                    let signed_alg = HashAlg::from_alg(signed_alg).ok_or(Rc::HASH.in_param(4))?;
                    let signed = hash(signed_alg, &[approved, policy_ref]);
                    self.valid_verified_ticket(key_name, &[signed.as_slice()], &ticket)
                }
            };
            if !valid {
                return Err(Rc::POLICY);
            }
        }
        let (state, _) = self.policy_mut(slot)?;
        state.reset_digest();
        state.extend(&[&cc::POLICY_AUTHORIZE.to_be_bytes(), key_name]);
        state.extend(&[policy_ref]);
        Ok(Output::default())
    }

    /// The checks of Part 3, clause 5.6, step 8, for a policy session in `slot` that authorizes
    /// a `(command code, authorized handle)` pair with `auth_policy`.
    /// `matches_hash` checks a restriction
    /// against the authorized command's actual content. ADMIN and DUP require
    /// an explicit command code. `n` is the one-based session number.
    pub(crate) fn policy_use_check(
        &self,
        slot: usize,
        (code, entity): (u32, u32),
        matches_hash: impl Fn(PolicyHashRestriction) -> bool,
        auth_policy: &[u8],
        requires_command_code: bool,
        n: u8,
    ) -> Result<(bool, bool), Rc> {
        let session = self.sessions.get(slot).ok_or(Rc::reference_session(0))?;
        if session.kind == SessionKind::Trial {
            return Err(Rc::ATTRIBUTES.in_session(n));
        }
        let st = &session.policy;
        if requires_command_code && st.command_code.is_none() {
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
        if st.digest.as_slice() != auth_policy {
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
