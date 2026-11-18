//! Command table and dispatcher.
//!
//! Order of checks (Part 1 v184, clause 9.2, flow of Figure 2): locality, header (tag, size),
//! implemented command code (`TPM_RC_COMMAND_CODE`), startup state (`TPM_RC_INITIALIZE`) and
//! failure mode (`TPM_RC_FAILURE`), handle area, handle validity, authorization, parameters,
//! command action. A command that fails leaves the TPM state unchanged except the self-test
//! state; the actions run only after every check.
//! NOT VERIFIED against TPM 2.0 Library v185: the position of the startup check relative to the
//! command-code check (the open text lists the header checks and the handle/authorization checks
//! but not where `TPM_RC_INITIALIZE` falls); phase A answers an unknown code before `Startup`
//! with `TPM_RC_COMMAND_CODE`.
//!
//! The command table is the single source for dispatch and for `TPM_CAP_COMMANDS`: a command that
//! is not in [`COMMANDS`] gets `TPM_RC_COMMAND_CODE`, never `TPM_RC_SUCCESS`.
//! `TPM2_PCR_Allocate` (code 0x12B) is deliberately absent, see `docs/specs/VTPM.md`.
//!
//! Authorization (Part 1 v184, clauses 16.6 and 17): the first `auth` handles of a command each
//! need one session in the authorization area, a password session (`TPM_RS_PW`) or an HMAC
//! session started by `TPM2_StartAuthSession`. An HMAC authorization is checked against the
//! cpHash of the command and answered with a response HMAC over the rpHash (`session.rs`).
//! No other sessions (audit, encrypt-only) are accepted: more sessions than authorized handles is
//! `TPM_RC_AUTHSIZE`. Slice C3: the `decrypt` and `encrypt` attributes of an authorization session
//! (`paramenc.rs`): the first command parameter is decrypted after the authorization (the cpHash
//! covers the encrypted bytes), the first response parameter is encrypted before the rpHash. `TPM2_FlushContext` takes no session at all (`TPM_RC_AUTH_CONTEXT`).
//! NOT VERIFIED against TPM 2.0 Library v185/v184: the Name of a sequence object in a cpHash is
//! the Empty Buffer (Part 1, Table 6, note 1), and a response HMAC for an entity that the command
//! deleted or flushed uses the Empty Buffer as authValue (the text says so for
//! `TPM2_NV_UndefineSpaceSpecial` only).
//! Slice C1: dictionary-attack protection wraps the authorization check (`da.rs`).
//! Slice C2: a policy session in the authorization area (handle type `0x03`) is checked by
//! `policy.rs` (`policy_use_check`) before the HMAC or password of the session is looked at; the
//! role of the handle (USER, or ADMIN for `objectHandle` of `TPM2_Certify`) decides which object
//! attribute gates a password or HMAC (Part 3, clause 5.6, steps 5-7): `userWithAuth` CLEAR is
//! `TPM_RC_POLICY_FAIL` for the USER role, `adminWithPolicy` SET is `TPM_RC_AUTH_TYPE` for the
//! ADMIN role. The context commands refuse any session (`TPM_RC_AUTH_CONTEXT`).
//! Slice B of phase B: the Name of a transient or persistent object in a cpHash is its Name
//! (`nameAlg || H(TPMT_PUBLIC)`), and an object is authorized with its authValue only while it has
//! a private part (`TPM_RC_AUTH_UNAVAILABLE` otherwise; NOT VERIFIED against TPM 2.0 Library
//! v185/v184: this code for that case, see `object.rs`). With `userWithAuth` CLEAR a password or
//! HMAC for the USER role is `TPM_RC_POLICY_FAIL` (slice C2, above).

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::codes::{cc, handle, session_attr, st};
use crate::command::{HEADER_SIZE, check_password_session, parse_auth_area, parse_header};
use crate::crypto::DIGEST;
use crate::da::DaClass;
use crate::hash::{AuthValue, password_matches};
use crate::hierarchy::is_authorization_handle;
use crate::marshal::{Reader, TpmsAuthCommand, limits};
use crate::nv::attr as nv_attr;
use crate::object::is_object_hierarchy;
use crate::paramenc::{
    SessionSym, apply as apply_param_encryption, first_parameter_is_sized,
    first_response_parameter_is_sized,
};
use crate::pcr::resolve_pcr_handle;
use crate::policy::PolicyState;
use crate::response::{Output, Rc, SessionAck, error_response, success_response};
use crate::session::{
    SessionKind, authorization_key, check_hmac_session_attributes, response_hmac,
    verify_command_hmac,
};
use crate::state::Tpm;

/// One implemented command.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CommandSpec {
    pub code: u32,
    /// Handles in the command handle area (`cHandles` of `TPMA_CC`).
    pub handles: u8,
    /// Handles that need authorization (the first `auth` handles).
    pub auth: u8,
    /// A handle in the response handle area (`rHandle`).
    pub rhandle: bool,
    /// The command may write NV (`{NV}` in Part 3).
    pub nv: bool,
    /// The sequence handle is flushed on success (`{F}` in Part 3).
    pub flushed: bool,
    /// The command may flush any number of loaded contexts (`{E}` in Part 3).
    pub extensive: bool,
}

const fn spec(
    code: u32,
    handles: u8,
    auth: u8,
    rhandle: bool,
    nv: bool,
    flushed: bool,
) -> CommandSpec {
    CommandSpec {
        code,
        handles,
        auth,
        rhandle,
        nv,
        flushed,
        extensive: false,
    }
}

/// Marks a command `{E}`.
const fn ext(mut s: CommandSpec) -> CommandSpec {
    s.extensive = true;
    s
}

/// Implemented commands in ascending command-index order. Attributes (`nv`, `flushed`,
/// `extensive`, `cHandles`, `rHandle`) are from the command tables of Part 3 v184 (checked
/// 2026-11-07).
pub(crate) static COMMANDS: [CommandSpec; 60] = [
    spec(cc::EVICT_CONTROL, 2, 1, false, true, false),
    ext(spec(cc::HIERARCHY_CONTROL, 1, 1, false, true, false)),
    spec(cc::NV_UNDEFINE_SPACE, 2, 1, false, true, false),
    ext(spec(cc::CLEAR, 1, 1, false, true, false)),
    spec(cc::CLEAR_CONTROL, 1, 1, false, true, false),
    spec(cc::HIERARCHY_CHANGE_AUTH, 1, 1, false, true, false),
    spec(cc::NV_DEFINE_SPACE, 1, 1, false, true, false),
    spec(cc::CREATE_PRIMARY, 1, 1, true, false, false),
    spec(cc::NV_INCREMENT, 2, 1, false, true, false),
    spec(cc::NV_SET_BITS, 2, 1, false, true, false),
    spec(cc::NV_EXTEND, 2, 1, false, true, false),
    spec(cc::NV_WRITE, 2, 1, false, true, false),
    spec(cc::NV_WRITE_LOCK, 2, 1, false, true, false),
    spec(cc::DICTIONARY_ATTACK_LOCK_RESET, 1, 1, false, true, false),
    spec(cc::DICTIONARY_ATTACK_PARAMETERS, 1, 1, false, true, false),
    spec(cc::PCR_EVENT, 1, 1, false, true, false),
    spec(cc::PCR_RESET, 1, 1, false, true, false),
    spec(cc::SEQUENCE_COMPLETE, 1, 1, false, false, true),
    spec(cc::SELF_TEST, 0, 0, false, true, false),
    spec(cc::STARTUP, 0, 0, false, true, false),
    spec(cc::SHUTDOWN, 0, 0, false, true, false),
    spec(cc::STIR_RANDOM, 0, 0, false, true, false),
    spec(cc::CERTIFY, 2, 2, false, false, false),
    spec(cc::GET_TIME, 2, 2, false, false, false),
    spec(cc::NV_READ, 2, 1, false, false, false),
    spec(cc::NV_READ_LOCK, 2, 1, false, true, false),
    spec(cc::POLICY_SECRET, 2, 1, false, false, false),
    spec(cc::CREATE, 1, 1, false, false, false),
    spec(cc::ECDH_Z_GEN, 1, 1, false, false, false),
    spec(cc::HMAC, 1, 1, false, false, false),
    spec(cc::LOAD, 1, 1, true, false, false),
    spec(cc::QUOTE, 1, 1, false, false, false),
    spec(cc::RSA_DECRYPT, 1, 1, false, false, false),
    spec(cc::SEQUENCE_UPDATE, 1, 1, false, false, false),
    spec(cc::SIGN, 1, 1, false, false, false),
    spec(cc::UNSEAL, 1, 1, false, false, false),
    spec(cc::CONTEXT_LOAD, 0, 0, true, false, false),
    spec(cc::CONTEXT_SAVE, 1, 0, false, false, false),
    spec(cc::FLUSH_CONTEXT, 0, 0, false, false, false),
    spec(cc::LOAD_EXTERNAL, 0, 0, true, false, false),
    spec(cc::NV_READ_PUBLIC, 1, 0, false, false, false),
    spec(cc::POLICY_AUTH_VALUE, 1, 0, false, false, false),
    spec(cc::POLICY_COMMAND_CODE, 1, 0, false, false, false),
    spec(cc::POLICY_OR, 1, 0, false, false, false),
    spec(cc::READ_PUBLIC, 1, 0, false, false, false),
    spec(cc::RSA_ENCRYPT, 1, 0, false, false, false),
    spec(cc::START_AUTH_SESSION, 2, 0, true, false, false),
    spec(cc::VERIFY_SIGNATURE, 1, 0, false, false, false),
    spec(cc::GET_CAPABILITY, 0, 0, false, false, false),
    spec(cc::GET_RANDOM, 0, 0, false, false, false),
    spec(cc::GET_TEST_RESULT, 0, 0, false, false, false),
    spec(cc::HASH, 0, 0, false, false, false),
    spec(cc::PCR_READ, 0, 0, false, false, false),
    spec(cc::POLICY_PCR, 1, 0, false, false, false),
    spec(cc::POLICY_RESTART, 1, 0, false, false, false),
    spec(cc::READ_CLOCK, 0, 0, false, false, false),
    spec(cc::PCR_EXTEND, 1, 1, false, true, false),
    spec(cc::EVENT_SEQUENCE_COMPLETE, 2, 2, false, true, true),
    spec(cc::HASH_SEQUENCE_START, 0, 0, true, false, false),
    spec(cc::POLICY_GET_DIGEST, 1, 0, false, false, false),
];

/// What a handler needs besides the parameter reader.
pub(crate) struct Invocation {
    pub handles: [u32; 2],
}

fn lookup(code: u32) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|c| c.code == code)
}

pub(crate) fn execute(tpm: &mut Tpm, locality: u8, command: &[u8]) -> Vec<u8> {
    match run(tpm, locality, command) {
        Ok(v) if v.len() <= limits::MAX_RESPONSE_SIZE => v,
        Ok(_) => error_response(Rc::FAILURE),
        Err(rc) => error_response(rc),
    }
}

/// `true` if handle position `i` of command `code` is a PCR (authorized by the empty value).
fn is_pcr_position(code: u32, i: usize) -> bool {
    matches!(
        (code, i),
        (cc::PCR_EXTEND | cc::PCR_EVENT | cc::PCR_RESET, 0) | (cc::EVENT_SEQUENCE_COMPLETE, 0)
    )
}

/// Authorization value of the entity behind handle number `i` of the command, for checking a
/// command authorization. PCR and `TPM_RH_NULL` use the empty authorization (PTP 1.07, clause
/// 4.7: Empty Auth for all PCR); a hierarchy needs its enable SET (`TPM_RC_HIERARCHY`).
fn handle_auth(
    tpm: &Tpm,
    code: u32,
    handles: &[u32; 2],
    i: usize,
    admin: bool,
    policy: bool,
) -> Result<AuthValue, Rc> {
    let h = handles[i];
    if is_pcr_position(code, i) || h == handle::RH_NULL {
        return Ok(AuthValue::empty());
    }
    let n = (i + 1) as u8;
    match handle::handle_type(h) {
        handle::HT_PERMANENT => {
            if !tpm.permanent_enabled(h) {
                return Err(Rc::HIERARCHY.in_handle(n));
            }
            tpm.permanent_auth(h).ok_or(Rc::VALUE.in_handle(n))
        }
        // A policy session on an NV index: the indices have no policy access (`POLICYREAD` and
        // `POLICYWRITE` are refused at definition, Part 3, clause 5.6, step 7).
        handle::HT_NV_INDEX if policy => Err(Rc::AUTH_UNAVAILABLE),
        handle::HT_NV_INDEX => {
            let index = tpm.persistent.nv.get(h).ok_or(Rc::HANDLE.in_handle(n))?;
            if code == cc::POLICY_SECRET && index.attributes & nv_attr::AUTHREAD == 0 {
                return Err(Rc::AUTH_TYPE);
            }
            Ok(index.auth.clone())
        }
        // Transient (not a sequence) and persistent objects (slice B).
        handle::HT_TRANSIENT | handle::HT_PERSISTENT if tpm.object(h).is_some() => {
            // PolicySecret requires userWithAuth even for policy-based authorization.
            tpm.object_auth(h, i, admin, policy && code != cc::POLICY_SECRET)
        }
        _ => {
            let slot = tpm.seq_slot(h, i)?;
            // `AuthValue` is a fixed buffer wiped on drop: no un-wiped copy in a plain `Vec`.
            Ok(tpm.seq_auth(slot))
        }
    }
}

/// The role of handle number `i` of a command: ADMIN for `objectHandle` of `TPM2_Certify`, USER
/// for every other authorized handle of the implemented commands.
fn admin_role(code: u32, i: usize) -> bool {
    code == cc::CERTIFY && i == 0
}

/// The `authPolicy` of the entity behind handle number `i`: that of an object, the Empty Buffer
/// for everything else (hierarchies have no `authPolicy` without `TPM2_SetPrimaryPolicy`).
fn handle_auth_policy(tpm: &Tpm, handles: &[u32; 2], i: usize) -> Vec<u8> {
    tpm.object(handles[i])
        .map(|o| o.public.auth_policy.clone())
        .unwrap_or_default()
}

/// DA class of the authorization of handle number `i` of the command (`da.rs`): a PCR and
/// `TPM_RH_NULL` are exempt.
fn da_class(tpm: &Tpm, code: u32, handles: &[u32; 2], i: usize) -> DaClass {
    let h = handles[i];
    if is_pcr_position(code, i) || h == handle::RH_NULL {
        return DaClass::Exempt;
    }
    tpm.da_class_of(h)
}

/// The authorization value for a response HMAC, read after the command ran: the current value
/// regardless of the enables, the Empty Buffer for an entity that no longer exists.
fn response_auth(tpm: &Tpm, code: u32, handles: &[u32; 2], i: usize) -> AuthValue {
    let h = handles[i];
    if is_pcr_position(code, i) || h == handle::RH_NULL {
        return AuthValue::empty();
    }
    match handle::handle_type(h) {
        handle::HT_PERMANENT => tpm.permanent_auth(h).unwrap_or_else(AuthValue::empty),
        handle::HT_NV_INDEX => tpm.nv_auth(h).unwrap_or_else(AuthValue::empty),
        handle::HT_TRANSIENT | handle::HT_PERSISTENT if tpm.object(h).is_some() => {
            tpm.object_auth_now(h).unwrap_or_else(AuthValue::empty)
        }
        _ => tpm
            .seq_slot(h, i)
            .map_or_else(|_| AuthValue::empty(), |slot| tpm.seq_auth(slot)),
    }
}

/// Name of the entity behind a handle for a cpHash (Part 1 v184, Table 6): the handle itself for
/// PCR, sessions and permanent values, `nameAlg || H(public)` for an NV index, the Empty Buffer
/// for a sequence object.
pub(crate) fn handle_name(tpm: &Tpm, h: u32) -> Vec<u8> {
    match handle::handle_type(h) {
        handle::HT_NV_INDEX => tpm.nv_name(h).unwrap_or_else(|| h.to_be_bytes().to_vec()),
        // An object has its Name, a sequence object the Empty Buffer.
        handle::HT_TRANSIENT | handle::HT_PERSISTENT => tpm.object_name(h).unwrap_or_default(),
        _ => h.to_be_bytes().to_vec(),
    }
}

/// cpHash of a command (Part 1, clause 16.7): `H(commandCode || Name1 || ... || parameters)`.
fn command_parameter_hash(
    tpm: &Tpm,
    spec: &CommandSpec,
    handles: &[u32; 2],
    parameters: &[u8],
) -> [u8; DIGEST] {
    let mut h = Sha256::new();
    h.update(spec.code.to_be_bytes());
    for handle in handles.iter().take(usize::from(spec.handles)) {
        h.update(handle_name(tpm, *handle));
    }
    h.update(parameters);
    h.finalize().into()
}

/// rpHash of a response (Part 1, clause 16.8): `H(responseCode || commandCode || parameters)`.
fn response_parameter_hash(code: u32, parameters: &[u8]) -> [u8; DIGEST] {
    let mut h = Sha256::new();
    h.update(Rc::SUCCESS.0.to_be_bytes());
    h.update(code.to_be_bytes());
    h.update(parameters);
    h.finalize().into()
}

/// How a session of the authorization area is used.
enum SessionUse {
    Password,
    /// HMAC session in `slot`; `used_auth` records whether the authValue of the entity went into
    /// the key of the command HMAC (and therefore goes into the response HMAC).
    Hmac {
        slot: usize,
        used_auth: bool,
    },
}

/// Format checks of the sessions (before any authValue is looked at).
fn classify_sessions(
    tpm: &Tpm,
    code: u32,
    sessions: &[TpmsAuthCommand],
) -> Result<Vec<SessionUse>, Rc> {
    let mut uses: Vec<SessionUse> = Vec::with_capacity(sessions.len());
    let (mut decrypting, mut encrypting) = (false, false);
    for (i, s) in sessions.iter().enumerate() {
        let n = i + 1;
        let hmac_handle = s.session_handle != handle::RS_PW
            && matches!(
                handle::handle_type(s.session_handle),
                handle::HT_HMAC_SESSION | handle::HT_POLICY_SESSION
            );
        if !hmac_handle {
            // Password session, or a handle that names no session at all.
            check_password_session(s, n)?;
            uses.push(SessionUse::Password);
            continue;
        }
        let slot = tpm
            .sessions
            .slot_of(s.session_handle)
            .ok_or_else(|| Rc::reference_session(i))?;
        // A session handle may occur only once in a command (Part 1, clause 16.6.3).
        if uses
            .iter()
            .any(|u| matches!(u, SessionUse::Hmac { slot: o, .. } if *o == slot))
        {
            return Err(Rc::VALUE.in_session(n as u8));
        }
        check_hmac_session_attributes(s.attributes, n)?;
        let session = tpm
            .sessions
            .get(slot)
            .ok_or_else(|| Rc::reference_session(i))?;
        if s.nonce.len() > session.nonce_tpm.len() {
            return Err(Rc::NONCE.in_session(n as u8));
        }
        // Parameter encryption (`paramenc.rs`): the session needs a symmetric algorithm, the
        // command a sized first parameter (response parameter), and only one session of the
        // command may decrypt and one encrypt.
        let num = n as u8;
        if s.attributes & (session_attr::DECRYPT | session_attr::ENCRYPT) != 0
            && session.symmetric == SessionSym::Null
        {
            return Err(Rc::SYMMETRIC.in_session(num));
        }
        if s.attributes & session_attr::DECRYPT != 0 {
            if !first_parameter_is_sized(code) || decrypting {
                return Err(Rc::ATTRIBUTES.in_session(num));
            }
            decrypting = true;
        }
        if s.attributes & session_attr::ENCRYPT != 0 {
            if !first_response_parameter_is_sized(code) || encrypting {
                return Err(Rc::ATTRIBUTES.in_session(num));
            }
            encrypting = true;
        }
        uses.push(SessionUse::Hmac {
            slot,
            used_auth: true,
        });
    }
    Ok(uses)
}

fn run(tpm: &mut Tpm, locality: u8, command: &[u8]) -> Result<Vec<u8>, Rc> {
    // Only locality 0 is served (ADR-0009, decision 4).
    if locality != 0 {
        return Err(Rc::LOCALITY);
    }
    let header = parse_header(command)?;
    let spec = lookup(header.code).ok_or(Rc::COMMAND_CODE)?;

    // Failure mode answers only GetTestResult and GetCapability, and only without sessions.
    let failure_ok = matches!(header.code, cc::GET_TEST_RESULT | cc::GET_CAPABILITY);
    if tpm.failed && (!failure_ok || header.tag != st::NO_SESSIONS) {
        return Err(Rc::FAILURE);
    }
    if !tpm.started && header.code != cc::STARTUP && !(tpm.failed && failure_ok) {
        return Err(Rc::INITIALIZE);
    }
    if tpm.started && header.code == cc::STARTUP {
        return Err(Rc::INITIALIZE);
    }

    let mut r = Reader::new(&command[HEADER_SIZE..]);
    let mut handles = [0u32; 2];
    for slot in handles.iter_mut().take(usize::from(spec.handles)) {
        *slot = r.u32()?;
    }
    check_handles(tpm, spec, &handles)?;

    // `TPM2_FlushContext` allows no session of any type (Part 3, clause 28.4.1).
    let with_sessions = header.tag == st::SESSIONS;
    if with_sessions
        && matches!(
            spec.code,
            cc::FLUSH_CONTEXT | cc::CONTEXT_SAVE | cc::CONTEXT_LOAD
        )
    {
        return Err(Rc::AUTH_CONTEXT);
    }

    // Authorization area.
    let sessions = if with_sessions {
        parse_auth_area(&mut r)?
    } else {
        Vec::new()
    };
    // NOT VERIFIED against TPM 2.0 Library v185/v184: TPM_RC_AUTHSIZE при authorizationSize < 9
    // для SESSIONS. With `TPM_ST_SESSIONS` and `authorizationSize = 0` a command that needs no
    // authorization (e.g. GetRandom) is served; the behaviour is kept, the spec text was not checked.
    let needed = usize::from(spec.auth);
    if sessions.len() < needed {
        return Err(Rc::AUTH_MISSING);
    }
    if sessions.len() > needed {
        return Err(Rc::AUTHSIZE);
    }
    let mut uses = classify_sessions(tpm, spec.code, &sessions)?;
    let params_start = command.len() - r.remaining();
    let parameters = &command[params_start..];

    // Verify every authorization. cpHash is computed once, with the first HMAC or policy
    // session's hash (SHA-256 is the only session hash).
    let mut cp_hash: Option<[u8; DIGEST]> = None;
    // `authValue` of the entity of each session, for `sessionValue` of parameter encryption.
    let mut entity_auths: Vec<Zeroizing<Vec<u8>>> = Vec::with_capacity(sessions.len());
    for (i, s) in sessions.iter().enumerate() {
        let n = (i + 1) as u8;
        let admin = admin_role(spec.code, i);
        let policy_slot = match &uses[i] {
            SessionUse::Hmac { slot, .. } => tpm
                .sessions
                .get(*slot)
                .filter(|x| x.kind.is_policy())
                .map(|_| *slot),
            SessionUse::Password => None,
        };
        let auth = handle_auth(tpm, spec.code, &handles, i, admin, policy_slot.is_some())?;
        entity_auths.push(Zeroizing::new(auth.as_slice().to_vec()));
        // Dictionary attack protection (`da.rs`): the lockout check comes before the authValue
        // check (Part 3, clause 5.6); a wrong authValue of a DA-protected entity is
        // `TPM_RC_AUTH_FAIL` and counts, of any other `TPM_RC_BAD_AUTH` (Part 1, clause 17.6.8).
        let class = da_class(tpm, spec.code, &handles, i);
        match &mut uses[i] {
            SessionUse::Password => {
                tpm.da_check(class)?;
                if !password_matches(&s.hmac, auth.as_slice()) {
                    return Err(tpm.da_auth_failed(class, n));
                }
            }
            SessionUse::Hmac { slot, used_auth } => {
                let cp = *cp_hash
                    .get_or_insert_with(|| command_parameter_hash(tpm, spec, &handles, parameters));
                if let Some(pslot) = policy_slot {
                    // A policy session: the policy is validated before any HMAC (Part 1, clause
                    // 17.8.3), a policy failure never touches the DA counters.
                    let policy = handle_auth_policy(tpm, &handles, i);
                    let (need_auth, need_password) =
                        tpm.policy_use_check(pslot, spec.code, &cp, &policy, admin, n)?;
                    let session = tpm.sessions.get(*slot).ok_or(Rc::reference_session(i))?;
                    if need_password {
                        tpm.da_check(class)?;
                        if !password_matches(&s.hmac, auth.as_slice()) {
                            return Err(tpm.da_auth_failed(class, n));
                        }
                        *used_auth = false;
                    } else if need_auth {
                        tpm.da_check(class)?;
                        let mut key = Zeroizing::new(session.key.as_slice().to_vec());
                        key.extend_from_slice(auth.as_slice());
                        if s.hmac.is_empty()
                            || !verify_command_hmac(
                                session,
                                &key,
                                &cp,
                                &s.nonce,
                                s.attributes,
                                &s.hmac,
                            )
                        {
                            return Err(tpm.da_auth_failed(class, n));
                        }
                        *used_auth = true;
                    } else {
                        // Only the session key (possibly empty) proves anything: no authValue,
                        // so no DA.
                        if !verify_command_hmac(
                            session,
                            session.key.as_slice(),
                            &cp,
                            &s.nonce,
                            s.attributes,
                            &s.hmac,
                        ) {
                            return Err(Rc::BAD_AUTH.in_session(n));
                        }
                        *used_auth = false;
                    }
                } else {
                    tpm.da_check(class)?;
                    let session = tpm.sessions.get(*slot).ok_or(Rc::reference_session(i))?;
                    let (key, with_auth) = authorization_key(session, handles[i], &auth);
                    if !verify_command_hmac(session, &key, &cp, &s.nonce, s.attributes, &s.hmac) {
                        return Err(tpm.da_auth_failed(class, n));
                    }
                    *used_auth = with_auth;
                }
            }
        }
    }

    // The fallible preparation of the response goes before the command's action.
    let mut new_nonces: Vec<Vec<u8>> = Vec::with_capacity(sessions.len());
    for u in &uses {
        let mut nonce = Vec::new();
        if let SessionUse::Hmac { slot, .. } = u {
            let len = tpm.sessions.get(*slot).map_or(0, |s| s.nonce_tpm.len());
            nonce = vec![0u8; len];
            tpm.drbg.generate(&mut nonce).ok_or(Rc::FAILURE)?;
        }
        new_nonces.push(nonce);
    }

    // Parameter decryption (after the authorization: the cpHash covers the encrypted bytes).
    let mut plain_params = Zeroizing::new(parameters.to_vec());
    for (i, s) in sessions.iter().enumerate() {
        if s.attributes & session_attr::DECRYPT == 0 {
            continue;
        }
        if let SessionUse::Hmac { slot, .. } = &uses[i]
            && let Some(session) = tpm.sessions.get(*slot)
        {
            let mut value = Zeroizing::new(session.key.as_slice().to_vec());
            value.extend_from_slice(&entity_auths[i]);
            apply_param_encryption(
                session.symmetric,
                &value,
                &s.nonce,
                &session.nonce_tpm,
                &mut plain_params,
                false,
            );
        }
    }

    let inv = Invocation { handles };
    let mut p = Reader::new(&plain_params);
    let mut out: Output = match spec.code {
        cc::STARTUP => tpm.cmd_startup(&mut p)?,
        cc::SHUTDOWN => tpm.cmd_shutdown(&mut p)?,
        cc::SELF_TEST => tpm.cmd_self_test(&mut p)?,
        cc::GET_TEST_RESULT => tpm.cmd_get_test_result(&mut p)?,
        cc::GET_CAPABILITY => tpm.cmd_get_capability(&mut p)?,
        cc::GET_RANDOM => tpm.cmd_get_random(&mut p)?,
        cc::STIR_RANDOM => tpm.cmd_stir_random(&mut p)?,
        cc::PCR_READ => tpm.cmd_pcr_read(&mut p)?,
        cc::PCR_EXTEND => tpm.cmd_pcr_extend(&inv, &mut p)?,
        cc::PCR_EVENT => tpm.cmd_pcr_event(&inv, &mut p)?,
        cc::PCR_RESET => tpm.cmd_pcr_reset(&inv, &mut p)?,
        cc::HASH => tpm.cmd_hash(&mut p)?,
        cc::HASH_SEQUENCE_START => tpm.cmd_hash_sequence_start(&mut p)?,
        cc::SEQUENCE_UPDATE => tpm.cmd_sequence_update(&inv, &mut p)?,
        cc::SEQUENCE_COMPLETE => tpm.cmd_sequence_complete(&inv, &mut p)?,
        cc::EVENT_SEQUENCE_COMPLETE => tpm.cmd_event_sequence_complete(&inv, &mut p)?,
        cc::READ_CLOCK => tpm.cmd_read_clock(&mut p)?,
        cc::HIERARCHY_CONTROL => tpm.cmd_hierarchy_control(&inv, &mut p)?,
        cc::HIERARCHY_CHANGE_AUTH => tpm.cmd_hierarchy_change_auth(&inv, &mut p)?,
        cc::CLEAR => tpm.cmd_clear(&mut p)?,
        cc::CLEAR_CONTROL => tpm.cmd_clear_control(&inv, &mut p)?,
        cc::START_AUTH_SESSION => tpm.cmd_start_auth_session(&inv, &mut p)?,
        cc::FLUSH_CONTEXT => tpm.cmd_flush_context(&mut p)?,
        cc::NV_DEFINE_SPACE => tpm.cmd_nv_define_space(&inv, &mut p)?,
        cc::NV_UNDEFINE_SPACE => tpm.cmd_nv_undefine_space(&inv, &mut p)?,
        cc::NV_READ_PUBLIC => tpm.cmd_nv_read_public(&inv, &mut p)?,
        cc::NV_WRITE => tpm.cmd_nv_write(&inv, &mut p)?,
        cc::NV_READ => tpm.cmd_nv_read(&inv, &mut p)?,
        cc::NV_WRITE_LOCK => tpm.cmd_nv_write_lock(&inv, &mut p)?,
        cc::NV_READ_LOCK => tpm.cmd_nv_read_lock(&inv, &mut p)?,
        cc::CREATE_PRIMARY => tpm.cmd_create_primary(&inv, &mut p)?,
        cc::CREATE => tpm.cmd_create(&inv, &mut p)?,
        cc::LOAD => tpm.cmd_load(&inv, &mut p)?,
        cc::LOAD_EXTERNAL => tpm.cmd_load_external(&mut p)?,
        cc::READ_PUBLIC => tpm.cmd_read_public(&inv, &mut p)?,
        cc::UNSEAL => tpm.cmd_unseal(&inv, &mut p)?,
        cc::EVICT_CONTROL => tpm.cmd_evict_control(&inv, &mut p)?,
        cc::SIGN => tpm.cmd_sign(&inv, &mut p)?,
        cc::VERIFY_SIGNATURE => tpm.cmd_verify_signature(&inv, &mut p)?,
        cc::RSA_ENCRYPT => tpm.cmd_rsa_encrypt(&inv, &mut p)?,
        cc::RSA_DECRYPT => tpm.cmd_rsa_decrypt(&inv, &mut p)?,
        cc::ECDH_Z_GEN => tpm.cmd_ecdh_zgen(&inv, &mut p)?,
        cc::DICTIONARY_ATTACK_LOCK_RESET => tpm.cmd_dictionary_attack_lock_reset(&mut p)?,
        cc::DICTIONARY_ATTACK_PARAMETERS => tpm.cmd_dictionary_attack_parameters(&mut p)?,
        cc::POLICY_RESTART => tpm.cmd_policy_restart(&inv, &mut p)?,
        cc::POLICY_PCR => tpm.cmd_policy_pcr(&inv, &mut p)?,
        cc::POLICY_OR => tpm.cmd_policy_or(&inv, &mut p)?,
        cc::POLICY_AUTH_VALUE => tpm.cmd_policy_auth_value(&inv, &mut p)?,
        cc::POLICY_COMMAND_CODE => tpm.cmd_policy_command_code(&inv, &mut p)?,
        cc::POLICY_SECRET => tpm.cmd_policy_secret(&inv, &mut p)?,
        cc::POLICY_GET_DIGEST => tpm.cmd_policy_get_digest(&inv, &mut p)?,
        cc::CONTEXT_SAVE => tpm.cmd_context_save(&inv, &mut p)?,
        cc::CONTEXT_LOAD => tpm.cmd_context_load(&mut p)?,
        cc::HMAC => tpm.cmd_hmac(&inv, &mut p)?,
        cc::QUOTE => tpm.cmd_quote(&inv, &mut p)?,
        cc::CERTIFY => tpm.cmd_certify(&inv, &mut p)?,
        cc::GET_TIME => tpm.cmd_get_time(&inv, &mut p)?,
        cc::NV_INCREMENT => tpm.cmd_nv_increment(&inv, &mut p)?,
        cc::NV_EXTEND => tpm.cmd_nv_extend(&inv, &mut p)?,
        cc::NV_SET_BITS => tpm.cmd_nv_set_bits(&inv, &mut p)?,
        // Unreachable while the table and this match agree; a missing arm is the safe answer.
        _ => return Err(Rc::COMMAND_CODE),
    };

    // Response parameter encryption (before the rpHash).
    for (i, s) in sessions.iter().enumerate() {
        if s.attributes & session_attr::ENCRYPT == 0 {
            continue;
        }
        if let SessionUse::Hmac { slot, .. } = &uses[i]
            && let Some(session) = tpm.sessions.get(*slot)
        {
            // `sessionValue` for the response uses the authValue as it is AFTER the command, the
            // same value as the response HMAC (a command that changes the auth of its own
            // authorization entity returns no parameters, so the order cannot show in a result).
            // NOT VERIFIED: the text of Part 1, 19.1 does not say at which moment the authValue is
            // taken for the encryption of a response (VTPM.md, item 59).
            let now = response_auth(tpm, spec.code, &handles, i);
            let mut value = Zeroizing::new(session.key.as_slice().to_vec());
            value.extend_from_slice(now.as_slice());
            apply_param_encryption(
                session.symmetric,
                &value,
                &new_nonces[i],
                &s.nonce,
                &mut out.params,
                true,
            );
        }
    }

    // Acknowledgements roll the nonce and release sessions that do not continue.
    let rp_hash = response_parameter_hash(spec.code, &out.params);
    let mut acks: Vec<SessionAck> = Vec::with_capacity(sessions.len());
    for (i, s) in sessions.iter().enumerate() {
        match &uses[i] {
            SessionUse::Password => acks.push(SessionAck::password(s.attributes)),
            SessionUse::Hmac { slot, used_auth } => {
                let now = response_auth(tpm, spec.code, &handles, i);
                let nonce = core::mem::take(&mut new_nonces[i]);
                let hmac = match tpm.sessions.get(*slot) {
                    // An empty command HMAC is answered with an empty one (clause 17.6.15).
                    Some(session) if !s.hmac.is_empty() => {
                        let mut key = session.key.as_slice().to_vec();
                        if *used_auth {
                            key.extend_from_slice(now.as_slice());
                        }
                        let tag =
                            response_hmac(&key, &rp_hash, &nonce, &s.nonce, s.attributes).to_vec();
                        zeroize::Zeroize::zeroize(&mut key);
                        tag
                    }
                    _ => Vec::new(),
                };
                if s.attributes & session_attr::CONTINUE_SESSION == 0 {
                    tpm.sessions.release(*slot);
                } else if let Some(session) = tpm.sessions.get_mut(*slot) {
                    session.nonce_tpm = nonce.clone();
                    // All sessions in this area authorize handles. A continuing policy session
                    // starts a fresh policy after its response has been protected (Part 1 v185,
                    // 16.7.1 and 16.7.8).
                    if session.kind == SessionKind::Policy {
                        session.policy = PolicyState::new();
                    }
                }
                acks.push(SessionAck {
                    nonce,
                    attributes: s.attributes,
                    hmac,
                });
            }
        }
    }
    Ok(success_response(&out, &acks, with_sessions))
}

/// Handle validity (type, loaded) and access control before authorization (Part 1, clause 9.2).
fn check_handles(tpm: &Tpm, spec: &CommandSpec, handles: &[u32; 2]) -> Result<(), Rc> {
    match spec.code {
        cc::PCR_EXTEND | cc::PCR_EVENT => {
            resolve_pcr_handle(handles[0], true, 1)?;
        }
        cc::PCR_RESET => {
            resolve_pcr_handle(handles[0], false, 1)?;
        }
        cc::SEQUENCE_UPDATE | cc::SEQUENCE_COMPLETE => {
            tpm.seq_slot(handles[0], 0)?;
        }
        cc::EVENT_SEQUENCE_COMPLETE => {
            resolve_pcr_handle(handles[0], true, 1)?;
            tpm.seq_slot(handles[1], 1)?;
        }
        cc::HIERARCHY_CONTROL => {
            // `TPMI_RH_BASE_HIERARCHY` (Part 2, Table 64): owner, platform, endorsement.
            if !matches!(
                handles[0],
                handle::RH_OWNER | handle::RH_PLATFORM | handle::RH_ENDORSEMENT
            ) {
                return Err(Rc::VALUE.in_handle(1));
            }
        }
        cc::HIERARCHY_CHANGE_AUTH => {
            // `TPMI_RH_HIERARCHY_AUTH` without `TPM_RH_NULL` (Part 3, Table 188).
            if !is_authorization_handle(handles[0]) {
                return Err(Rc::VALUE.in_handle(1));
            }
        }
        cc::CLEAR | cc::CLEAR_CONTROL => {
            // `TPMI_RH_CLEAR` (Part 2, Table 69): lockout or platform.
            if !matches!(handles[0], handle::RH_LOCKOUT | handle::RH_PLATFORM) {
                return Err(Rc::VALUE.in_handle(1));
            }
        }
        cc::START_AUTH_SESSION => check_start_auth_session_handles(tpm, handles)?,
        cc::CREATE_PRIMARY => {
            // `TPMI_RH_HIERARCHY` (Part 3, Table 174): owner, platform, endorsement or null.
            if !is_object_hierarchy(handles[0]) {
                return Err(Rc::VALUE.in_handle(1));
            }
        }
        cc::CREATE
        | cc::LOAD
        | cc::READ_PUBLIC
        | cc::UNSEAL
        | cc::SIGN
        | cc::VERIFY_SIGNATURE
        | cc::RSA_ENCRYPT
        | cc::RSA_DECRYPT
        | cc::ECDH_Z_GEN
        | cc::HMAC => tpm.check_object_handle(handles[0], 0, false)?,
        // `signHandle` may be `TPM_RH_NULL` (Part 3, clause 18.1: `TPMI_DH_OBJECT+`).
        cc::QUOTE => check_sign_handle(tpm, handles[0], 0)?,
        cc::CERTIFY => {
            tpm.check_object_handle(handles[0], 0, false)?;
            check_sign_handle(tpm, handles[1], 1)?;
        }
        cc::GET_TIME => {
            // `TPMI_RH_ENDORSEMENT` (Part 3, Table 99): `TPM_RH_ENDORSEMENT` only.
            if handles[0] != handle::RH_ENDORSEMENT {
                return Err(Rc::VALUE.in_handle(1));
            }
            check_sign_handle(tpm, handles[1], 1)?;
        }
        cc::POLICY_RESTART
        | cc::POLICY_PCR
        | cc::POLICY_OR
        | cc::POLICY_AUTH_VALUE
        | cc::POLICY_COMMAND_CODE
        | cc::POLICY_GET_DIGEST => tpm.check_policy_session(handles[0], 0)?,
        cc::POLICY_SECRET => tpm.check_policy_session(handles[1], 1)?,
        cc::DICTIONARY_ATTACK_LOCK_RESET | cc::DICTIONARY_ATTACK_PARAMETERS => {
            // `TPMI_RH_LOCKOUT` (Part 3, Tables 193 and 195): `TPM_RH_LOCKOUT` only.
            if handles[0] != handle::RH_LOCKOUT {
                return Err(Rc::VALUE.in_handle(1));
            }
        }
        cc::EVICT_CONTROL => {
            // `TPMI_RH_PROVISION` (Part 3, Table 213): owner or platform.
            if !matches!(handles[0], handle::RH_OWNER | handle::RH_PLATFORM) {
                return Err(Rc::VALUE.in_handle(1));
            }
            tpm.check_object_handle(handles[1], 1, true)?;
        }
        cc::NV_DEFINE_SPACE
        | cc::NV_UNDEFINE_SPACE
        | cc::NV_READ_PUBLIC
        | cc::NV_WRITE
        | cc::NV_WRITE_LOCK
        | cc::NV_READ
        | cc::NV_READ_LOCK
        | cc::NV_INCREMENT
        | cc::NV_EXTEND
        | cc::NV_SET_BITS => tpm.check_nv_handles(spec.code, handles)?,
        _ => {}
    }
    Ok(())
}

/// A `signHandle` (`TPMI_DH_OBJECT+`): a loaded object or `TPM_RH_NULL`.
fn check_sign_handle(tpm: &Tpm, h: u32, idx: usize) -> Result<(), Rc> {
    if h == handle::RH_NULL {
        Ok(())
    } else {
        tpm.check_object_handle(h, idx, false)
    }
}

/// `tpmKey` and `bind` of `TPM2_StartAuthSession`: `tpmKey` is `TPM_RH_NULL` (salted sessions
/// are not implemented: a loaded object as `tpmKey` is `TPM_RC_VALUE`, an unloaded transient
/// handle "not loaded", a missing persistent one `TPM_RC_HANDLE`), `bind` is `TPM_RH_NULL` or a
/// hierarchy authorization handle (binding to an object is not implemented).
/// NOT VERIFIED against TPM 2.0 Library v185/v184: the code `TPM_RC_VALUE` for an object handle
/// that the TPM could use but this implementation does not.
fn check_start_auth_session_handles(tpm: &Tpm, handles: &[u32; 2]) -> Result<(), Rc> {
    let key = handles[0];
    if key != handle::RH_NULL {
        return Err(match handle::handle_type(key) {
            _ if tpm.object(key).is_some() => Rc::VALUE.in_handle(1),
            handle::HT_TRANSIENT => Rc::reference_handle(0),
            handle::HT_PERSISTENT => Rc::HANDLE.in_handle(1),
            _ => Rc::VALUE.in_handle(1),
        });
    }
    let bind = handles[1];
    if bind == handle::RH_NULL || is_authorization_handle(bind) {
        return Ok(());
    }
    Err(match handle::handle_type(bind) {
        _ if tpm.object(bind).is_some() => Rc::VALUE.in_handle(2),
        handle::HT_TRANSIENT => Rc::reference_handle(1),
        handle::HT_PERSISTENT => Rc::HANDLE.in_handle(2),
        // NV indices, PCR, ...: not bindable in slice A.
        _ => Rc::VALUE.in_handle(2),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_unique_and_in_profile() {
        let mut prev = 0;
        for c in &COMMANDS {
            assert!(c.code > prev, "{:#x} out of order", c.code);
            prev = c.code;
            assert!(c.auth <= c.handles);
        }
        // PCR_Allocate (0x12B) and PCR_SetAuthPolicy/Value are not implemented.
        for code in [0x12Bu32, 0x12C, 0x183] {
            assert!(lookup(code).is_none());
        }
        assert_eq!(COMMANDS.len(), 60);
    }
}
